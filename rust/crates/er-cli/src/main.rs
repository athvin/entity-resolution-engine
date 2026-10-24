//! The `er` binary: a shim over [`er_cli::cli::run`].
//!
//! The command-line surface itself lives in the library (`src/cli.rs`) rather
//! than here, so the deployed image can host it inside a single binary instead
//! of shipping a second copy of statically-linked DuckDB. `er-server` dispatches
//! to the same entry point when it is invoked under the name `er`; see
//! `rust/Dockerfile`.
//!
//! Building `er` on its own stays supported and is what local work uses
//! (`cargo build -p er-cli --bin er`, per docs/runbook.md).

fn main() -> anyhow::Result<()> {
    er_cli::cli::run()
}
