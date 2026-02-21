use futures::StreamExt;
use futures_testing::{drive_sink, testcase};

#[test]
fn test_sink_close() {
    futures_testing::tests(testcase!(|| {
        let (tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_sink(tx);
        // This future will only complete when the sink is closed (causing rx to yield None)
        let factory = async move |_: ()| {
            while rx.next().await.is_some() {}
        };

        (driver, factory)
    }))
    .run();
}
