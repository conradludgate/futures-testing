use futures_testing::{drive_poll_fn, testcase};
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    .seed(0xaf251b5f00000003)
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
            NoWakerFuture {
                ready: ready.clone(),
            }
            .await
        };

        (driver, factory)
    }))
    .seed(0xe46e62a900000001)
    .run();
}

/// A future that shares a counter with its driver. The invariant: future
/// and driver alternate increments, so after the future increments the
/// counter should always be odd. The implementation just does `+= 1` each
/// time, so a spurious poll (two future polls in a row) produces an even
/// value, violating the invariant.
struct TurnCounter {
    counter: Arc<AtomicUsize>,
    waker_store: Arc<Mutex<Option<Waker>>>,
}

impl Future for TurnCounter {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match self.counter.fetch_add(1, Ordering::SeqCst) {
            0 => {
                *self.waker_store.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
            2 => Poll::Ready(()),
            n => panic!(
                "polled without driver making progress: counter is {}",
                n + 1
            ),
        }
    }
}

/// This test demonstrates a future that breaks under spurious polling
/// via an application-level invariant, not just waker bookkeeping.
///
/// A shared counter alternates between future and driver. The future
/// increments and asserts the result is odd. The driver only increments
/// when the counter is odd (its turn), making it even and waking the
/// future. Under normal alternation this holds, but a spurious poll
/// increments twice in a row, producing an even value and panicking.
#[test]
#[should_panic(expected = "polled without driver making progress")]
fn spurious_poll() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let counter = Arc::new(AtomicUsize::new(0));
        let waker_store: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));

        let counter_d = counter.clone();
        let waker_d = waker_store.clone();
        let driver = drive_poll_fn(move |()| {
            if let Some(w) = waker_d.lock().unwrap().take() {
                assert_eq!(counter_d.fetch_add(1, Ordering::SeqCst), 1);
                w.wake();
            }
            Poll::Ready(ControlFlow::Continue(()))
        });

        let factory = async move |_: ()| {
            counter.store(0, Ordering::SeqCst);
            TurnCounter {
                counter: counter.clone(),
                waker_store: waker_store.clone(),
            }
            .await
        };

        (driver, factory)
    }))
    .seed(0x82b7a72500000000)
    .run();
}

/// A future that registers the waker on the first poll but never updates it.
/// It shares the initial waker with the driver so the driver can wake it,
/// but after a ChangeWaker event the new waker is never stored.
struct StaleWakerFuture {
    stored_waker: Option<Waker>,
    waker_share: Arc<Mutex<Option<Waker>>>,
}

impl Future for StaleWakerFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.stored_waker.is_none() {
            this.stored_waker = Some(cx.waker().clone());
            *this.waker_share.lock().unwrap() = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// This test demonstrates a future that stores the waker once but never
/// updates it. The driver wakes the future (setting woken=true), which
/// allows ChangeWaker to fire. After ChangeWaker, the framework polls
/// with a new waker, but the future ignores it.
///
/// 1. Poll with waker1 -- future stores waker1
/// 2. Drive -- driver wakes future via stored waker, woken=true
/// 3. ChangeWaker (woken=true) -- framework creates waker2
/// 4. Poll with waker2 -- future skips storing waker2 (already has waker1)
/// 5. poll_fut: waker2 refcount is 1, woken is false -- panic
#[test]
#[should_panic(expected = "Waker passed to future was lost without being woken")]
fn stale_waker() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let waker_store: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));

        let waker_d = waker_store.clone();
        let driver = drive_poll_fn(move |()| {
            let guard = waker_d.lock().unwrap();
            if let Some(w) = guard.as_ref() {
                w.wake_by_ref();
                Poll::Ready(ControlFlow::Continue(()))
            } else {
                Poll::Pending
            }
        });

        let factory = async move |_: ()| {
            StaleWakerFuture {
                stored_waker: None,
                waker_share: waker_store.clone(),
            }
            .await
        };

        (driver, factory)
    }))
    .seed(0x5f50ece0000001f4)
    .run();
}
