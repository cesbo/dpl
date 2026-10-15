#!/usr/bin/env python3
"""Deploy a dpl unit over HTTP.

    git archive --format tar.gz HEAD | curl -N --data-binary @- http://<server>:8099/<unit>

Accepts requests only from the IPs in DEPLOY_ALLOW (space-separated), saves the
archive to a temp file and runs `dpl deploy <unit> <file>`. dpl's step lines are
forwarded as they are written, so the client is not waiting on a silent
connection. The status is 200 once dpl has started: it has to be sent before the
deploy finishes, so a failed deploy is not an HTTP error. The last line is
`dpl-http: exit N` (dpl's exit code; 0 is success). On failure the `Log: <path>`
line is replaced with the tail of that log: the client cannot read server files.

    curl -N -X POST 'http://<server>:8099/<unit>/rollback?version=N'

POST to /<unit>/rollback runs `dpl rollback <unit> [N]` (no body; `version`
optional) and answers like a deploy.

    curl 'http://<server>:8099/<unit>/build.log?lines=1000'

GET returns the last `lines` (default 1000) of {DPL_BASE}/state/<unit>/log/<name>
for build, runtime, timers and access logs.
"""
import os
import re
import select
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
VERSION = re.compile(r'[1-9][0-9]*')  # ASCII only: int() would take digits clap rejects
RUNTIME_V = re.compile(rf'runtime-v{VERSION.pattern}\.log')  # an app version's own log, as `Log:` names it
LOG_LINE = re.compile(r'^ *Log: (\S+)\s*\Z', re.M)  # last line of a failed `dpl deploy`/`dpl rollback`


def tail(path, lines):
    with open(path, 'rb') as f:
        return b''.join(deque(f, maxlen=lines)).decode(errors='replace')


def log_path_of(line):
    """Path named by a `Log:` line, or None. `line` has no trailing newline."""
    try:
        text = line.decode()
    except UnicodeDecodeError:
        return None
    m = LOG_LINE.fullmatch(text)
    return m[1] if m else None


def is_log_prefix(fragment):
    """True if `fragment` could still grow into a `Log:` line.

    Anything else is forwarded at once. Phase lines have no newline until the
    step finishes; holding them would hide the step in progress.
    """
    try:
        text = fragment.decode()
    except UnicodeDecodeError:
        return fragment.decode(errors='ignore').lstrip(' ').startswith('Log:')
    rest = text.lstrip(' ')
    if 'Log: '.startswith(rest):
        return True  # spaces, or a cut-short "Log: "
    if not rest.startswith('Log:'):
        return False
    path = rest[4:]
    if path.startswith(' '):
        path = path[1:]
    elif path:
        return False  # "Log:x" is not "Log: <path>"
    return re.fullmatch(r'\S*\s*', path) is not None


class DeployOutput:
    """Forward dpl's output as it arrives, holding back the server path in `Log:`."""

    def __init__(self, write):
        self.write = write
        self.held = b''
        self.log_path = None

    def feed(self, chunk):
        data = self.held + chunk
        self.held = b''
        while True:
            nl = data.find(b'\n')
            if nl < 0:
                break
            line, data = data[:nl], data[nl + 1:]
            if path := log_path_of(line):
                self.log_path = path
            else:
                self.write(line + b'\n')
        if data and is_log_prefix(data):
            self.held = data
        elif data:
            self.write(data)

    def finish(self):
        if not self.held:
            return
        if path := log_path_of(self.held):
            self.log_path = path
        else:
            self.write(self.held)
        self.held = b''


class Server(HTTPServer):
    # Checked on accept, before the request is read: another host cannot hold the one connection slot.
    def verify_request(self, request, client_address):
        if client_address[0] in ALLOW:
            return True
        sys.stderr.write(f'rejected {client_address[0]}\n')
        return False


class Handler(BaseHTTPRequestHandler):
    timeout = 60  # a stalled upload must not block the next deploy
    disable_nagle_algorithm = True  # a phase line is a few bytes; Nagle would sit on it

    def do_POST(self):
        url = urlsplit(self.path)
        unit, _, action = url.path.strip('/').partition('/')
        if not UNIT.fullmatch(unit) or action not in ('', 'rollback'):
            return self.reply(404, 'POST /<unit> with the archive, or /<unit>/rollback?version=N\n')
        if action == 'rollback':  # no body: the version comes from the query
            version = parse_qs(url.query, keep_blank_values=True).get('version', [None])[0]
            if version is not None and not VERSION.fullmatch(version):
                return self.reply(400, 'version must be a positive number\n')
            return self.run('rollback', unit, *([version] if version else []))
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
            self.run('deploy', unit, f.name)

    def run(self, command, unit, *args):
        try:
            proc = subprocess.Popen(
                ['dpl', command, unit, *args],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                bufsize=0,
            )
        except OSError as e:
            return self.reply(500, f'{command} {unit}: {e.strerror}\n')
        # Status goes out before dpl exits, so it cannot be 500 on a failed deploy.
        # The body is not Content-Length-delimited: HTTP/1.0 ends it when we close.
        self.gone = False
        self.at_nl = True
        out = DeployOutput(self.write_stream)
        try:
            self.start_stream()
            self.drain(proc, out)
        finally:
            self.drain(proc, out)  # a dropped client must not fill the pipe and stall dpl
            rc = proc.wait()
            self.log_message('%s %s: exit %d', command, unit, rc)
        out.finish()
        if out.log_path:
            self.write_stream(self.log_note(unit, out.log_path))
        if not self.at_nl:
            self.write_stream(b'\n')
        self.write_stream(f'dpl-http: exit {rc}\n'.encode())

    def drain(self, proc, out):
        fd = proc.stdout.fileno()
        while True:
            try:
                readable, _, _ = select.select([fd], [], [])
            except (ValueError, OSError):
                return
            if not readable:
                continue
            try:
                chunk = os.read(fd, 1 << 16)
            except OSError:
                return
            if not chunk:
                return
            out.feed(chunk)

    def start_stream(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('X-Accel-Buffering', 'no')  # a proxy must not hold the steps back
        self.end_headers()

    def write_stream(self, data):
        if not data or self.gone:
            return
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionError, TimeoutError):
            self.gone = True
            return
        self.at_nl = data.endswith(b'\n')

    def log_note(self, unit, path):
        name = os.path.basename(path)
        try:
            body = tail(path, 100)
        except OSError as e:
            return f'\nread {name}: {e.strerror}\n'.encode()
        note = f'\n--- {name}, tail -n 100; more: GET /{unit}/{name}?lines=N ---\n'
        return note.encode() + body.encode()

    def do_GET(self):
        url = urlsplit(self.path)
        unit, _, name = url.path.strip('/').partition('/')
        if not UNIT.fullmatch(unit) or (name not in LOGS and not RUNTIME_V.fullmatch(name)):
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


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit())  # unwind: kill the running deploy, drop its temp archive
    # ponytail: one request at a time, so deploys never overlap; a second curl waits for the build in progress.
    Server(('', PORT), Handler).serve_forever()


if __name__ == '__main__':
    main()
