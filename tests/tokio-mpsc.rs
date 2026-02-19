use futures_testing::{drive_fn, testcase};
use std::ops::ControlFlow;

#[test]
fn mpsc_rx() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);

        let driver = drive_fn(async move |item: u8| match tx.send(item).await {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        });

        let factory = async move |_: ()| {
            let _ = rx.recv().await;
        };

        (driver, factory)
    }))
    .run();
}

#[test]
fn mpsc_tx() {
    futures_testing::tests(testcase!(|_args: &mut ()| -> u8 {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);

        let driver = drive_fn(async move |_: ()| match rx.recv().await {
            Some(_) => ControlFlow::Continue(()),
            None => ControlFlow::Break(()),
        });

        let factory = async move |item: u8| {
            let _ = tx.send(item).await;
        };

        (driver, factory)
    }))
    .run();
}
