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
