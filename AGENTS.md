# dpl

Single-binary deploy CLI + local supervisor. Manages units (app, db-server, db, http-server, domain, cloudflare-tunnel), encrypted secrets, and one-shot deploys via podman. Runtime: `dpl serve`.

Requires podman. Default base: `/opt/dpl` (`DPL_BASE`; must be non-empty if set).

## Commands

```
cargo build
cargo test
cargo clippy
DPL_BASE=/opt/dpl cargo run -- deploy myapp ./build.tar.gz
```

Never run `cargo fmt`. Edition 2024. Release is a musl Linux static binary (`x86_64` and `aarch64`) plus macOS; see `.github/workflows/deploy.yml`.

Hidden CLI (not for operators): `dpl start <unit>`, `dpl timer <unit> <timer>`.

## Layout

```
src/main.rs          clap; DPL_BASE; DeployError::Reported stays quiet
src/cmd/             CLI handlers (anyhow)
src/config/          names, env templates `${secret:…}` / `${unit:key}`
src/deploy/unit/     per-kind config + deploy (app, cloudflare_tunnel, db, domain, http_server)
src/serve/           daemon: supervisor, timer tick, SIGHUP notify
src/podman/          bounded podman wrappers (do not call podman ad hoc)
src/state.rs         deploy.json + flock deploy.lock
src/secret.rs        AES-GCM master.key + {name}.json
src/reference.rs     typed ref errors (trail innermost-first)
src/context.rs       MainContext path helpers — use these, do not join paths
```

Configs: `{base}/conf/{name}.yaml` (`type:` selects `UnitConfig`). Runtime: `{base}/state/{name}/`. Secrets: `{base}/secrets/`.

## Architecture

Deploys never start containers. They lock `deploy.lock` (whole process lifetime; pid inside), bump version, render MiniJinja artifacts, `podman build`, mark status `check`, SIGHUP `dpl serve`, wait for readiness, then `ready`.

`dpl serve` holds `{base}/state/serve.pid` (exclusive flock). Liveness is from podman (`events` + poll), not child processes — serve can restart without stopping containers. It starts `check` units once via detached `dpl start` (under `systemd-run --scope` when systemd is the parent). `ready` units restart with backoff. `dpl down` stops serve then tears down supervised containers; deploy state stays.

`start_after` (on `deploy.json`) is snapshotted from an app's `${db:…}` refs at hand-off. Serve will not spawn until those db-server containers are up. Domain deploys write nginx conf + www into the http-server unit and reload it; they are not supervised containers.

Container name: `dpl--{unit}` (`UnitName::scoped_unit_name`). Shared network: `dpl`.

App shapes: `runtime.port` → service; `runtime` without port → worker; no `runtime` → static (build+export only).

cloudflare-tunnel is a supervised image-only unit: the token goes into the encrypted runtime env as `TUNNEL_TOKEN`.

## Constraints (do not reopen)

- One static Rust binary, min runtime deps. Target: Podman 4.8+ (netavark).
- Do not migrate to Quadlet, sops/age, Kamal, Coolify/Dokku/CapRover. See [Build vs buy](#build-vs-buy-decided-do-not-reopen).
- Keep the unit/reference model (`type:` + `${…}` + `dpl check`).
- Secrets keep plaintext out of yaml/git, not from root on the host. Runtime env is encrypted to `state/{name}/.env-v{N}.json`; `dpl start` decrypts into the podman process env, never argv.
- Do not delete `deploy.lock` to unstick a deploy; kill the pid it names. Serve never holds it.
- `UnitName`: `[a-z0-9-]+`, no leading/trailing `-`, no `--`. `SecretName`: same segments joined by `/`.
- Unit yaml structs use `#[serde(deny_unknown_fields)]`.
- YAML `type` is kebab-case (`http-server`, `db-server`).
- Loopback binds fail readiness; nginx reaches apps on the `dpl` network. Port probe needs a shell and `/proc`.
- The cloudflared image is distroless (no shell): no port probe, no app-style build; readiness is the worker criterion.
- Control-plane podman is wall-clock bounded (15s metadata, 120s stop/rm, 30min pull/cp, 5s inspect/report). Exceptions: `podman build` and timer scripts.
- Readiness: 90s budget, 5s per probe, 800ms interval. db-server ping: 60s / 10s. Worker: 3 consecutive `running` under the same 90s. Health budget must stay below supervisor `STARTUP_GRACE` (120s).
- `podman` timeouts kill only the podman process, never conmon/container descendants.

## Build vs buy (decided, do not reopen)

dpl deliberately keeps its own supervisor (`dpl serve`), AES-GCM secret store, and CLI shape instead of the obvious alternatives. Do not propose these migrations without the new information listed at the end.

- **Quadlet rejected.** Podman 4.4+ only; the floor is now Podman 4.8+ (2026-09-14), so the version argument is gone and Quadlet is available on every target. It stays rejected on control grounds: Quadlet generates the `.service` for you, which reduces control over the `ExecStart`/`ExecStop` delegation to `dpl start`/`dpl stop`, healthcheck wiring, and custom `Restart=` logic that dpl needs.
- **sops / age rejected for secrets.** For a single-host root tool the threat model is identical: the decryption key lives on the same disk as the ciphertext. sops/age would only add a dependency and editor integration, not real protection. dpl resolves plaintext close to where it is used (runtime env encrypted into `state/{name}/.env-v{N}.json` at deploy; `dpl start` decrypts it into the podman process environment, never argv), so any solution converges on "plaintext reaches the container env" — env vars are always visible to root via `/proc/[pid]/environ`. The ~580 lines of `src/secret.rs` are the price of zero external runtime deps.
- **Kamal rejected.** Requires Ruby + SSH, has no typed cross-unit references (`${app-db:url}`, `${secret:...}`). Different shape, not a drop-in.
- **Coolify / Dokku / CapRover rejected.** Need a server-side component. dpl is explicitly a single static Rust binary.
- **Postgres/MariaDB image env-var init is not a replacement for `sql.rs`.** Official images create one user/db on first boot only. dpl's `db-server` + N×`db` split exists specifically to create multiple databases against one server post-init via `podman exec`.

What *would* reopen a question: wanting to give up the `dpl start`/`dpl stop` control above (reopens Quadlet; the Podman 4.8+ floor alone was re-evaluated on 2026-09-14 and did not); accepting a runtime dep on `age` (reopens sops); accepting a server-side component (reopens Coolify-shaped alternatives). The unit/reference model (`type:` + `${...}` + `dpl check`) is the novel piece of dpl and must survive any refactor.

## Code conventions

- `thiserror` in libraries; `anyhow` only in `src/cmd/` and `main`.
- Match existing rustfmt-like wrapping (multiline `use` lists). Do not reformat unrelated code.
- Tests live in `#[cfg(test)]` next to the code (~290). Use `tempfile::TempDir` + `MainContext::write_test_unit`. No extra test framework.
- Path layout goes through `MainContext` so tests cannot drift.
- `ReferenceError` trail is innermost-first; Display prints outermost-first.
- Deploy failures: `DeployError::Step` records stage; CLI prints once then returns `DeployError::Reported`.
- Do not invent new config/state files; extend the existing `{base}/conf|state|secrets` layout.
- Prefer stdlib / already-declared crates. Do not add dependencies for small helpers.

## Do not

- Propose replacing the supervisor with systemd unit generation or Quadlet.
- Hold `deploy.lock` from serve.
- Start containers from `dpl deploy`.
- Pass secrets on argv.
