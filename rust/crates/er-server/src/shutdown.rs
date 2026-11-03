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
