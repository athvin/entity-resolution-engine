# Exit codes and the failure taxonomy

## Contents

- The five exit codes
- The seven error classes
- How a class maps to an exit code
- Retryable means "the cause is outside the run"
- Reading a failure
- Rules for new failure paths

## The five exit codes

Uniform across every `er` command (`er-errors`, the S4.0 contract):

| Code | Name | Meaning |
|---|---|---|
| `0` | `Success` | the command did work and it succeeded |
| `1` | `StageFailure` | a stage attempted something and it failed |
| `2` | `Config` | the configuration is wrong — nothing was attempted |
| `3` | `Precondition` | a precondition for running was not met |
| `10` | `NothingToDo` | **success**: there was nothing to do |

`10` is the one that trips people up. It sits deliberately outside the S4.7
failure table: a no-op run is a correct outcome, not an error. Any wrapper that
does `if exit != 0; then fail` will misreport healthy runs — treat `0` and `10`
as success.

`2` and `3` are distinguishable on purpose. `Config` means the input was never
valid; `Precondition` means the input was fine but the world was not ready
(a lock held, a required prior stage missing). One is a code or config fix, the
other is usually a retry or a sequencing fix.

## The seven error classes

`ErrorClass` is what gets stored in `run_stages.error_class`. The string from
`as_str()` **is** the stored `VARCHAR` and the S5.2 log value — so renaming a
variant's spelling is a data-format change, not a refactor.

| Class | Cause |
|---|---|
| `transient_io` | I/O that may succeed on a retry |
| `lock_conflict` | another writer held the lock |
| `precondition` | a required condition for the run was not met |
| `config` | invalid or inconsistent configuration |
| `contradiction` | contradictory steward assertions — see CONTRADICTION-1 |
| `non_convergence` | a bounded fixpoint did not settle |
| `data` | the data itself was unusable; also the default for an unclassified failure |

`data` doubling as the unclassified default matters when triaging: a `data`
class does not prove anyone deliberately classified it that way. If a failure
is `data` and the message does not look data-shaped, the real cause may simply
never have been classified.

## How a class maps to an exit code

One transcription of S4.7's table, derived once in `ErrorClass::exit_code`:

| Classes | Exit |
|---|---|
| `lock_conflict`, `precondition` | `3` (`Precondition`) |
| `config` | `2` (`Config`) |
| `transient_io`, `contradiction`, `non_convergence`, `data` | `1` (`StageFailure`) |

Note `transient_io` exits `1`, not `3`. A retryable failure is still a stage
failure — retryability is advisory metadata, not a different outcome.

## Retryable means "the cause is outside the run"

`ErrorClass::retryable()` is true for exactly two classes: `transient_io` and
`lock_conflict`. Those are the two whose cause is external — a flaky network or
another writer — so repeating the identical run may succeed.

It is **advisory to an operator, and nothing in the CLI acts on it.** There is
no automatic retry. If you are adding orchestration that retries, this flag is
the signal to read; do not infer retryability from the message text.

The four non-retryable classes will fail identically on a retry, which is why
re-running them wastes a slot: the config is still wrong, the assertions still
contradict, the fixpoint still will not settle, the data is still bad.

## Reading a failure

1. **Exit code first.** `2` and `3` mean nothing was attempted — look at config
   and preconditions, not at the stage logic.
2. **Then `run_stages.error_class` and `error_detail`.** For a contradiction,
   `error_detail` names the offending `assertion_id`s and the closure
   component — that is the whole diagnosis, no further digging needed.
3. **Then the chained message.** Error types in this workspace keep their cause
   out of `Display`, so a flattened single-line message is the whole chain only
   if whoever flattened it walked `.source()`. See the rust-conventions skill.

For a failed job in the control plane, `resume` re-queues it. Runs are
deterministic over their pinned inputs, so a resumed run reproduces rather than
approximating — it is not a fresh attempt at a moving target.

## Rules for new failure paths

- Classify deliberately. Falling through to `data` is allowed but it is a
  choice, and it is the choice that makes triage harder later.
- Do not interpolate a `#[source]` into an error's `Display` — it double-prints
  under any reporter that walks the chain, and there is a test that fails the
  build on it.
- `nothing_to_do` is the only case with no error class, because it is not a
  failure. If you find yourself wanting a class for it, the operation probably
  did do something and should exit `0`.
- A stage that cannot find its required inputs should **fail with a clear
  reason**, never silently skip. The dispatcher follows the same rule for jobs.
