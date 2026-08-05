use futures_testing::{drive_fn_with, generators as gs, testcase};
use std::ops::ControlFlow;

#[test]
fn mpsc_rx() {
    futures_testing::tests(testcase!(|| {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);

        let driver = drive_fn_with(gs::integers::<u8>(), async move |item| {
            match tx.send(item).await {
                Ok(()) => ControlFlow::Continue(()),
                Err(_) => ControlFlow::Break(()),
            }
        });

        let factory = async move |()| {
            let _ = rx.recv().await;
        };

        (driver, factory)
    }))
    .run();
}

#[test]
fn mpsc_tx() {
    futures_testing::tests(testcase!(|| -> u8 {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(4);

        let driver = drive_fn_with(gs::unit(), async move |()| match rx.recv().await {
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
