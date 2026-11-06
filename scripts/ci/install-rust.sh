#!/usr/bin/env bash
# Put the stable toolchain on the runner. SEVEN call sites across rust.yml and
# agent-task.yml, in two variants.
#
# The variants are the reason this is shared rather than repeated. Five sites
# install the toolchain alone; two also add rustfmt and clippy, because only
# the `lint` job and the agent need them. All seven agreed on
# `--no-self-update` when they were collapsed — which is the flag worth
# protecting, since dropping it lets rustup replace ITSELF mid-job, and a
# seven-way copy is seven chances for one of them to lose it unnoticed.
#
# NOT `rust-toolchain.toml`'s job: that file pins the channel rustup resolves
# (`stable`) and already declares rustfmt and clippy, which is why no version
# appears here. This only ensures the toolchain is present and default.
#
# `--toolchain stable` IS LOAD-BEARING even though the default was just set to
# it. The call sites used to run from `rust/`, where the override file selects
# the toolchain; they now run from the repo root, where nothing does. Naming
# the toolchain makes the two identical instead of equal-by-coincidence — the
# coincidence being that `rust-toolchain.toml` pins `stable` rather than a
# version number, which it need not always.
#
# Usage:
#   bash scripts/ci/install-rust.sh                 # toolchain only
#   bash scripts/ci/install-rust.sh --components    # + rustfmt, clippy
set -euo pipefail

rustup update stable --no-self-update
rustup default stable

case "${1:-}" in
--components)
	rustup component add --toolchain stable rustfmt clippy
	;;
"") ;;
*)
	echo "install-rust: unknown argument '$1' (expected --components)" >&2
	exit 2
	;;
esac
