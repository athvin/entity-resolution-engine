# Path classification for `devcontainer.yml`'s `detect` job. Sourced, not executed.
#
# DEVCONTAINER_INCLUDE is an ALLOWLIST, like `frontend-paths.sh`'s and unlike
# `detect-paths.sh`'s denylist. The direction matters: a tree this regex does
# not name is already skipped, so the risk is a path that SHOULD rebuild the
# image failing to be listed — never an accidental widening. Anything that goes
# into the image therefore has to be added here by hand.
#
# What is in it, and why each entry is not optional:
#
#   .devcontainer/**              the Dockerfile and devcontainer.json themselves
#   .github/workflows/devcontainer.yml
#                                 this workflow carries the registry name and the
#                                 `push` decision; a change to it must run the
#                                 build it defines, or a break in the gate would
#                                 stop reporting on exactly the PR most likely to
#                                 cause it — the same reasoning that keeps
#                                 `rust.yml` and `frontend.yml` out of
#                                 `detect-paths.sh`'s exempt set
#   scripts/devcontainer-*.sh     postCreate/postStart run INSIDE the container
#                                 and the build job executes them, so they are
#                                 build inputs even though they are not COPYed
#   scripts/ci/devcontainer-*.sh  this file, the detect step that sources it, and
#                                 the smoke assertions the build runs
#
# NOT included, deliberately: `rust/rust-toolchain.toml`, `frontend/.nvmrc` and
# `frontend/package.json`. The image resolves those at `make check` time rather
# than baking them, so a version bump does not invalidate it — and putting them
# here would rebuild the image on most backend PRs for nothing. The cost of that
# choice is that a bump to a channel the image's base cannot satisfy surfaces on
# first use rather than in CI; `scripts/ci/devcontainer-smoke.sh` is what would
# catch it on the next image-touching PR.
DEVCONTAINER_INCLUDE='^(\.devcontainer/|\.github/workflows/devcontainer\.yml$|scripts/devcontainer-[A-Za-z0-9._-]+\.sh$|scripts/ci/devcontainer-[A-Za-z0-9._-]+\.sh$)'
