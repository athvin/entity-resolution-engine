# The development container

`make check` wants rustup at the channel `rust/rust-toolchain.toml` pins, mold,
DuckDB's extensions, corepack pnpm, and — for `make check-e2e` — a Postgres
reachable at `ER_TEST_PG_DSN`. Reproducing that on Windows natively is not a
one-afternoon job, and getting it subtly wrong produces failures that look like
code bugs rather than environment bugs.

`.devcontainer/` makes it one **Reopen in Container**.

## Opening it

You need [Docker Desktop](https://www.docker.com/products/docker-desktop/) with
the WSL2 backend, and VS Code with the
[Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers)
extension.

1. **Clone inside WSL2, not on `C:`.** `\\wsl$\…` paths are a 9p bind mount from
   the container's point of view, and this repo is heavy enough that the
   difference is minutes per build, not seconds. From a WSL2 shell:
   `git clone … && code .`
2. **Give the WSL2 disk ~60 GB of headroom** before the first build — see
   below. This is the step people skip.
3. `Ctrl+Shift+P` → **Dev Containers: Reopen in Container**.

The first open **pulls a prebuilt image** rather than building one, because
`devcontainer.json` names a `cacheFrom` that `.github/workflows/devcontainer.yml`
publishes to GHCR on every push to `main`. Expect a few minutes, not forty.

## The disk, which is the thing that actually goes wrong

`rust/target` in this repo is **30–80 GB**, and the primary checkout's has been
measured at 132 GB. Docker Desktop's WSL2 virtual disk defaults well below what
a full target plus the toolchain needs.

**A full disk does not say it is full.** It surfaces as:

```
error: linking with `cc` failed
```

which reads as a linker or dependency problem and sends you looking in the wrong
place entirely. CI has the same symptom, from the same cause, which is why
several Rust jobs carry an explicit disk-reclaim step.

To grow it: Docker Desktop → *Settings* → *Resources* → *Virtual disk limit*.

**The build directory is not `rust/target` in here.** `CARGO_TARGET_DIR` points
at `~/.cache/er/cargo-target`, which is a named volume living in the Docker VM's
own filesystem — so the biggest directory in the repo never touches the Windows
bind mount, and never reaches the host disk at all. Three caches work this way:

| Volume | Mounted at | Holds |
|---|---|---|
| `er-cargo-target` | `~/.cache/er/cargo-target` | the 30–80 GB build directory |
| `er-cargo-registry` | `/usr/local/cargo/registry` | downloaded crates |
| `er-duckdb` | `~/.duckdb` | DuckDB's extensions |

The registry is the odd one out: `CARGO_HOME` cannot be moved, because the base
image pins it with an unconditional `export` in `/etc/bash.bashrc` that the
devcontainer CLI's env probe re-applies over `containerEnv`. So that volume
lives outside `$HOME` and is kept writable by *mode* (2777, as the base image
already does for `/usr/local/cargo`) rather than by owner — which matters
because `devcontainers/ci` rewrites the container user's uid and chowns only
`$HOME`.

The consequence is that deleting a folder does not reclaim the space:

```sh
docker volume rm er-cargo-target er-cargo-registry er-duckdb
```

Those are recreated empty on the next open, so the next build is cold.

`frontend/node_modules` is deliberately *not* a volume — it is 783 MB, and
pnpm's symlink layout has to sit beside `frontend/package.json`, so it stays on
the workspace mount. Everything that has to be writable is a path the image
pre-creates owned by `vscode`, which is what lets Docker's copy-up hand each
volume over ready to use; **nothing in this container needs `sudo`**. If you add
a volume to `devcontainer.json`, add its directory to the `Dockerfile` in the
same change — `scripts/ci/devcontainer-smoke.sh` fails the build if you do not.

## What is inside, and two things that are not

| Tool | Where its version comes from |
|---|---|
| rust, rustfmt, clippy | `rust/rust-toolchain.toml` (`stable`), resolved by rustup |
| node 22 | a pinned devcontainer feature |
| pnpm | corepack, from `frontend/package.json`'s `packageManager` |
| postgres 16 | PGDG, matching the `postgres:16-alpine` service `rust.yml` runs |
| mold | Debian bookworm |

**mold is mandatory, not an optimisation.** `RUSTFLAGS` selects it. On
aarch64-linux — every Apple-silicon and Windows-on-ARM host — GNU ld fails the
bundled-DuckDB link with `undefined reference to
duckdb_register_static_extensions`, because libduckdb-sys's whole-archive
extension-autoregister object lands after the rlib group and single-pass ld
never extracts the defining member. mold resolves symbols globally regardless of
archive order. `rust/Dockerfile` installs it for the same reason.

**cmake is deliberately absent.** The aws crates here are pinned
`default-features = false` so `aws-lc-sys`, which needs cmake, never enters the
graph — and `rust/Dockerfile` does not install cmake either. A dev image
carrying it would build a graph the release image cannot, so an accidental
`default-features` relaxation would pass in here and fail `images.yml` after
merge. If you ever need it, add it to both files in one change.

## The gates

```sh
make check        # the backend PR gate
make check-e2e    # + DuckLake and the Postgres control plane
make frontend     # the frontend PR gate, Playwright tier included
```

`make check-e2e` needs no argument: a Postgres 16 cluster is started by
`scripts/devcontainer-poststart.sh` on every container start, and
`ER_TEST_PG_DSN` is preset to the same DSN CI uses. That cluster lives **inside
the image**, so rebuilding the container throws its data away — correct for a
test database, worth knowing before you put anything in it.

**The container is the right place to regenerate Playwright visual baselines.**
They compare on Linux only, so a baseline generated on a Windows or macOS host
is wrong for CI:

```sh
make frontend-baselines
```

## Troubleshooting

| Symptom | Cause |
|---|---|
| `linking with cc failed` | the disk is full; see above |
| `bad interpreter: /usr/bin/env bash^M`, or a gate script failing for no reason | CRLF line endings. `.gitattributes` forces `eol=lf` on `*.sh`, `Makefile` and the two extensionless scripts — re-clone if your checkout predates it |
| rust-analyzer finds no project | the cargo workspace root is `rust/`, not the repo root. `devcontainer.json` sets `rust-analyzer.linkedProjects` for this; check it survived a settings merge |
| the first open builds from scratch | the `cacheFrom` image could not be pulled. It is public; a corporate proxy blocking `ghcr.io` is the usual reason, and the build still succeeds, just slowly |
| `make check` fails on a missing tool | run `bash scripts/ci/devcontainer-smoke.sh` — it names which one, and is the same assertion CI runs on the image |

## Changing the image

`.github/workflows/devcontainer.yml` builds on any PR touching `.devcontainer/`,
`scripts/devcontainer-*.sh`, `scripts/ci/devcontainer-*.sh` or itself, and
publishes on `main`. Two rules travel with it:

- **It is not a required status check, and must not be added to the required set
  until it has reported green on `main` once** — a required context for a job
  that does not yet exist there leaves every PR pending forever. It uses the
  `detect` pattern rather than a `paths:` filter so that promoting it is a
  branch-protection edit rather than a workflow rewrite.
- **The image name lives in two files that cannot read each other** — the
  workflow's `IMAGE`/`IMAGE_TAG` and `devcontainer.json`'s `build.cacheFrom`.
  `scripts/ci/devcontainer-image-check.sh` keeps them equal, because the failure
  mode otherwise is silent: a `cacheFrom` naming an unpublished tag just builds
  from scratch, successfully, and nothing goes red.
