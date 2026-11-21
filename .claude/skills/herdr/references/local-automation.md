# Local Herdr automation

The optional helpers under `scripts/herdr/` were migrated from a personal
Herdr installation. They are separate from pane layout and do not run as part
of a skill load or repository gate.

- `kimi-quota-check.sh` runs a real, ephemeral Codex turn through the local
  `kimi` profile. It consumes provider quota; only run it when requested.
- `codex1-watch.sh` preserves an existing local recovery watcher. It probes
  quota and may prompt the named agent `codex-1`. It is not installed or started
  automatically by the repository. Retarget or enable it only on request.

Both keep state in `${HERDR_STATE_DIR:-$HOME/.local/state/herdr}`. The existing
LaunchAgent's script and log paths must be updated when migrating an installed
watcher. Preserve its state, including notification markers, before reloading
it; otherwise a migration can send a duplicate recovery message.

The API credential stays in that private state directory with owner-only
permissions. No credential, runtime log or notification response belongs in
Git. Back up personal installs outside every skill-discovery directory before
removing them. Keep built-in agent skills managed by their application.

The quota probe's exit codes distinguish a completed turn, a provider rate
limit and an unknown failure. A successful model-list request does not exercise
generation quota. An estimated reset time based on a prior failure is not a
provider promise.
