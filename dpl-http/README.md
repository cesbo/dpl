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

If a firewall is on, open the port. dpl-http does not check the client IP; a
firewall rule is the only address limit:

```sh
ufw allow from 203.0.113.10 to any port 8099 proto tcp
```

A token does not open the port. The firewall has to allow the client too.

## Config

Settings live in `dpl-http.yaml`, in the same directory as the script
(`/opt/dpl/dpl-http.yaml`). The service does not read `DEPLOY_ALLOW`,
`DEPLOY_TOKEN`, `DEPLOY_PORT`, or `DPL_BASE`. If a systemd override still sets
them, they are ignored and a line is written to the journal. Delete those
lines (`systemctl edit dpl-http`).

Do not edit `dpl-http.service` for settings. An update replaces that file and
does not touch `dpl-http.yaml`.

```yaml
tokens:
  ci:
    token: replace-with-a-long-random-string
  laptop:
    token: another-long-random-string
```

`tokens` is required, or the service does not start. Replace the token before
starting. The example value is not a secret.

A request is accepted only when the token matches. The traffic is plain HTTP,
so a token is visible on the wire. Allow only hosts you trust, or treat the
token as the only secret.

Restart after a change:

```sh
systemctl restart dpl-http
```

| Key      | Default    | Meaning                               |
|----------|------------|---------------------------------------|
| `tokens` | (required) | Bearer tokens; a request needs one    |
| `port`   | `8099`     | Port to listen on                     |
| `base`   | `/opt/dpl` | dpl base directory, if you changed it |

`base` is passed to `dpl` as `DPL_BASE`, so a deploy and a log read use the
same tree. It must be an absolute path.

`tokens` is a map of names, not a list of strings. The name (`ci`, `laptop`)
is a label in this file. The client sends `token`, not the name.

Omit `units` to let that token deploy, roll back, and read logs of every
unit. Set it to limit the token to those unit names (the `<unit>` in the
URL). A request for another unit is `403` before the archive is read.

```yaml
tokens:
  ci:
    token: replace-with-a-long-random-string
    units:
      - myapp
      - api
  laptop:
    token: another-long-random-string
```

Write lists as lines under the key, not `[a, b]`. A tab is an error. An
`allow` key is rejected: the client IP is not checked.

## Token

A request is accepted only when it sends one of the `token` values:

```
Authorization: Bearer <token>
```

The client IP is not checked. An update of the script does not accept a token
until you set one, and does not start without one.

Rules:

- Printable ASCII only, no spaces. Generate one with `uuidgen`.
- A `#` starts a comment when a space precedes it. Quote the token if it
  contains `#`.
- Any one of the tokens is accepted. Use more than one so a CI job and a
  laptop can have different tokens, and you can drop one without locking the
  other out.
- The token is not written to the journal. It is still visible on the wire:
  this is plain HTTP.

A missing or wrong token is rejected with `401` before the archive is read,
so it does not block the next deploy. A token that may not touch the unit in
the URL is rejected with `403` the same way. The reply names the token label,
not the secret.

## Usage

Deploy a unit:

```sh
git archive --format tar.gz HEAD | curl -N -H "Authorization: Bearer $TOKEN" --data-binary @- http://<server>:8099/<unit>
```

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
create the yaml first (see [Config](#config)). `DEPLOY_ALLOW` is not read:
set a token. The service does not start without one. Then:

```sh
systemctl daemon-reload
systemctl restart dpl-http
```

A restart stops a deploy that is in progress. Run the deploy again after the
restart.
