#!/usr/bin/env python3
"""Deploy a dpl unit over HTTP.

    git archive --format tar.gz HEAD | curl --fail-with-body --data-binary @- http://<server>:8099/<unit>

Accepts requests only from the IPs in DEPLOY_ALLOW (space-separated), saves the
archive to a temp file and runs `dpl deploy <unit> <file>`. The response is the
dpl output: status 200 on success, 500 on failure.
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
        self.reply(200 if r.returncode == 0 else 500, r.stdout)

    def reply(self, code, text):
        body = text.encode()
        self.send_response(code)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


# ponytail: one request at a time, so deploys never overlap; a second curl waits for the build in progress.
Server(('', PORT), Handler).serve_forever()
