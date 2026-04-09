# dpl

## Runtime configuration

`DPL_BASE` defines the working directory.
If it is not set, `dpl` uses `/opt/dpl`.

The application config file is loaded from:

```text
{DPL_BASE}/config.yaml
```

If the file is missing, the server starts with defaults.

Config example:

```yaml
server:
	addr: 0.0.0.0
	port: 3000
```

## Authorization

All routes under `/deploy` require Bearer authorization from:

```text
{DPL_BASE}/auth.yaml
```

Stage 1 keeps tokens in plain text. Example:

```yaml
keys:
	- id: deploy-key
		token: open-token
		apps: [frontend]
		scopes: [deploy, status]
		disabled: false
```

Current scope mapping:

- `POST /deploy/{name}` -> `deploy`
- `GET /deploy/{name}/state` -> `status`

If `auth.yaml` is missing or unreadable, deploy routes fail closed.
