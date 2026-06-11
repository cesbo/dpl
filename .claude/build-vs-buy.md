# Build vs buy decisions

dpl deliberately keeps its homegrown systemd templating, AES-GCM secret store, and overall CLI shape instead of migrating to obvious alternatives. Do not propose any of these migrations without new information.

## Why

- **Quadlet rejected.** Podman 4.4+ only. Ubuntu 22.04 LTS ships podman 3.4 and Debian 12 ships 4.3 — no Quadlet. Target hosts include those distros. Also: Quadlet generates the `.service` for you, which reduces control over the `ExecStart`/`ExecStop` delegation to `dpl start`/`dpl stop`, healthcheck wiring, and custom `Restart=` logic that dpl needs.
- **sops / age rejected for secrets.** For a single-host root tool the threat model is identical: the decryption key lives on the same disk as the ciphertext. sops/age would only add a dependency and editor integration, not real protection. dpl resolves plaintext close to where it is used (an app's resolved runtime env is encrypted into `state/{name}/.env-v{N}.json` at deploy; `dpl start` decrypts it and passes vars via the podman process environment, never argv — the unit file holds no secret), so any solution converges on "plaintext reaches the container env" — env vars are always visible to root via `/proc/[pid]/environ`. The ~580 lines of `src/secret.rs` are the price of zero external runtime deps.
- **Kamal rejected.** Requires Ruby + SSH, has no typed cross-unit references (`${app-db:url}`, `${secret:...}`). Different shape, not a drop-in.
- **Coolify / Dokku / CapRover rejected.** Need a server-side component. dpl is explicitly a single static Rust binary, no daemon.
- **Postgres/MariaDB image env-var init not a replacement for `sql.rs`.** Official images create one user/db on first boot only. dpl's `db-server` + N×`db` split exists specifically to create multiple databases against one server post-init via `podman exec`.

## How to apply

- If a future conversation proposes "use Quadlet", "use sops", "use age", "use Kamal", "use Coolify" as a replacement for these subsystems — don't. The constraints (one static Rust binary, minimum runtime deps, Ubuntu LTS / Debian stable support) already excluded them.
- New information that *would* reopen the question: dropping Ubuntu 22.04 / Debian 12 from supported targets (reopens Quadlet); accepting a runtime dep on `age` (reopens sops); accepting a server-side component (reopens Coolify-shaped alternatives).
- The unit/reference model (`type: app|db-server|db|domain` + `${...}` validation via `dpl check`) is the actual novel piece of dpl and should be preserved across any future refactor.
