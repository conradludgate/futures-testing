use std::future::Future;

use futures::StreamExt;
use futures_testing::{drive_sink, testcase, Driver};

#[test]
fn mpsc() {
    futures_testing::tests(testcase!(|_args: &mut ()| {
        let (tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_sink(tx);
        let future = async move { while rx.next().await.is_some() {} };

        (driver, future)
    }))
    .run();
}
