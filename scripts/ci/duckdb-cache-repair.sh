#!/usr/bin/env bash
# Repair the restored libduckdb-sys source tree, or leave an intact one alone.
#
# #424 cured the eight compile jobs by DELETING rust-cache. rust.yml's
# `coverage` job is the one that keeps it — for the reasons stated on the
# restore step there — so it is the one place the poisoning survives, and it
# needs this repair instead.
#
# THE MECHANISM, from the crate source rather than the symptom.
# `libduckdb-sys` does not unpack into `OUT_DIR`: `src/build_paths.rs:40`
# walks UP from it to a tree shared across the whole target dir,
# `target/duckdb-sources/<key>/`, guarded by a `.complete` DOTFILE. Present,
# the build script returns early. Absent, it unpacks into a fresh staging dir
# and `fs::rename`s that over `<key>/` — and renaming onto a non-empty
# directory is ENOTEMPTY, which is Linux errno 39:
#
#   unpacking bundled DuckDB sources: Os { code: 39, kind: DirectoryNotEmpty }
#
# THAT PANIC MESSAGE NAMES THE WRONG OPERATION. `fs::rename` in `publish()` is
# the only syscall in that function that can return 39 — the staging dir is
# uniquely minted, so `archive.unpack` cannot — and the `?` carries it to an
# `.expect("unpacking bundled DuckDB sources")` at `build_bundled_cc.rs:65`.
# Reading that string as the location is what sent two earlier fixes at
# `OUT_DIR`, where they verifiably deleted `.o` files while the panic happened
# anyway.
#
# So the poisoned state is a POPULATED `<key>/` with NO `.complete`, which is
# what a rust-cache save/restore round-trip leaves behind. The repair cannot
# go before the save — rust-cache writes in its POST-step, after every step
# the workflow can declare — so after the restore is the only place it can go.
#
# Dated prediction this exists to stop: #376 bumped duckdb at 01:07 on
# 2026-11-03, after that day's nightly. The next scheduled run therefore gets
# a NEW `<key>`, unpacks cleanly, passes, and SAVES the stripped tree; the run
# after that restores it and dies before a test. Breakage never lands on the
# run that caused it.
#
# The test below is exactly the build script's own precondition, so an intact
# tree is left alone and keeps the fast path. Dropping one costs the tarball
# unpack (seconds), not the compile (minutes) — the C++ objects come from
# sccache. IT ECHOES PER TREE because both earlier fixes went GREEN having
# done nothing: "it ran" and "it did something" have to be distinguishable in
# the log.
#
# Run: bash scripts/ci/duckdb-cache-repair.sh
#
# COVERAGE BUILDS UNPACK A SECOND TREE. `cargo llvm-cov` does not build in
# `target/`: its default target dir is `target/llvm-cov-target/`, so the
# instrumented build's shared source tree is
# `rust/target/llvm-cov-target/duckdb-sources/<key>/`. rust-cache's
# `workspaces: rust` saves all of `rust/target/`, sentinel-stripped there too,
# so the same poisoned restore lands one directory over and the nightly died
# before a test while this script reported the root tree intact. Both roots
# are repaired below; a cold cache has neither, and that absence still prints.
#
# NO `set -u`, deliberately and with a failure to show for it: under `-u`,
# expanding `"${trees[@]}"` when `nullglob` left the array EMPTY is an unbound
# variable, so the absent-tree path — the normal one on a cold cache — exited
# 1. The inline original ran under GitHub's default `bash -eo pipefail`, which
# is what is reproduced here.
set -eo pipefail

cd "$(dirname "$0")/../.."

shopt -s nullglob
trees=(rust/target/duckdb-sources/*/ rust/target/llvm-cov-target/duckdb-sources/*/)
if [ ${#trees[@]} -eq 0 ]; then
	echo "none     rust/target{,/llvm-cov-target}/duckdb-sources/ absent - build script unpacks cold"
fi
for tree in "${trees[@]}"; do
	if [ -f "$tree.complete" ]; then
		echo "intact   $tree"
	else
		echo "REPAIRED $tree (no .complete - would panic ENOTEMPTY)"
		rm -rf "$tree"
	fi
done
