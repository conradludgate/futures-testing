use futures::StreamExt;
use futures_testing::{drive_poll_fn, testcase};
use std::ops::ControlFlow;

#[test]
fn mpsc() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_poll_fn(move |item: u8| match tx.try_send(item) {
            Ok(()) => std::task::Poll::Ready(ControlFlow::Continue(())),
            Err(_) => std::task::Poll::Pending,
        });
        let factory = async move || {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
