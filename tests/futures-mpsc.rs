use futures::StreamExt;
use futures_testing::{drive_sink, testcase};

#[test]
fn mpsc() {
    futures_testing::tests(testcase!(|| {
        let (tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_sink(tx);
        let factory = async move |_: ()| {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
