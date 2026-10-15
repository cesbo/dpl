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

Set the allowed IP addresses (see the next section), then start the service:

```sh
systemctl daemon-reload
systemctl enable --now dpl-http
```

Check that it runs:

```sh
systemctl status dpl-http
journalctl -u dpl-http
```

If a firewall is on, open the port for the allowed addresses, for example:

```sh
ufw allow from 203.0.113.10 to any port 8099 proto tcp
```

## Allowed IP addresses

Only clients from `DEPLOY_ALLOW` can connect. Other connections are closed
before the request is read. The setting is required: without it the service
does not start.

Do not edit `dpl-http.service` for this. Use a systemd override, so an update
of the service file does not remove your settings:

```sh
systemctl edit dpl-http
```

Add these lines and save:

```ini
[Service]
Environment="DEPLOY_ALLOW=203.0.113.10 198.51.100.7"
```

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

| Variable       | Default    | Meaning                                  |
|----------------|------------|------------------------------------------|
| `DEPLOY_ALLOW` | (required) | Allowed client IP addresses              |
| `DEPLOY_PORT`  | `8099`     | Port to listen on                        |
| `DPL_BASE`     | `/opt/dpl` | dpl base directory, if you changed it    |

The IP list is the only access control, and the traffic is plain HTTP.
Allow only hosts you trust.

## Usage

Deploy a unit:

```sh
git archive --format tar.gz HEAD | curl -N --data-binary @- http://<server>:8099/<unit>
```

The response is the `dpl deploy` output, sent as each step is written rather
than after the deploy finishes. `-N` keeps curl from buffering that stream
when its output is not a terminal.

The status is `200` once `dpl` has started. It is sent before the result is
known, so a failed deploy is not an HTTP error.
The last line is `dpl-http: exit N` — `N` is `dpl`'s exit code, and `0` is
success. On failure the response also has the last 100 lines of the build log
(or the runtime log, if the app did not start).

Roll back a unit:

```sh
curl -N -X POST http://<server>:8099/<unit>/rollback
curl -N -X POST 'http://<server>:8099/<unit>/rollback?version=N'
```

Runs `dpl rollback <unit> [N]`: the previous retained version (or version `N`)
is deployed again as a new version. No body is sent. The response is handled
like a deploy.

Read a unit log:

```sh
curl 'http://<server>:8099/<unit>/build.log?lines=1000'
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
