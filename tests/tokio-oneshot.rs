use futures_testing::{drive_fn, testcase, Driver};
use std::ops::ControlFlow;

#[test]
fn oneshot() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, rx) = tokio::sync::oneshot::channel();

        let mut tx = Some(tx);
        let driver = drive_fn(move |()| {
            if let Some(tx) = tx.take() {
                tx.send(()).unwrap();
                return std::task::Poll::Ready(ControlFlow::Break(()));
            }
            std::task::Poll::Pending
        });

        let mut rx = Some(rx);
        let factory = async move || {
            if let Some(inner_rx) = rx.as_mut() {
                let _ = inner_rx.await;
                rx = None;
            }
        };

        (driver, factory)
    }))
    .run();
}
