# Secrets, IaC, and supply chain

Findings that live outside the Rust/TS test tiers. Per your scope decision, **enforcement of these is
deferred to a future CI layer** — this file documents each finding, its fix, and the gate that will
lock it, so wiring them up later is mechanical. The `/pentest` report surfaces them; it does not add
CI in this build.

## Contents
- The builder admin key (highest infra risk)
- Dev-credential confinement (§8.5) and settings.local.json
- EKS public endpoint
- What already exists (don't duplicate)
- Supply chain
- The deferred CI layer (ready to wire later)

## The builder admin key (highest infra risk)

`infra/terraform/envs/dev/builder.tf` creates a **long-lived static IAM access key with
`AdministratorAccess`** for an autonomous build loop. Its own `builder_prod_guard` mirrors
PlatformAdmin's denies **with one deliberate omission the file flags itself**: `iam:CreateAccessKey`
is *not* denied — so the key can mint further access keys, defeating the guard. Key material lands in
Secrets Manager `er/dev/shared/builder`. This contradicts `docs/infrastructure.md` §8.1 ("no
long-lived access keys exist anywhere except two service identities"); a third, most-powerful one now
exists, tagged `Retire = "when-build-loop-done"`.

- **Fix:** add `iam:CreateAccessKey` (and ideally `iam:CreateUser`, `iam:AttachUserPolicy`,
  `iam:PutUserPolicy`) to the deny set; schedule the key's retirement; prefer a short-lived role over a
  static key.
- **Future gate:** a CI policy/grep step asserting the builder deny set includes `iam:CreateAccessKey`,
  and a "builder key retired" check tied to the `Retire` tag. (A full OPA/conftest engine is overkill
  given none exists — a focused scan is enough.)

## Dev-credential confinement (§8.5) and settings.local.json

`docs/infrastructure.md` §8.5 states the rule: the dev token set
(`ERWEB_SESSION_SECRET`, `ERWEB_CREDENTIAL_KEY`, `ERSERVER_OPERATOR_TOKEN`, MinIO/Postgres passwords,
`admin@dupezone.com`) "stays exclusively in the local Compose path," and every cloud environment draws
its secrets from Secrets Manager. **Nothing mechanically enforces this today.**

Two concrete leaks:
- `.claude/settings.local.json` contains literal dev creds (`ERWEB_CREDENTIAL_KEY=...`,
  a `postgresql://postgres:er@...` DSN) and is ignored **only by your global gitignore**, not the repo
  `.gitignore`. A teammate without that global ignore would commit it. **This is the one quick win
  worth doing even while the rest is deferred** — add `.claude/settings.local.json` to the repo
  `.gitignore`.
- Dev creds are also in `frontend/dev/env.sh`, `frontend/dev/create-users.mjs`,
  `frontend/playwright*.config.ts`, `frontend/tests/.../helpers.ts`, `frontend/.env.example` — all
  dev-scoped and documented, but confinement is by convention only.

- **Future gate:** gitleaks over the repo + a grep gate asserting the §8.5 token set appears only under
  `frontend/dev/` (and test configs) and never in `infra/**` or k8s manifests. This spans
  frontend + infra + dotfiles, so it is a CI step, not a Rust test.

## EKS public endpoint

`infra/terraform/modules/eks/main.tf` sets `endpoint_public_access = true` with **no
`public_access_cidrs`** — the control-plane API endpoint is reachable from `0.0.0.0/0` (auth still
required). Access entries grant cluster-admin to the SSO PlatformAdmin role.

- **Fix:** set `public_access_cidrs` to an allowlist (office/VPN/Tailscale egress), or disable public
  access and reach the API over the private endpoint + tailnet.
- **Future gate:** checkov/tfsec over `infra/terraform/**` flags this directly
  (`CKV_AWS_38`/`39`-class rules).

## What already exists (don't duplicate)

- **cargo-deny** — `rust/deny.toml` + a `security` job in `.github/workflows/rust.yml`
  (`check advisories licenses bans sources`), with 6 documented advisory ignores. Keep using it; don't
  add cargo-audit on top.
- **Dependency pinning** — all GH Actions SHA-pinned (S9 policy), `Cargo.lock`/`pnpm-lock.yaml`
  committed, Helm/terraform versions pinned, MSRV in `rust-toolchain.toml`.
- **S3 public-access blocks** — all four flags true on every bucket.
- **Dev secret generation** — `scripts/dev_env.sh` is well-hardened (per-dev secrets to 0700 temp
  files, never promoted to cloud).

## Supply chain

- **cargo-deny** covers Rust advisories; **but Dependabot does not cover cargo or npm** (only
  github-actions, weekly) and `pnpm audit` never runs.
- **Future additions:** a `pnpm audit` step in `frontend.yml`; `.github/dependabot.yml` ecosystems for
  `cargo` + `npm`; an SBOM (syft / cargo-sbom).

## The deferred CI layer (ready to wire later)

When you want it, create `.github/workflows/security.yml` mirroring the SHA-pinning of `rust.yml`:
- **gitleaks** — secret scanning over the repo.
- **checkov** or **tfsec** — `infra/terraform/**` policy (catches the EKS endpoint directly).
- **SBOM** — syft / cargo-sbom artifact.
- **grep gates** — the §8.5 dev-cred-confinement gate and the `iam:CreateAccessKey` deny gate.
- edit `frontend.yml` — add `pnpm audit`.
- add `.github/dependabot.yml` — cargo + npm + github-actions.
- edit repo `.gitignore` — `.claude/settings.local.json`.

Each maps to a finding above; none is built in the current scope.
