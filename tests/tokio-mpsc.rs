use futures_testing::{drive_fn, testcase};
use std::ops::ControlFlow;

#[test]
fn mpsc() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);

        let driver = drive_fn(async move |item: u8| match tx.send(item).await {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        });

        let factory = async move || {
            let _ = rx.recv().await;
        };

        (driver, factory)
    }))
    .run();
}
