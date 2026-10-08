#!/usr/bin/env python3
"""Deploy a dpl unit over HTTP.

    git archive --format tar.gz HEAD | curl --fail-with-body --data-binary @- http://<server>:8099/<unit>

Accepts requests only from the IPs in DEPLOY_ALLOW (space-separated), saves the
archive to a temp file and runs `dpl deploy <unit> <file>`. The response is the
dpl output: status 200 on success, 500 on failure. On failure the `Log: <path>`
line is replaced with the tail of that log: the client cannot read server files.
"""
import os
import re
import subprocess
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, HTTPServer

ALLOW = set(os.environ['DEPLOY_ALLOW'].split())
PORT = int(os.environ.get('DEPLOY_PORT', '8099'))
UNIT = re.compile(r'[a-z0-9]+(-[a-z0-9]+)*')  # dpl unit names
LOG_LINE = re.compile(r'^ *Log: (\S+)\s*\Z', re.M)  # last line of a failed `dpl deploy`


def log_tail(path, lines=100, limit=64 << 10):
    try:
        with open(path, 'rb') as f:
            f.seek(max(0, f.seek(0, os.SEEK_END) - limit))
            if f.tell():
                f.readline()  # drop the line cut by the seek
            tail = f.read().decode(errors='replace').splitlines()[-lines:]
    except OSError as e:
        return f'\nread {os.path.basename(path)}: {e.strerror}\n'
    return f'\n--- {os.path.basename(path)}, last {len(tail)} lines ---\n' + ''.join(l + '\n' for l in tail)


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
            out = out[:m.start()] + log_tail(m[1])
        self.reply(200 if r.returncode == 0 else 500, out)

    def reply(self, code, text):
        body = text.encode()
        self.send_response(code)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


# ponytail: one request at a time, so deploys never overlap; a second curl waits for the build in progress.
Server(('', PORT), Handler).serve_forever()
