//! Bounded synchronous read work, isolated from the HTTP executor.
//!
//! Each read owns a single-threaded DuckDB session. Limit simultaneous sessions
//! to two so moving them off Tokio does not create an unbounded blocking pool.
//! The permit lives in the closure: cancelling the awaiting request cannot
//! release capacity while its non-cancellable DuckDB work is still running.

use std::sync::{Arc, LazyLock};

use tokio::sync::Semaphore;
use tokio::task::JoinError;

static CAPACITY: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub(crate) async fn run<F, T>(work: F) -> Result<T, JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    run_with_capacity(Arc::clone(&CAPACITY), work).await
}

async fn run_with_capacity<F, T>(capacity: Arc<Semaphore>, work: F) -> Result<T, JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let permit = capacity
        .acquire_owned()
        .await
        .expect("read capacity never closes");
    er_obs::spawn_blocking_in_span(move || {
        let _permit = permit;
        work()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_request_keeps_capacity_until_its_worker_finishes() {
        let capacity = Arc::new(Semaphore::new(1));
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let first = tokio::spawn(run_with_capacity(Arc::clone(&capacity), move || {
            started.send(()).unwrap();
            held.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        running.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(capacity.available_permits(), 0);

        let (started, mut running) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(run_with_capacity(Arc::clone(&capacity), move || {
            started.send(()).unwrap();
        }));
        tokio::task::yield_now().await;
        assert!(matches!(
            running.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        second.await.unwrap().unwrap();
        running.await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test]
    async fn failed_worker_releases_capacity_and_reports_failure() {
        let capacity = Arc::new(Semaphore::new(1));
        let result = run_with_capacity(Arc::clone(&capacity), || panic!("fixture panic")).await;
        assert!(result.unwrap_err().is_panic());
        assert_eq!(capacity.available_permits(), 1);
    }
}
