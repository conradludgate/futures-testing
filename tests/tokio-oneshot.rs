use std::future::Future;

use futures_testing::{drive_fn, testcase, Driver};

#[test]
fn oneshot() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, rx) = tokio::sync::oneshot::channel();

        let mut tx = Some(tx);
        let driver = drive_fn(move |()| {
            if let Some(tx) = tx.take() {
                tx.send(()).unwrap();
                return std::task::Poll::Ready(());
            }
            std::task::Poll::Pending
        });

        let future = rx;

        (driver, future)
    }))
    .run();
}
