# Measurement discipline

## Contents

- Establish comparability first
- Span and duration arithmetic
- Resource evidence
- Coverage: what the absence of data means
- The historical records, and what they license
- Claiming a gain
- Handling artifacts

## Establish comparability first

Two runs are comparable only if the things that move results are equal. Check
before measuring, not when the numbers disagree:

- the engine build / image digest,
- input hashes and the generator scenario and seed,
- the configuration,
- CPU quota, container memory limit, DuckDB thread count and memory limit,
- the matching runtime version.

Different values mean the comparison is **not controlled**. That is not a
caveat to mention in a footnote; it means the delta is uninterpretable.

One deliberate exception in the historical campaign shape: a per-trial config
hash differs by design, because each trial got a fresh tenant namespace in its
storage paths. The semantic configuration hash is the comparability key, not
the full config hash. If you build a new harness, keep that distinction — you
need an identifier that ignores incidental path differences.

Also: **a fresh catalog does not mean cold caches.** Page cache and extension
state survive a new catalog. If you need cold, arrange it deliberately.

## Span and duration arithmetic

| Rule | Why |
|---|---|
| Never sum nested durations | parent and child spans overlap; the sum exceeds elapsed time |
| Separate startup from stage wall time | startup is real but is not the stage's cost |
| CPU can exceed wall | parallel execution; CPU totals are not elapsed time |
| Use the run's stage ledger and monotonic timestamps | wall-clock timestamps can move under you |

The failure this prevents: a report that adds stage times to a total exceeding
the command's own runtime, and then attributes the discrepancy to overhead.

## Resource evidence

- Buffer and spill numbers are **high-water marks**. They are not per-query
  allocations and they do not add up across queries.
- cgroup memory **includes page cache**, so it overstates what the process
  needs.
- Process RSS can **double-count shared pages**.
- Correlate peaks and any CPU throttling with the workload interval. A peak
  outside the interval belongs to something else.

Memory conclusions are the easiest to overstate. "Peak 12GB" from a high-water
mark does not mean the workload needs 12GB of headroom per query.

## Coverage: what the absence of data means

**Missing or failed evidence makes a conclusion incomplete. Never turn a
missing measurement into a zero.**

Specific traps:

- An **unprofiled thread is not an idle thread.**
- An unavailable catalog timing **cannot be inferred** from overall SQL latency.
- A capability-gated storage log is evidence only if it is actually present.
- A **local object-store stand-in cannot establish remote request latency or
  bandwidth.** It can establish CPU, plan shape and pruning behaviour — say
  which of those you are claiming.

Before concluding, state what you could not measure. A ranked ledger with an
honest coverage section is more useful than a complete-looking one built on
inferred zeroes.

## The historical records, and what they license

`docs/performance.md` and `docs/profiling.md` are **both banner-marked
historical records** of the retired Python/Splink pipeline and its Compose-based
harness. `docs/measurements/*.json` are that era's committed measurement
records.

They remain genuinely useful for:

- **findings** — e.g. that DuckDB's thread count is the dominant scaling lever,
- **rejected experiments**, so the same idea is not re-proposed blind,
- the shape of a disciplined measurement report,
- the promotion criteria the project held itself to.

They do **not** license:

- comparing a number measured today against one of those figures and calling
  the difference a delta — a different implementation produced them,
- treating their absolute timings as a current baseline,
- assuming their harness, env vars or scripts still exist. They do not.

If you need a current baseline, measure one.

## Claiming a gain

A single control/diagnostic pair **locates** a bottleneck. It does not
establish a gain.

To claim an improvement: repeated unprofiled baseline-versus-candidate trials,
with the comparability checklist satisfied, alternating arm order so ordering
effects do not accrue to one side. Report the median and the spread, not a best
case.

The live surface for this is `cargo bench -p dedupe-core` (`benches/pipeline.rs`,
`harness = false`). CI compiles the benches on main and nightly but does not
run them as a gate, so benchmark numbers are something you produce
deliberately, not something CI hands you.

State the **rejection criterion before running**. Deciding afterwards what
counts as success is how a neutral result becomes a reported win.

## Handling artifacts

Profiling artifacts may contain tenant data — SQL text, literal values, sampled
rows.

- Keep analysis **local**. Do not send artifact SQL or values to an external
  service.
- Apply the same access controls you would to source data.
- Retain the artifact directory alongside any finding you publish, so the claim
  stays checkable.
- Save durable findings in `docs/`.

And a scope rule: when the request is analysis, do not change production schema
or configuration, and do not launch an expensive run to satisfy curiosity.
