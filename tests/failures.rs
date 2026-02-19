use futures_testing::{drive_poll_fn, testcase};
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// This test demonstrates a cancel-unsafe pattern: consuming the sender before
/// the receiver completes. If cancelled after send but before receive, the
/// receiver is dropped and subsequent retries fail.
///
/// The failure occurs because:
/// 1. Future starts, takes rx from Option, begins awaiting
/// 2. Cancel happens - future dropped, rx dropped with it
/// 3. New future created - rx.take() returns None, future does nothing
/// 4. Driver runs, sends to dropped receiver, returns Break
/// 5. Assertion fails: "future was not woken when driver made progress"
#[test]
#[should_panic(expected = "future was not woken when driver made progress")]
fn oneshot_cancel_unsafe() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let mut tx = Some(tx);
        let driver = drive_poll_fn(move |()| {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
                return std::task::Poll::Ready(ControlFlow::Break(()));
            }
            std::task::Poll::Pending
        });

        let mut rx = Some(rx);
        let factory = async move |_: ()| {
            if let Some(rx) = rx.take() {
                let _ = rx.await;
            }
        };

        (driver, factory)
    }))
    .seed(0xb294428e0000000a)
    .run();
}

/// A future that never registers the waker -- it just polls shared state.
struct NoWakerFuture {
    ready: Arc<AtomicBool>,
}

impl Future for NoWakerFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.ready.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// This test demonstrates a future that never registers the waker.
/// On any poll that returns Pending, the framework detects that the waker
/// was not cloned (Arc refcount is 1) and was not woken, so it panics.
#[test]
#[should_panic(expected = "Waker passed to future was lost without being woken")]
fn no_waker_registration() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let ready = Arc::new(AtomicBool::new(false));

        let ready2 = ready.clone();
        let driver = drive_poll_fn(move |()| {
            ready2.store(true, Ordering::SeqCst);
            Poll::Ready(ControlFlow::Break(()))
        });

        let factory = async move |_: ()| {
            NoWakerFuture { ready: ready.clone() }.await
        };

        (driver, factory)
    }))
    .seed(0xe46e62a900000001)
    .run();
}

/// A future that registers the waker on the first poll but never updates it.
/// After a waker change, the new waker is never stored, so poll_fut detects
/// the new waker was dropped without being woken.
struct StaleWakerFuture {
    stored_waker: Option<Waker>,
}

impl Future for StaleWakerFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.stored_waker.is_none() {
            this.stored_waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// This test demonstrates a future that stores the waker once but never
/// updates it. After a waker change, the framework polls with a new waker,
/// but the future ignores it. The framework detects the new waker was not
/// stored and panics.
///
/// The failure requires a ChangeWaker event between two polls:
/// 1. Poll with waker1 -- future stores waker1 (Arc refcount > 1, poll_fut passes)
/// 2. ChangeWaker -- framework creates waker2
/// 3. Poll with waker2 -- future skips storing waker2 (already has waker1)
/// 4. poll_fut: waker2 refcount is 1 (not cloned), woken is false -- panic
///
/// A no-op driver ensures only the waker-lost assertion can fire.
/// size_min(1000) ensures enough choices for the rare ChangeWaker event
/// (probability 1/256 per choice) to appear reliably.
#[test]
#[should_panic(expected = "Waker passed to future was lost without being woken")]
fn stale_waker() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let driver = drive_poll_fn(|()| -> Poll<ControlFlow<()>> { Poll::Pending });

        let factory = async move |_: ()| {
            StaleWakerFuture { stored_waker: None }.await
        };

        (driver, factory)
    }))
    .seed(0x5b96b04a000001f4)
    .run();
}
