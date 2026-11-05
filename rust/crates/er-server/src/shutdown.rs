//! When the HTTP server should stop accepting connections.
//!
//! `axum::serve` without `with_graceful_shutdown` has no shutdown path at all:
//! the process dies on the default SIGTERM handler, mid-response, and every
//! in-flight request becomes a connection reset at the client. On a `Recreate`
//! rollout that is a handful of 502s at the edge every deploy, attributed to
//! nothing. It is also the half of a blue/green cutover that keeps the old
//! colour correct — traffic is moved first, and the old pod has to finish what
//! it already accepted before it goes.
//!
//! Two decisions worth stating, because both are the kind that look like
//! oversights:
//!
//! **SIGTERM only; SIGINT keeps its default handler.** Kubernetes sends
//! SIGTERM, so that is the signal that has to drain. Ctrl-C locally sends
//! SIGINT, and catching it would make the first Ctrl-C *wait* — on a hung
//! request, indefinitely — while a second did nothing, because this future has
//! already resolved. A developer who interrupts a dev server expects it to
//! stop. Leaving SIGINT alone keeps that exactly as it was.
//!
//! **A failed registration waits forever rather than resolving.** The future
//! returned here is the server's shutdown trigger, so resolving it on an error
//! would shut the server down the instant it started serving — a total outage
//! produced by the error path of a feature meant to prevent small ones. If the
//! handler cannot be installed, the process keeps the pre-existing behaviour
//! (die on the default handler), which is strictly no worse than before.

/// Resolve when the process is asked to terminate, and not before.
///
/// Intended as the argument to `axum::serve(..).with_graceful_shutdown(..)`:
/// axum stops accepting new connections once this resolves and then waits for
/// the requests already in flight.
pub async fn on_sigterm() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut term) => {
            term.recv().await;
            tracing::info!("SIGTERM received; draining in-flight requests");
        }
        Err(e) => {
            // Never resolve: see the module header. A shutdown trigger that
            // fires on its own error is an outage, not a degradation.
            tracing::error!(error = %e, "could not install the SIGTERM handler; graceful shutdown is off");
            std::future::pending::<()>().await;
        }
    }
}

/// A flag that flips to `true` on SIGTERM, for a loop that drains rather than
/// a server that stops accepting.
///
/// [`on_sigterm`] is the right shape for `axum::serve`, which wants a future.
/// The dispatcher's loop wants the opposite: a value it can poll at the top of
/// each iteration, so it finishes the job in hand and exits at a boundary of
/// its own choosing. Same signal, two shapes.
///
/// **Registering this alongside [`on_sigterm`] is safe.** `tokio::signal` hands
/// every caller an independent stream, and a delivered signal wakes all of
/// them — so the HTTP server and an in-process dispatcher both learn about the
/// same SIGTERM, rather than one consuming it from under the other. That is
/// what makes the merged process able to drain both halves.
///
/// A failed registration leaves the flag permanently `false`, which is the
/// pre-existing behaviour (die on the default handler) rather than a loop that
/// exits immediately — the same fail-safe direction as [`on_sigterm`], for the
/// same reason: the error path of a drain must not itself end the process.
#[must_use]
pub fn sigterm_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let flag = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&flag);
    tokio::spawn(async move {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
                tracing::info!("SIGTERM received; draining the in-flight job");
                setter.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                tracing::error!(error = %e, "could not install the SIGTERM handler; the drain flag will never be set");
            }
        }
    });
    flag
}
