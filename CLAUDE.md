# dpl

## Project Overview

**dpl** is a lightweight web application deployment system written in Go.
It receives application archives via HTTP, builds container images using Podman, and runs them as systemd services on a Linux server.

### Entity Model

All managed entities live in `/opt/dpl/<name>/config.yaml`.
Each config has a `type` field that determines the entity kind:

- **app** — web application running in a Podman container (build, run, systemd service).
- **domain** — domain configuration (nginx, TLS). *Future.*
- **static** — static file site with its own deploy logic. *Future.*
- **database** — managed database service. *Future.*

Each entity type is a separate Go package under `internal/entities/` (`internal/entities/app`, `internal/entities/domain`, etc.) with its own config struct, templates, and deploy logic.

Examples:
```
/opt/dpl/example.com/config.yaml   # type: domain
/opt/dpl/landing/config.yaml        # type: static
/opt/dpl/profile/config.yaml        # type: app
/opt/dpl/profile-db/config.yaml     # type: database
```

### Core Workflow (app)

1. GitHub Actions creates a tag → builds a tar.gz archive of the web application.
2. The archive is POSTed to `http://server:PORT/deploy/app-name` with Bearer token auth.
3. dpl reads entity config from `/opt/dpl/app-name/config.yaml`, checks `type: app`.
4. dpl creates build directory `/opt/dpl/app-name/deploy_{version}/`
5. dpl generates build artifacts in the build directory: `build.sh`, `Containerfile`, `run.sh`, systemd `.service` file.
6. dpl extracts received archive into `/opt/dpl/app-name/deploy_{version}/app/`.
7. dpl builds a Podman image and starts a container via systemd.

### Target Platform

- Linux server with **systemd** and **Podman** (rootful or rootless).
- No Docker dependency. Use Podman CLI and Podman-specific features (e.g., `--sdnotify=conmon`, `--cgroups=split`).

## Language & Style

- **Go** (latest stable, currently 1.24+).
- Use the standard library wherever possible. Minimize external dependencies.
- Follow idiomatic Go: short variable names in narrow scopes, exported names with doc comments, error wrapping with `fmt.Errorf("context: %w", err)`.
- Use `slog` for structured logging.
- Use `errors.Is` / `errors.As` for error inspection.
- Prefer returning errors over panicking.

## Project Structure

```
cmd/
  dpl/              # main package — CLI entry point
internal/
  entities/         # deploy entity packages (one per entity type)
    app/            # app entity: config, templates, generation, deploy pipeline
  server/           # HTTP server, deploy handler, auth middleware
  podman/           # podman build & run commands
  systemd/          # systemd unit management (enable, start, stop, restart)
```

- `cmd/dpl/main.go` — entry point starting the HTTP server.
- `internal/` — all internal packages, not importable from outside.
- `internal/entities/` — deploy entity packages. Each entity type (`app`, `domain`, `static`, `database`) gets its own package here with its own config struct, templates, and deploy logic.
- Shared infrastructure (`podman`, `systemd`, `server`, `base`) lives in `internal/` and is used by entity packages.

## Key Design Decisions

### Configuration

- All entities live in `/opt/dpl/<name>/`.
- Every `config.yaml` has a `type` field (`app`, `domain`, `static`, `database`).
- Config is parsed with `gopkg.in/yaml.v3`.
- Config struct fields use `yaml:"..."` tags.
- Server-level settings (listening address) are passed as environment variables (`DPL_ADDR`) set in the dpl systemd service file.
- Auth tokens are per-entity: each entity's `config.yaml` contains a `tokens` array. This allows different tokens for different entities and users.

### Dispatch by Type

- The HTTP handler reads the `type` field from `config.yaml` and dispatches to the corresponding entity package (`entities/app.Deploy()`, `entities/static.Deploy()`, etc.).
- Each entity package implements its own deploy logic independently.

### Template Rendering

- Each entity package embeds its own templates via `embed.FS`.
- Use Go `text/template` for generating scripts and config files.
- Build/runtime env vars use heredoc syntax with a random UUID delimiter per variable to safely handle multiline values.

### HTTP API

- `POST /deploy/<name>` — accepts `multipart/form-data` or raw body with the tar.gz archive.
- Bearer token authentication via `Authorization` header, validated against the `tokens` array in the entity's `config.yaml`.
- Return meaningful HTTP status codes: 401 (bad token), 404 (unknown entity), 500 (build/deploy errors).
- Stream build output back in the response body where practical.

### Container Build (app)

- Build context is assembled in a temp directory.
- `build.sh` is mounted as a secret during build (`--secret id=build-sh`), not baked into the image.
- The final image is tagged as `localhost/<name>:<timestamp>`.

### systemd Integration (app)

- Each app container is a systemd service: `dpl-<name>.service`.
- Service uses `Type=notify` with Podman's conmon sdnotify.
- Port mapping: `127.0.0.1:<random-host-port>:<container-port>`.

### Networking / Reverse Proxy

- Containers bind to `127.0.0.1` only.
- nginx config generation is planned (via `domain` entity).
- TLS is handled externally for now.

## Testing

- Unit tests with `testing` package, table-driven style.
- Use `t.TempDir()` for file system tests.
- Integration tests that call Podman should be guarded with a build tag `//go:build integration`.
- Test file generation by comparing output against golden files in `testdata/`.

## File Naming Conventions

- Go files: `snake_case.go`
- Templates: `<name>.tmpl` (e.g., `containerfile.tmpl`, `build_sh.tmpl`)
- Test files: `*_test.go`
- Golden/test data: `testdata/` directories within each package

## Important Notes

- Never use Docker. All container operations use `podman` CLI.
- The heredoc delimiter for env vars must be unique per variable (UUID v4).
- `build.sh` runs inside the container at build time via `--mount=type=secret`.
- `run.sh` is copied into the image and runs at container start.
- Port allocation for host-side mapping should pick a random free port and persist it in the service file.

## Workflow

When work on a phase from `TODO.md` is finished:

1. Mark all phase items as done (`[x]`).
2. `git add -A && git commit` with a message like `phase N: short description`.
