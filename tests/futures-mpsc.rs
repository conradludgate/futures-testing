use futures::StreamExt;
use futures_testing::{drive_sink_with, generators as gs, testcase};

#[test]
fn mpsc() {
    futures_testing::tests(testcase!(|| {
        let (tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_sink_with(tx, gs::integers::<u8>());
        let factory = async move |()| {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
