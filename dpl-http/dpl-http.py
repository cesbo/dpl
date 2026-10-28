#!/usr/bin/env python3
"""Deploy a dpl unit over HTTP.

    git archive --format tar.gz HEAD | curl -N -H "Authorization: Bearer $TOKEN" --data-binary @- http://<server>:8099/<unit>

Settings are read from dpl-http.yaml in the same directory as this script.
A request is accepted when it sends one of the `tokens` values as
`Authorization: Bearer`. At least one token must be set, or the service does
not start. A missing or wrong token is rejected before the body is read.
Saves the archive to a temp file and runs `dpl deploy <unit> <file>`. dpl's
step lines are forwarded as they are written, so the client is not waiting on
a silent connection. The status is 200 once dpl has started: it has to be sent
before the deploy finishes, so a failed deploy is not an HTTP error. The last
line is `dpl-http: ok` or `dpl-http: failed`. On failure the `Log: <path>`
line is replaced with the tail of that log: the client cannot read server
files.

    curl -N -H "Authorization: Bearer $TOKEN" -X POST 'http://<server>:8099/<unit>/rollback?version=N'

POST to /<unit>/rollback runs `dpl rollback <unit> [N]` (no body; `version`
optional) and answers like a deploy.

    curl -H "Authorization: Bearer $TOKEN" 'http://<server>:8099/<unit>/build.log?lines=1000'

GET returns the last `lines` (default 1000) of {base}/state/<unit>/log/<name>
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

PORT = 8099
BASE = '/opt/dpl'
LOGS = {'build.log', 'runtime.log', 'timers.log', 'access.log'}  # files in {base}/state/{unit}/log/
UNIT = re.compile(r'[a-z0-9]+(-[a-z0-9]+)*')  # dpl unit names
VERSION = re.compile(r'[1-9][0-9]*')  # ASCII only: int() would take digits clap rejects
RUNTIME_V = re.compile(rf'runtime-v{VERSION.pattern}\.log')  # an app version's own log, as `Log:` names it
LOG_LINE = re.compile(r'^ *Log: (\S+)\s*\Z', re.M)  # last line of a failed `dpl deploy`/`dpl rollback`
TOKEN_NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]*')
# Filled from dpl-http.yaml before serve. A request needs one of these.
TOKENS = ()
IGNORED_ENV = ('DEPLOY_ALLOW', 'DEPLOY_TOKEN', 'DEPLOY_PORT', 'DPL_BASE')


def config_path():
    """dpl-http.yaml next to this script, not the process cwd."""
    return os.path.join(os.path.dirname(os.path.realpath(__file__)), 'dpl-http.yaml')


def fail(path, line, message):
    where = f'{path}:{line}' if line else path
    sys.exit(f'{where}: {message}')


def cut_unquoted(line, pred):
    """Index of the first unquoted char where pred(line, i) holds.

    None if a quote is unclosed. -1 if pred never matches.
    """
    quote = None
    i = 0
    while i < len(line):
        c = line[i]
        if quote:
            if quote == "'" and line.startswith("''", i):
                i += 2
                continue
            if quote == '"' and c == '\\':
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in '"\'':
            quote = c
            i += 1
            continue
        if pred(line, i):
            return i
        i += 1
    if quote:
        return None
    return -1


def strip_comment(line):
    """Drop a `#` comment that is not inside quotes. None if a quote is unclosed."""
    i = cut_unquoted(line, lambda s, j: s[j] == '#' and (j == 0 or s[j - 1].isspace()))
    if i is None:
        return None
    return line if i < 0 else line[:i]


def unquote(value, path, line):
    """Scalar. Quoted strings keep their contents; anything else is literal."""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in '"\'':
        body = value[1:-1]
        if value[0] == "'":
            if "'" in body.replace("''", ''):
                fail(path, line, 'unescaped quote in string')
            return body.replace("''", "'")
        out = []
        i = 0
        while i < len(body):
            if body[i] == '\\':
                if i + 1 >= len(body):
                    fail(path, line, 'trailing backslash in string')
                n = body[i + 1]
                if n == 'n':
                    out.append('\n')
                elif n == 't':
                    out.append('\t')
                elif n in '\\"':
                    out.append(n)
                else:
                    fail(path, line, f'unknown escape \\{n}')
                i += 2
                continue
            if body[i] == '"':
                fail(path, line, 'unescaped quote in string')
            out.append(body[i])
            i += 1
        return ''.join(out)
    if value and value[0] in '[{*&!|>':
        fail(path, line, 'use a block list or map; inline and multiline values are not supported')
    return value


def split_key(text, path, line):
    """`key: value`. The first colon ends the key; the value may contain colons."""
    i = cut_unquoted(text, lambda s, j: s[j] == ':')
    if i is None:
        fail(path, line, 'unclosed quote')
    if i < 0:
        fail(path, line, 'expected key: value')
    key = unquote(text[:i].strip(), path, line)
    if not key:
        fail(path, line, 'empty key')
    rest = text[i + 1:]
    if rest.startswith(' '):
        rest = rest[1:]
    return key, rest.strip()


def yaml_lines(text, path):
    items = []
    for n, raw in enumerate(text.splitlines(), 1):
        if '\t' in raw:
            fail(path, n, 'tab in indentation; use spaces')
        stripped = strip_comment(raw)
        if stripped is None:
            fail(path, n, 'unclosed quote')
        stripped = stripped.rstrip()
        if not stripped.strip():
            continue
        indent = len(stripped) - len(stripped.lstrip(' '))
        items.append((n, indent, stripped.strip()))
    if items and items[0][2] == '---':
        items = items[1:]
    return items


def parse_yaml(text, path):
    """Block maps and scalar lists only, so the script stays dependency-free."""
    items = yaml_lines(text, path)
    if not items:
        return {}
    n, indent, text = items[0]
    if indent != 0:
        fail(path, n, 'top level must not be indented')
    if text.startswith('- ') or text == '-':
        fail(path, n, 'config must be a map')
    value, i = parse_map(items, 0, path, 0)
    if i != len(items):
        fail(path, items[i][0], 'bad indent')
    return value


def parse_map(items, i, path, indent):
    mapping = {}
    while i < len(items):
        n, got, text = items[i]
        if got < indent:
            break
        if got > indent:
            fail(path, n, 'bad indent')
        if text == '---':
            fail(path, n, 'multiple documents are not supported')
        if text.startswith('- ') or text == '-':
            fail(path, n, 'list item in a map')
        key, rest = split_key(text, path, n)
        if key in mapping:
            fail(path, n, f'duplicate key {key!r}')
        i += 1
        if rest.startswith(('|', '>')):
            fail(path, n, 'multiline strings are not supported')
        if rest:
            mapping[key] = unquote(rest, path, n)
            continue
        nxt = items[i] if i < len(items) else None
        if nxt and (nxt[2].startswith('- ') or nxt[2] == '-') and nxt[1] >= indent:
            mapping[key], i = parse_list(items, i, path, items[i][1])
            continue
        if i < len(items) and items[i][1] > indent:
            mapping[key], i = parse_node(items, i, path, indent)
            if i < len(items) and items[i][1] > indent:
                fail(path, items[i][0], 'bad indent')
            continue
        mapping[key] = None
    return mapping, i


def parse_node(items, i, path, parent_indent):
    n, indent, text = items[i]
    if text.startswith('- ') or text == '-':
        return parse_list(items, i, path, indent)
    if indent <= parent_indent:
        fail(path, n, 'bad indent')
    return parse_map(items, i, path, indent)


def parse_list(items, i, path, indent):
    result = []
    while i < len(items) and items[i][1] == indent:
        n, _, text = items[i]
        if text == '-':
            fail(path, n, 'empty list item')
        if not text.startswith('- '):
            break
        rest = text[2:].strip()
        if not rest:
            fail(path, n, 'empty list item')
        if rest.startswith(('|', '>')):
            fail(path, n, 'multiline strings are not supported')
        i += 1
        if i < len(items) and items[i][1] > indent:
            fail(path, items[i][0], 'nested values under a list item are not supported')
        result.append(unquote(rest, path, n))
    return result, i


def parse_port(value, path):
    if not isinstance(value, str) or not re.fullmatch(r'[1-9][0-9]*', value):
        fail(path, 0, 'port must be a number from 1 to 65535')
    port = int(value)
    if port > 65535:
        fail(path, 0, 'port must be a number from 1 to 65535')
    return port


def parse_base(value, path):
    if not isinstance(value, str) or not os.path.isabs(value):
        fail(path, 0, 'base must be an absolute path')
    return value


def parse_token_map(value, path):
    """Named token entries. `projects` is the later per-unit limit; not read yet.

    A token is a map, not a string, so that key can be added without a new shape.
    Reject it now: a limit that is parsed and ignored would look enforced.
    """
    if not isinstance(value, dict):
        fail(path, 0, 'tokens must be a map of name: {token: ...}')
    found = []
    seen = {}
    for name, body in value.items():
        if not TOKEN_NAME.fullmatch(name):
            fail(path, 0, f'tokens: {name!r} is not a valid name')
        if not isinstance(body, dict):
            fail(path, 0, f'tokens.{name} must be a map with a token key')
        for key in body:
            if key == 'projects':
                # Reserved: a later version limits this token to those units.
                # Rejected now so a limit cannot be set and silently ignored.
                fail(path, 0, f'tokens.{name}: projects is not supported yet; omit it')
            if key != 'token':
                fail(path, 0, f'tokens.{name}: unknown key {key!r}')
        token = body.get('token')
        if (
            not isinstance(token, str)
            or not token
            or not token.isascii()
            or not token.isprintable()
            or any(c.isspace() for c in token)
        ):
            fail(path, 0, f'tokens.{name}: token must be printable ASCII without spaces')
        if token in seen:
            fail(path, 0, f'tokens.{name}: same token as {seen[token]}')
        seen[token] = name
        found.append(token)
    return tuple(found)


def interpret(data, path):
    if not isinstance(data, dict):
        fail(path, 0, 'config must be a map')
    for key in data:
        if key == 'allow':
            fail(path, 0, 'allow is not supported; use tokens')
        if key == 'projects':
            fail(path, 0, 'projects belongs under a token, and is not supported yet')
        if key not in ('port', 'base', 'tokens'):
            fail(path, 0, f'unknown key {key!r}')
    port = 8099 if 'port' not in data else parse_port(data['port'], path)
    base = '/opt/dpl' if 'base' not in data else parse_base(data['base'], path)
    tokens = () if data.get('tokens') is None else parse_token_map(data['tokens'], path)
    if not tokens:
        fail(path, 0, 'set tokens')
    return port, base, tokens


def load_config(path):
    try:
        with open(path, encoding='utf-8-sig') as f:
            text = f.read()
    except FileNotFoundError:
        sys.exit(f'{path}: not found\ncopy dpl-http.yaml.example to {path} and set tokens')
    except UnicodeDecodeError:
        fail(path, 0, 'not utf-8')
    except OSError as e:
        fail(path, 0, e.strerror or str(e))
    cfg = interpret(parse_yaml(text, path), path)
    ignored = [name for name in IGNORED_ENV if name in os.environ]
    if ignored:
        sys.stderr.write(f'ignored {", ".join(ignored)}; settings are in {path}\n')
    return cfg


def bearer(header):
    """Token from `Authorization: Bearer <token>`, or None."""
    scheme, sep, token = header.strip().partition(' ')
    if sep != ' ' or scheme.lower() != 'bearer':
        return None
    token = token.strip()
    if not token or any(c.isspace() for c in token):
        return None
    return token


def token_ok(header):
    """True if `header` carries one configured token. False when none are set."""
    if not TOKENS:
        return False
    presented = bearer(header)
    return presented is not None and presented in TOKENS


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


class Handler(BaseHTTPRequestHandler):
    timeout = 60  # a stalled upload must not block the next deploy
    disable_nagle_algorithm = True  # a phase line is a few bytes; Nagle would sit on it

    def parse_request(self):
        # Before the body is read, so a failed check cannot hold the slot with an upload.
        if not super().parse_request():
            return False
        if token_ok(self.headers.get('Authorization', '')):
            return True
        sys.stderr.write(f'rejected {self.client_address[0]}: unauthorized\n')
        self.close_connection = True
        self.reply(401, 'send Authorization: Bearer <token>\n')
        return False

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
                # yaml base, not the service environment: log reads and dpl must agree
                env={**os.environ, 'DPL_BASE': BASE},
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
        self.write_stream(b'dpl-http: ok\n' if rc == 0 else b'dpl-http: failed\n')

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
    global PORT, BASE, TOKENS
    PORT, BASE, TOKENS = load_config(config_path())
    signal.signal(signal.SIGTERM, lambda *_: sys.exit())  # unwind: kill the running deploy, drop its temp archive
    # ponytail: one request at a time, so deploys never overlap; a second curl waits for the build in progress.
    HTTPServer(('', PORT), Handler).serve_forever()


if __name__ == '__main__':
    main()
