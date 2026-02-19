use futures_testing::{drive_poll_fn, testcase};
use std::ops::ControlFlow;

#[test]
fn oneshot() {
    futures_testing::tests(testcase!(|| {
        let (tx, rx) = tokio::sync::oneshot::channel();

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
            if let Some(inner_rx) = rx.as_mut() {
                let _ = inner_rx.await;
                rx = None;
            }
        };

        (driver, factory)
    }))
    .run();
}
