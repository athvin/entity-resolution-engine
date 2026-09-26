# frontend — the standalone cloud UI

The web application over the erserver control plane, implementing
[docs/frontend-design.md](../docs/frontend-design.md). A **standalone pnpm
project**, mirroring how `server/` is a standalone uv project: own lockfile,
own CI job, nothing at the repo root references this directory at runtime.

Next.js (App Router) serves both the UI and the **BFF** — the only network
boundary the browser ever sees. The BFF holds sessions, org API keys, the
operator token, and the Anthropic key server-side; there are deliberately no
`NEXT_PUBLIC_*` variables in this project. Erserver types are generated from
the server's OpenAPI schema and committed (`pnpm gen:api` regenerates;
CI fails on drift).

## Layout

| Path       | Role                                                                                                                                                                                  |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/app/` | Routes: `(auth)` login, `(tenant)/[org]/…` the nine tenant screens, `(admin)` super-admin console, `api/` the BFF route handlers                                                      |
| `src/lib/` | `api/` typed erserver client + committed schema, `db/` Drizzle schema for the BFF-owned `erweb` tables + credential vault, `bff/` proxy/error envelope, `env.ts` validated server env |
| `drizzle/` | Committed SQL migrations for the `erweb` schema (control-plane Postgres)                                                                                                              |
| `dev/`     | The dev/e2e substrate: compose file, boot script, tenant seed                                                                                                                         |
| `tests/`   | `unit`/`component` (Vitest), `functional` (Playwright vs mock erserver), `e2e` (full stack), `mock-erserver/`                                                                         |

## Development

```sh
make frontend-dev    # containers (Postgres x2 + MinIO) + erweb migrations + erserver api/dispatcher
make frontend-seed   # one resolved tenant (acme-dev): corpus, model, full run (~3-4 min)
cd frontend && corepack pnpm dev   # Next.js on :3000
```

`dev/compose.yaml` replaces the three ad-hoc `docker run` containers
`server/README.md` documents (same ports 5433/5434/9000, same credentials) —
the two setups are mutually exclusive on one machine. `make frontend-dev-reset`
tears everything down, volumes included. Dev-only keys land in
`dev/.state/keys.json`; nothing under `dev/.state/` is ever committed.

Seeded logins (dev stack and the mock tier alike; password `password-123!`):
`root@er.dev` (super admin, no memberships — views tenants via the operator
token), `admin@acme.dev`, `steward@acme.dev`, `viewer@acme.dev`.

## Tests

Three escalating tiers, mirroring the server's bare / +Postgres / +lake prose:

```sh
# bare (seconds, no services): lib logic, components, BFF handlers
corepack pnpm test

# + mock erserver (~minutes, no Docker): the bulk of coverage — every screen and
# flow, error/empty states, visual baselines, all three device projects
# (desktop Chromium 1440x900, iPhone 15 WebKit, Pixel 7)
corepack pnpm build && corepack pnpm test:e2e

# + full substrate (the release gate, ~15 min): the real journey against the
# seeded dev stack, gated on FRONTEND_E2E=1
make frontend-dev && make frontend-seed && make frontend-e2e
```

The mock tier exists because BFF→erserver calls happen server-side, where
browser-level route interception can't reach: `tests/mock-erserver/server.mjs`
serves deterministic fixtures and the BFF points at it via `ERSERVER_BASE_URL`.

**Visual baselines are Linux-only.** They are generated in CI's Ubuntu runner or
via `make frontend-baselines` (which runs `--update-snapshots` inside the pinned
`mcr.microsoft.com/playwright` image); on macOS the comparison is skipped
(`ignoreSnapshots`), so local runs never fight font rendering. The full-stack
tier never pixel-compares — each journey step saves a full-page screenshot
(`tests/e2e/helpers.ts` `snap()`) as the visual record instead.

## Environment

See `.env.example`. `ERWEB_*` is the BFF's own surface (database URL for the
`erweb` schema, session secret, AES-256-GCM credential-vault key);
`ERSERVER_BASE_URL`/`ERSERVER_OPERATOR_TOKEN` point at the control plane;
`ANTHROPIC_API_KEY` powers the assistant. Env is zod-validated at first use
(`src/lib/env.ts`) — missing settings fail loudly, in the erserver style.
