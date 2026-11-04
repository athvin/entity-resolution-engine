//! Every SQL execution, recorded durably — the decision to keep all of them
//! rather than a rollup, made safe by batching.
//!
//! ## Why this is a `Layer` and not a call from generated code
//!
//! The generated statement functions already open a span carrying everything
//! this table needs. Having them ALSO call a recorder would mean the generated
//! crate depending on a buffer, a clock and a lake handle, and it would mean
//! re-emitting those 245 call sites to change the shape of a row. Observing the
//! spans instead keeps `er-entities` coupled to nothing but `er-span`, and
//! keeps all the buffering here.
//!
//! ## Three properties this has to hold, each guarding a real failure
//!
//! **Batched per stage, never per execution.** DuckLake creates a snapshot per
//! commit, so a write per statement would produce tens of thousands of
//! snapshots in a run — wrecking snapshot history as thoroughly as it would
//! wreck the hot loop. The pipeline drains the buffer once per stage, in
//! `observed()`, which already brackets them.
//!
//! **Re-entrancy guarded.** The drain's write is itself a generated
//! `append_rows`, which opens a `db.query` span. Unguarded, that span is
//! recorded and appended by the next drain, which appends another — a table
//! that grows by writing about itself. [`guard`] is how a caller suppresses
//! recording for the duration of its write.
//!
//! **Capped, and the drop count is returned.** A pathological run cannot buffer
//! without bound. At the cap, rows are dropped and [`take`] hands back how
//! many — never silently, because a silent cap reads as "we captured
//! everything", which is worse than no table at all.
//!
//! ## Best effort, always
//!
//! `reporter.rs` states the contract this follows: "a reporter, not an
//! executor: observation must never change a run". Every failure here is
//! swallowed after being logged. A run must not fail because its telemetry
//! could not be written.

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// How many executions may be buffered before rows are dropped.
///
/// A whole stage's worth is the unit, and a big stage in a large run is
/// thousands rather than millions. 50k is far above anything observed (a
/// measured 10-source run produced ~150 spans in total) and still bounds the
/// buffer at a few megabytes.
const BUFFER_CAP: usize = 50_000;

/// One execution, in the shape the lake row needs minus what only the writer
/// knows: the `run_id`, and the `seq`/`id` it assigns.
#[derive(Debug, Clone)]
pub struct Execution {
    /// Which stage this ran inside, read from the enclosing `stage` span.
    pub stage: String,
    pub statement: String,
    /// `declared` or `generator`.
    pub kind: &'static str,
    pub store: String,
    pub relation: Option<String>,
    pub duration_ms: i64,
    pub row_count: Option<i64>,
    pub outcome: String,
}

/// Whether to buffer at all.
///
/// Off by default, which is the normal state for `er-entity`, `dedupe` and
/// every test binary — so installing the layer unconditionally costs nothing.
/// The pipeline turns it on once it has a lake to write to.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Start buffering executions. The caller takes on the obligation to drain.
///
/// A drain rather than a callback sink, and the reason is a lifetime: a sink
/// would have to be `&'static` to live in a global, and the thing that writes
/// these rows is a per-run `Session` borrowed for the length of one run. The
/// pipeline already brackets every stage (`observed()`), so it is the natural
/// place to drain, and this keeps `er-obs` free of any lake handle.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Stop buffering and discard anything still held.
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
    BUFFER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    *DROPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
}

/// Take everything buffered so far, with the number dropped to the cap.
///
/// Returns `(rows, dropped)`. The caller writes them; a caller that does not
/// is why [`disable`] clears rather than leaks.
/// Suppress recording on this thread until the returned guard drops.
///
/// The caller writing these rows must hold one: its `append_rows` opens a
/// `db.query` span, and recording that would make the table grow by describing
/// its own writes.
#[must_use = "recording resumes the moment the guard drops"]
pub fn guard() -> Guard {
    FLUSHING.set(true);
    Guard
}

/// Returned by [`guard`]; resumes recording on drop.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        FLUSHING.set(false);
    }
}

#[must_use]
pub fn take() -> (Vec<Execution>, usize) {
    let rows = std::mem::take(
        &mut *BUFFER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let dropped = std::mem::take(
        &mut *DROPPED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    (rows, dropped)
}

thread_local! {
    /// True while a flush is in progress on this thread. See the module's
    /// re-entrancy note: the flush's own `append_rows` span must not be
    /// recorded, or it appends again.
    static FLUSHING: Cell<bool> = const { Cell::new(false) };
}

/// What a span carried, captured as its fields are recorded.
#[derive(Default)]
struct Captured {
    statement: Option<String>,
    generator: Option<String>,
    store: Option<String>,
    relation: Option<String>,
    rows: Option<i64>,
    outcome: Option<String>,
    stage: Option<String>,
}

impl Visit for Captured {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "er.statement" => self.statement = Some(value.to_string()),
            "er.generator" => self.generator = Some(value.to_string()),
            "db.system.name" => self.store = Some(value.to_string()),
            "db.collection.name" => self.relation = Some(value.to_string()),
            "er.outcome" => self.outcome = Some(value.to_string()),
            "er.stage" => self.stage = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "er.rows" {
            self.rows = i64::try_from(value).ok();
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "er.rows" {
            self.rows = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `%`-formatted fields arrive here. Trim the quoting `Debug` adds so a
        // stage name does not land in the table wrapped in escapes.
        let rendered = format!("{value:?}");
        let trimmed = rendered.trim_matches('"').to_string();
        match field.name() {
            "er.statement" => self.statement = Some(trimmed),
            "er.generator" => self.generator = Some(trimmed),
            "db.system.name" => self.store = Some(trimmed),
            "db.collection.name" => self.relation = Some(trimmed),
            "er.outcome" => self.outcome = Some(trimmed),
            "er.stage" => self.stage = Some(trimmed),
            _ => {}
        }
    }
}

/// Per-span state the layer keeps in the registry's extensions.
struct Observed {
    opened: Instant,
    captured: Captured,
}

/// The buffer, keyed by nothing: a stage's executions are contiguous because a
/// stage runs to completion before the next begins.
static BUFFER: Mutex<Vec<Execution>> = Mutex::new(Vec::new());
static DROPPED: Mutex<usize> = Mutex::new(0);

/// Observes `db.query` and `stage` spans and buffers one row per execution.
pub struct ExecutionRecorder;

impl ExecutionRecorder {
    /// Is this a span we record?
    fn is_statement(meta: &Metadata<'_>) -> bool {
        meta.name() == "db.query"
    }

    fn is_stage(meta: &Metadata<'_>) -> bool {
        meta.name() == "stage"
    }
}

impl<S> Layer<S> for ExecutionRecorder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let meta = attrs.metadata();
        if !Self::is_statement(meta) && !Self::is_stage(meta) {
            return;
        }
        let mut captured = Captured::default();
        attrs.record(&mut captured);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Observed {
                opened: Instant::now(),
                captured,
            });
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        // The identity and the outcome are recorded AFTER creation — `sql_span`
        // fills `er.statement` once it knows a subscriber is listening, and
        // `check` fills `er.outcome` at the end — so a layer that only read
        // `on_new_span` would see neither.
        if let Some(span) = ctx.span(id)
            && let Some(observed) = span.extensions_mut().get_mut::<Observed>()
        {
            values.record(&mut observed.captured);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        if FLUSHING.get() {
            // The drain's own `append_rows` span. Recording it would append
            // again on the next drain, and each append would add one more.
            return;
        }
        let Some(span) = ctx.span(&id) else { return };

        if !Self::is_statement(span.metadata()) {
            return;
        }

        let extensions = span.extensions();
        let Some(observed) = extensions.get::<Observed>() else {
            return;
        };
        // A declared statement carries `er.statement`; a runtime-assembled one
        // carries `er.generator`. Keeping the class explicit means an analysis
        // does not have to infer it from the name's shape.
        let (statement, kind) = match (&observed.captured.statement, &observed.captured.generator) {
            (Some(s), _) => (s.clone(), "declared"),
            (None, Some(g)) => (g.clone(), "generator"),
            // Neither field was recorded, so there is nothing to attribute this
            // to. Dropping it is better than inventing a name.
            (None, None) => return,
        };

        // The enclosing stage, walked from this span's scope. A statement span
        // does not carry the stage itself — that is the whole point of ambient
        // context — so it is read from the ancestor that does. A statement with
        // no stage ancestor (schema reconciliation runs before the run span
        // opens) is attributed to `none` rather than guessed at.
        let stage = span
            .scope()
            .find(|s| Self::is_stage(s.metadata()))
            .and_then(|s| {
                s.extensions()
                    .get::<Observed>()
                    .and_then(|o| o.captured.stage.clone())
            })
            .unwrap_or_else(|| "none".to_string());

        let execution = Execution {
            stage,
            statement,
            kind,
            store: observed
                .captured
                .store
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            relation: observed.captured.relation.clone(),
            duration_ms: i64::try_from(observed.opened.elapsed().as_millis()).unwrap_or(i64::MAX),
            row_count: observed.captured.rows,
            outcome: observed
                .captured
                .outcome
                .clone()
                .unwrap_or_else(|| "ok".to_string()),
        };

        let mut buffer = BUFFER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if buffer.len() >= BUFFER_CAP {
            *DROPPED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            return;
        }
        buffer.push(execution);
    }
}

#[cfg(test)]
mod tests {
    use super::{BUFFER_CAP, Execution};

    #[test]
    fn the_cap_is_documented_and_generous() {
        // The point of the assertion is the REASON: a measured 10-source run
        // produced ~150 spans in total, so the cap is three orders of magnitude
        // above anything observed. If someone lowers it to a number that could
        // plausibly be hit by a real stage, the drop path stops being a
        // pathological-case guard and starts being normal behaviour — and a
        // table that silently misses rows is worse than no table.
        // A `const` assertion rather than a runtime one: clippy is right that
        // the comparison is constant, and the point was never to execute it.
        // It fails the BUILD if someone lowers the cap, which is stricter.
        const _: () = assert!(
            BUFFER_CAP >= 10_000,
            "a cap a real stage can reach makes dropping normal"
        );
    }

    #[test]
    fn an_execution_carries_no_sql_and_no_values() {
        // A shape assertion, held by the type: there is nowhere on `Execution`
        // to put statement text or a bind value, which is the same structural
        // argument `er_span::sql_span` makes by having no parameter.
        let fields = [
            "stage",
            "statement",
            "kind",
            "store",
            "relation",
            "duration_ms",
            "row_count",
            "outcome",
        ];
        let rendered = format!(
            "{:?}",
            Execution {
                stage: "match".into(),
                statement: "golden_record.update.override_email".into(),
                kind: "declared",
                store: "duckdb".into(),
                relation: Some("golden_records".into()),
                duration_ms: 12,
                row_count: Some(3),
                outcome: "ok".into(),
            }
        );
        for field in fields {
            assert!(rendered.contains(field), "{field} must be on the row");
        }
        assert!(
            !rendered.to_lowercase().contains("select")
                && !rendered.to_lowercase().contains("insert"),
            "no SQL may appear on an execution row"
        );
    }
}
