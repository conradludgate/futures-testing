use futures_testing::{drive_poll_fn, testcase};
use std::ops::ControlFlow;

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
    .seed(0x30b669de00002811)
    .run();
}
