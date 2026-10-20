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
```

Set an IP allow list, a deploy token, or both (see the next sections), then
start the service:

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

A token does not open the port. If the client is not in `DEPLOY_ALLOW`, the
firewall has to allow it too.

## Allowed IP addresses

A client whose IP is in `DEPLOY_ALLOW` is accepted with no token. The variable
may be left unset when `DEPLOY_TOKEN` is set. If neither is set the service
does not start.

When no token is set, other connections are closed before the request is read.
When a token is set, a client that is not listed can still connect: the token
is checked instead (see [Token](#token)).

Do not edit `dpl-http.service` for this. Use a systemd override, so an update
of the service file does not remove your settings:

```sh
systemctl edit dpl-http
```

Add these lines and save:

```ini
[Service]
Environment="DEPLOY_ALLOW=203.0.113.10 198.51.100.7"
Environment="DEPLOY_TOKEN=replace-with-a-long-random-string"
```

Set at least one of `DEPLOY_ALLOW` and `DEPLOY_TOKEN`. Several tokens go in
the same variable, separated by spaces (`token-for-ci token-for-laptop`). See
[Token](#token).

Then restart the service:

```sh
systemctl restart dpl-http
```

Rules:

- Separate addresses with spaces.
- Use exact IPv4 addresses. Ranges (CIDR) and host names do not work.
- The override is saved in
  `/etc/systemd/system/dpl-http.service.d/override.conf`.

Other settings go in the same override:

| Variable       | Default    | Meaning                                      |
|----------------|------------|----------------------------------------------|
| `DEPLOY_ALLOW` | (none)     | Client IPs accepted with no token            |
| `DEPLOY_TOKEN` | (none)     | Bearer tokens accepted from any IP           |
| `DEPLOY_PORT`  | `8099`     | Port to listen on                            |
| `DPL_BASE`     | `/opt/dpl` | dpl base directory, if you changed it        |

A request is accepted when the IP is listed or the token matches. One is
enough; they are not combined. The traffic is plain HTTP, so a token is
visible on the wire. Allow only hosts you trust, or treat the token as the
only secret.

## Token

A request that sends one of the `DEPLOY_TOKEN` values is accepted from any IP:

```
Authorization: Bearer <token>
```

A listed IP is accepted even without this header, and even if the token is
wrong. Leave the variable unset to keep IP checks only. An update of the
script does not start requiring a token, and does not accept one, until you
set it.

Rules:

- Separate tokens with spaces. A token itself cannot contain spaces.
- Printable ASCII only. Generate one with `uuidgen`.
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

Omit `-H` when the client IP is in `DEPLOY_ALLOW`.

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

Download the files again (see [Install](#install)), then:

```sh
systemctl daemon-reload
systemctl restart dpl-http
```

A restart stops a deploy that is in progress. Run the deploy again after the
restart.
