#!/usr/bin/env python3
"""Deploy a dpl unit over HTTP.

    git archive --format tar.gz HEAD | curl --fail-with-body --data-binary @- http://<server>:8099/<unit>

Accepts requests only from the IPs in DEPLOY_ALLOW (space-separated), saves the
archive to a temp file and runs `dpl deploy <unit> <file>`. The response is the
dpl output: status 200 on success, 500 on failure. On failure the `Log: <path>`
line is replaced with the tail of that log: the client cannot read server files.

    curl --fail-with-body 'http://<server>:8099/<unit>/build.log?lines=1000'

GET returns the last `lines` (default 1000) of {DPL_BASE}/state/<unit>/log/<name>
for build, runtime, timers and access logs.
"""
import os
import re
import signal
import subprocess
import sys
import tempfile
from collections import deque
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit

ALLOW = set(os.environ['DEPLOY_ALLOW'].split())
PORT = int(os.environ.get('DEPLOY_PORT', '8099'))
BASE = os.environ.get('DPL_BASE') or '/opt/dpl'
LOGS = {'build.log', 'runtime.log', 'timers.log', 'access.log'}  # files in {base}/state/{unit}/log/
UNIT = re.compile(r'[a-z0-9]+(-[a-z0-9]+)*')  # dpl unit names
LOG_LINE = re.compile(r'^ *Log: (\S+)\s*\Z', re.M)  # last line of a failed `dpl deploy`


def tail(path, lines):
    with open(path, 'rb') as f:
        return b''.join(deque(f, maxlen=lines)).decode(errors='replace')


class Server(HTTPServer):
    # Checked on accept, before the request is read: another host cannot hold the one connection slot.
    def verify_request(self, request, client_address):
        if client_address[0] in ALLOW:
            return True
        sys.stderr.write(f'rejected {client_address[0]}\n')
        return False


class Handler(BaseHTTPRequestHandler):
    timeout = 60  # a stalled upload must not block the next deploy

    def do_POST(self):
        unit = self.path.strip('/')
        if not UNIT.fullmatch(unit):
            return self.reply(404, 'not a unit name\n')
        length = int(self.headers.get('Content-Length') or 0)
        if length <= 0:
            return self.reply(411, 'send the archive with --data-binary @-\n')
        with tempfile.NamedTemporaryFile(suffix='.tar.gz') as f:
            while length:
                chunk = self.rfile.read(min(length, 1 << 20))
                if not chunk:
                    return self.reply(400, 'archive cut short\n')
                f.write(chunk)
                length -= len(chunk)
            f.flush()
            r = subprocess.run(['dpl', 'deploy', unit, f.name], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.log_message('deploy %s: exit %d', unit, r.returncode)
        out = r.stdout
        if m := LOG_LINE.search(out):
            name = os.path.basename(m[1])
            try:
                log = f'\n--- {name}, tail -n 100; more: GET /{unit}/{name}?lines=N ---\n' + tail(m[1], 100)
            except OSError as e:
                log = f'\nread {name}: {e.strerror}\n'
            out = out[:m.start()] + log
        self.reply(200 if r.returncode == 0 else 500, out)

    def do_GET(self):
        url = urlsplit(self.path)
        unit, _, name = url.path.strip('/').partition('/')
        if not UNIT.fullmatch(unit) or name not in LOGS:
            return self.reply(404, 'GET /<unit>/{build,runtime,timers,access}.log?lines=N\n')
        try:
            lines = int(parse_qs(url.query).get('lines', ['1000'])[0])
        except ValueError:
            lines = 0
        if lines < 1:
            return self.reply(400, 'lines must be a positive number\n')
        try:
            text = tail(os.path.join(BASE, 'state', unit, 'log', name), lines)
        except FileNotFoundError:
            return self.reply(404, f'no {name} for {unit}\n')
        except OSError as e:
            return self.reply(500, f'read {name}: {e.strerror}\n')
        self.reply(200, text)

    def reply(self, code, text):
        body = text.encode()
        self.send_response(code)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


signal.signal(signal.SIGTERM, lambda *_: sys.exit())  # unwind: kill the running deploy, drop its temp archive
# ponytail: one request at a time, so deploys never overlap; a second curl waits for the build in progress.
Server(('', PORT), Handler).serve_forever()
