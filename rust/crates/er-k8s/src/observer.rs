//! The watch discipline (§6.5): one watcher per dispatcher process over the
//! managed selector, reconnecting with bounded backoff, and a `Resync` frame
//! on every (re)initialisation so the consumer reconciles from a LIST rather
//! than trusting a resumed stream. `kube`'s watcher already owns the
//! resourceVersion/410 mechanics; this loop owns the lifecycle: a stream that
//! ends or errors is re-opened at 250 ms doubling to 30 s, and a stream silent
//! past the stall bound is abandoned and re-opened.

use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{PodApi, Selector, WatchFrame};

/// Reconnect backoff: 250 ms doubling to 30 s (dagr's observer discipline).
pub const BACKOFF_MIN: Duration = Duration::from_millis(250);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A stream silent this long is presumed stalled and re-opened.
pub const STALL_BOUND: Duration = Duration::from_secs(90);

/// Drive the two-stream watch forever, forwarding frames to `tx`. Returns when
/// the receiver is gone (the dispatcher is shutting down).
pub async fn run_watch(api: &dyn PodApi, selector: Selector, tx: UnboundedSender<WatchFrame>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if tx.send(WatchFrame::Resync).is_err() {
            return;
        }
        let mut stream = api.watch(&selector);
        // A transport end (`Ok(None)`) or a stall (`Err`) falls out of the
        // loop: abandon the stream and re-open it after the backoff.
        while let Ok(Some(frame)) = tokio::time::timeout(STALL_BOUND, stream.next()).await {
            backoff = BACKOFF_MIN;
            if tx.send(frame).is_err() {
                return;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiError, EventSnapshot, JobSnapshot, Listing, ObjectKind, PodApi};
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A PodApi whose watch yields one scripted frame then ends, so the loop's
    /// reconnect-and-resync behaviour is observable.
    struct Scripted {
        connects: AtomicUsize,
    }

    impl PodApi for Scripted {
        fn create_job(
            &self,
            _job: &k8s_openapi::api::batch::v1::Job,
        ) -> BoxFuture<'_, Result<(), ApiError>> {
            Box::pin(async { Ok(()) })
        }
        fn get_job(&self, _name: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>> {
            Box::pin(async { Ok(None) })
        }
        fn list_events(&self, _about: &str) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn list(&self, _selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>> {
            Box::pin(async { Ok(Listing::default()) })
        }
        fn patch_labels(
            &self,
            _kind: ObjectKind,
            _name: &str,
            _labels: &[(&str, Option<&str>)],
        ) -> BoxFuture<'_, Result<(), ApiError>> {
            Box::pin(async { Ok(()) })
        }
        fn delete_job(&self, _name: &str) -> BoxFuture<'_, Result<(), ApiError>> {
            Box::pin(async { Ok(()) })
        }
        fn watch(&self, _selector: &Selector) -> BoxStream<'_, WatchFrame> {
            let n = self.connects.fetch_add(1, Ordering::SeqCst);
            let job = JobSnapshot {
                name: format!("job-{n}"),
                ..JobSnapshot::default()
            };
            Box::pin(futures::stream::iter(vec![WatchFrame::Job(job)]))
        }
    }

    #[tokio::test]
    async fn a_dead_stream_reconnects_with_a_resync_frame_first() {
        let api = Scripted {
            connects: AtomicUsize::new(0),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = run_watch(&api, Selector("er.dupezero.com/managed=task".into()), tx);
        let collector = async {
            let mut frames = Vec::new();
            while frames.len() < 4 {
                frames.push(rx.recv().await.expect("a frame"));
            }
            drop(rx); // hang up: run_watch must return
            frames
        };
        let (frames, ()) = tokio::join!(collector, watcher);
        // Resync, job-0, Resync, job-1: every (re)connect re-lists first.
        assert!(matches!(frames[0], WatchFrame::Resync));
        assert!(matches!(&frames[1], WatchFrame::Job(j) if j.name == "job-0"));
        assert!(matches!(frames[2], WatchFrame::Resync));
        assert!(matches!(&frames[3], WatchFrame::Job(j) if j.name == "job-1"));
        assert!(api.connects.load(Ordering::SeqCst) >= 2, "it reconnected");
    }
}
