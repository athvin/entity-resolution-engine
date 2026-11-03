//! Carrying the current span across a blocking hop.
//!
//! `tracing`'s current span is **thread-local**. `tokio::task::spawn_blocking`
//! moves its closure to a different thread, so everything the closure creates
//! is parented to nothing — a fresh root, disconnected from the request or job
//! that caused it.
//!
//! This matters more here than in most codebases. The engine is synchronous
//! because DuckDB is, so the entire pipeline runs inside `spawn_blocking`
//! closures that the async dispatcher hands off. Miss one and every stage span
//! and every statement span beneath it detaches from the job that asked for
//! them — which does not look like a bug. It looks like tracing not working.
//!
//! And nothing catches it: no test fails, no type is wrong, the spans are all
//! still produced. They are simply orphans. That is why
//! `er-schema/tests/span_boundaries.rs` bans the bare form in `er-server`
//! rather than trusting a convention.

/// `tokio::task::spawn_blocking`, with the caller's span carried onto the
/// blocking thread.
///
/// Use this instead of the bare form anywhere the closure may create a span —
/// which, in this workspace, is anywhere it touches the engine.
///
/// ```ignore
/// // Wrong: every stage span inside becomes a disconnected root.
/// tokio::task::spawn_blocking(move || er_cli::run_all(&session, &config, &inputs))
///
/// // Right: the run's spans hang off the job that asked for them.
/// er_obs::spawn_blocking_in_span(move || er_cli::run_all(&session, &config, &inputs))
/// ```
pub fn spawn_blocking_in_span<F, T>(f: F) -> tokio::task::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // Captured on the CALLING thread, where the context still exists.
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        f()
    })
}

#[cfg(test)]
mod tests {
    use super::spawn_blocking_in_span;

    #[tokio::test]
    async fn the_closure_runs_and_its_value_comes_back() {
        // The span plumbing is invisible without a subscriber, so what is
        // asserted here is that wrapping does not change the hop's behaviour:
        // the closure runs on a blocking thread and its value is returned.
        let handle = spawn_blocking_in_span(|| 6 * 7);
        assert_eq!(handle.await.unwrap(), 42);
    }

    #[tokio::test]
    async fn a_panicking_closure_still_surfaces_as_a_join_error() {
        // The dispatcher relies on this: a panicking stage must come back as a
        // JoinError it can classify, not be swallowed by the span guard.
        let handle = spawn_blocking_in_span(|| panic!("stage exploded"));
        let err = handle.await.expect_err("a panic must surface");
        assert!(err.is_panic());
    }
}
