use futures::{SinkExt, StreamExt};
use futures_testing::{drive_fn, testcase};
use std::ops::ControlFlow;

#[test]
fn async_driver() {
    futures_testing::tests(testcase!(|| {
        let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        // Async driver using async || syntax
        let driver = drive_fn(async move |item: u8| match tx.send(item).await {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        });

        let factory = async move |_: ()| {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
