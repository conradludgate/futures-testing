use futures::{SinkExt, StreamExt};
use futures_testing::{drive_fn_with, generators as gs, testcase};
use std::ops::ControlFlow;

#[test]
fn async_driver() {
    futures_testing::tests(testcase!(|| {
        let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        // Async driver using async || syntax
        let driver = drive_fn_with(gs::integers::<u8>(), async move |item| {
            match tx.send(item).await {
                Ok(()) => ControlFlow::Continue(()),
                Err(_) => ControlFlow::Break(()),
            }
        });

        let factory = async move |_: ()| {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
