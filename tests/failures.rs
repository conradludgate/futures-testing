use futures_testing::{ArbitraryDefault, drive_poll_fn, testcase};
use std::ops::ControlFlow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Poll, Waker};

/// This test demonstrates a cancel-unsafe pattern: taking the receiver out of
/// an Option, consuming it, then putting it back. If the future is cancelled
/// mid-await, the receiver is dropped and subsequent calls panic.
///
/// The failure occurs because:
/// 1. Future takes rx from Option, begins awaiting recv()
/// 2. Cancel happens -- future dropped, rx dropped with it
/// 3. New future created -- rx.take() returns None, unwrap panics
#[test]
#[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
fn mpsc_cancel_unsafe() {
    futures_testing::tests(testcase!(|| {
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);

        let driver = drive_poll_fn(move |()| match tx.try_send(()) {
            Ok(()) => Poll::Ready(ControlFlow::Continue(())),
            Err(_) => Poll::Pending,
        });

        let mut rx = Some(rx);
        let factory = async move |_: ()| {
            let mut inner = rx.take().unwrap();
            let _ = inner.recv().await;
            rx = Some(inner);
        };

        (driver, factory)
    }))
    .seed(0x112e7ee800000001)
    .run();
}

/// This test demonstrates a future that never registers the waker.
/// On any poll that returns Pending, the framework detects that the waker
/// was not cloned (Arc refcount is 1) and was not woken, so it panics.
#[test]
#[should_panic(expected = "Waker passed to future was lost without being woken")]
fn no_waker_registration() {
    futures_testing::tests(testcase!(|args: &mut ArbitraryDefault<AtomicBool>| {
        let ready = &args.0;

        let driver = drive_poll_fn(move |()| {
            ready.store(true, Ordering::SeqCst);
            Poll::Ready(ControlFlow::Break(()))
        });

        let factory = async move |_: ()| {
            std::future::poll_fn(|_cx| {
                if ready.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await
        };

        (driver, factory)
    }))
    .seed(0xe46e62a900000001)
    .run();
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
    futures_testing::tests(testcase!(|args: &mut ArbitraryDefault<(
        AtomicUsize,
        Mutex<Option<Waker>>
    )>| {
        let counter = &args.0.0;
        let waker_store = &args.0.1;

        let driver = drive_poll_fn(move |()| {
            if let Some(w) = waker_store.lock().unwrap().take() {
                assert_eq!(counter.fetch_add(1, Ordering::SeqCst), 1);
                w.wake();
            }
            Poll::Ready(ControlFlow::Continue(()))
        });

        let factory = async move |_: ()| {
            counter.store(0, Ordering::SeqCst);
            std::future::poll_fn(|cx| match counter.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    *waker_store.lock().unwrap() = Some(cx.waker().clone());
                    Poll::Pending
                }
                2 => Poll::Ready(()),
                n => panic!(
                    "polled without driver making progress: counter is {}",
                    n + 1
                ),
            })
            .await
        };

        (driver, factory)
    }))
    .seed(0x94f262b900000001)
    .run();
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
    futures_testing::tests(testcase!(|args: &mut ArbitraryDefault<
        Mutex<Option<Waker>>,
    >| {
        let waker_store = &args.0;

        let driver = drive_poll_fn(move |()| {
            let guard = waker_store.lock().unwrap();
            if let Some(w) = guard.as_ref() {
                w.wake_by_ref();
                Poll::Ready(ControlFlow::Continue(()))
            } else {
                Poll::Pending
            }
        });

        let factory = async move |_: ()| {
            let mut stored_waker: Option<Waker> = None;
            std::future::poll_fn(|cx| {
                if stored_waker.is_none() {
                    stored_waker = Some(cx.waker().clone());
                    *waker_store.lock().unwrap() = Some(cx.waker().clone());
                }
                Poll::Pending
            })
            .await
        };

        (driver, factory)
    }))
    .seed(0x215f81a900000005)
    .run();
}
