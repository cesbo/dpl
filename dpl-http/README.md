# dpl-http

A small HTTP server that runs `dpl deploy` on the server. A CI job or an AI
agent sends the project archive with `curl` and gets the deploy result back.
It can also read unit logs, so the client does not need SSH access.

It is one Python script with no dependencies. It runs as a systemd service.

## Requirements

- `dpl` installed with `install.sh`, and `dpl serve` running
- `python3`

## Install

Run as root on the server:

```sh
R=https://raw.githubusercontent.com/cesbo/dpl/master/dpl-http
curl -fsSL -o /opt/dpl/dpl-http.py "$R/dpl-http.py"
curl -fsSL -o /etc/systemd/system/dpl-http.service "$R/dpl-http.service"
curl -fsSL -o /opt/dpl/dpl-http.yaml.example "$R/dpl-http.yaml.example"
cp /opt/dpl/dpl-http.yaml.example /opt/dpl/dpl-http.yaml
chmod 600 /opt/dpl/dpl-http.yaml
```

Edit `/opt/dpl/dpl-http.yaml` (see [Config](#config)), then start the service:

```sh
systemctl daemon-reload
systemctl enable --now dpl-http
```

Check that it runs:

```sh
systemctl status dpl-http
journalctl -u dpl-http
```

If a firewall is on, open the port for the clients that should connect, for
example an allow-listed address:

```sh
ufw allow from 203.0.113.10 to any port 8099 proto tcp
```

A token does not open the port. If the client is not in `allow`, the firewall
has to allow it too.

## Config

Settings live in `dpl-http.yaml`, in the same directory as the script
(`/opt/dpl/dpl-http.yaml`). The service does not read `DEPLOY_ALLOW`,
`DEPLOY_TOKEN`, `DEPLOY_PORT`, or `DPL_BASE`. If a systemd override still sets
them, they are ignored and a line is written to the journal. Delete those
lines (`systemctl edit dpl-http`).

Do not edit `dpl-http.service` for settings. An update replaces that file and
does not touch `dpl-http.yaml`.

```yaml
allow:
  - 203.0.113.10
  - 198.51.100.7

tokens:
  ci:
    token: replace-with-a-long-random-string
  laptop:
    token: another-long-random-string
```

Set at least one of `allow` and `tokens`, or the service does not start.
Replace the token before starting. The example value is not a secret.

A request is accepted when the IP is listed or the token matches. One is
enough; they are not combined. The traffic is plain HTTP, so a token is
visible on the wire. Allow only hosts you trust, or treat the token as the
only secret.

Restart after a change:

```sh
systemctl restart dpl-http
```

| Key      | Default    | Meaning                               |
|----------|------------|---------------------------------------|
| `allow`  | (none)     | Client IPs accepted with no token     |
| `tokens` | (none)     | Bearer tokens accepted from any IP    |
| `port`   | `8099`     | Port to listen on                     |
| `base`   | `/opt/dpl` | dpl base directory, if you changed it |

`base` is passed to `dpl` as `DPL_BASE`, so a deploy and a log read use the
same tree. It must be an absolute path.

`tokens` is a map of names, not a list of strings. The name (`ci`, `laptop`)
is a label in this file. The client sends `token`, not the name. A later
version will read an optional `projects` list on a token — unit names, as in
the URL — and limit that token to those units. Leave the key out until then:
it is rejected, so a limit cannot be set and silently ignored. Once it is
read, omitting it will mean every unit.

```yaml
tokens:
  ci:
    token: replace-with-a-long-random-string
    # projects:        # not accepted yet
    #   - myapp
    #   - api
```

Write lists as lines under the key, not `[a, b]`. A tab is an error.

## Allowed IP addresses

A client whose IP is in `allow` is accepted with no token. `allow` may be
omitted when `tokens` is set.

When no token is set, other connections are closed before the request is read.
When a token is set, a client that is not listed can still connect: the token
is checked instead (see [Token](#token)).

Rules:

- One address per line.
- Exact IPv4. Ranges (CIDR) and host names do not work.

## Token

A request that sends one of the `token` values is accepted from any IP:

```
Authorization: Bearer <token>
```

A listed IP is accepted even without this header, and even if the token is
wrong. Omit `tokens` to keep IP checks only. An update of the script does not
start requiring a token, and does not accept one, until you set it.

Rules:

- Printable ASCII only, no spaces. Generate one with `uuidgen`.
- A `#` starts a comment when a space precedes it. Quote the token if it
  contains `#`.
- Any one of the tokens is accepted. Use more than one so a CI job and a
  laptop can have different tokens, and you can drop one without locking the
  other out.
- The token is not written to the journal. It is still visible on the wire:
  this is plain HTTP.

A client that is not listed, and that sends a missing or wrong token, is
rejected with `401` before the archive is read, so it does not block the next
deploy.

## Usage

Deploy a unit:

```sh
git archive --format tar.gz HEAD | curl -N -H "Authorization: Bearer $TOKEN" --data-binary @- http://<server>:8099/<unit>
```

Omit `-H` when the client IP is in `allow`.

The response is the `dpl deploy` output, sent as each step is written rather
than after the deploy finishes. `-N` keeps curl from buffering that stream
when its output is not a terminal.

The status is `200` once `dpl` has started. It is sent before the result is
known, so a failed deploy is not an HTTP error.
The last line is `dpl-http: ok` or `dpl-http: failed`. On failure the response
also has the last 100 lines of the build log
(or the runtime log, if the app did not start).

Roll back a unit:

```sh
curl -N -H "Authorization: Bearer $TOKEN" -X POST http://<server>:8099/<unit>/rollback
curl -N -H "Authorization: Bearer $TOKEN" -X POST 'http://<server>:8099/<unit>/rollback?version=N'
```

Runs `dpl rollback <unit> [N]`: the previous retained version (or version `N`)
is deployed again as a new version. No body is sent. The response is handled
like a deploy.

Read a unit log:

```sh
curl -H "Authorization: Bearer $TOKEN" 'http://<server>:8099/<unit>/build.log?lines=1000'
```

- Logs: `build.log`, `runtime.log`, `timers.log`, `access.log`. For an app,
  `runtime.log` is the log of the version in service and `runtime-v<N>.log`
  one version's own (what a failed deploy's `Log:` line names).
- `lines` is the number of last lines to return. The default is `1000`.

The server handles one request at a time. A second request waits until the
current deploy is done.

## Update

Download the script and the service file again (see [Install](#install)), not
`dpl-http.yaml`. If this host still sets `DEPLOY_ALLOW` or `DEPLOY_TOKEN`,
create the yaml first (see [Config](#config)): the service does not start
without it. Then:

```sh
systemctl daemon-reload
systemctl restart dpl-http
```

A restart stops a deploy that is in progress. Run the deploy again after the
restart.
