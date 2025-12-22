use std::future::Future;

use diatomic_waker::DiatomicWaker;
use futures_testing::{ArbitraryDefault, Driver, drive_fn, testcase};

#[test]
fn oneshot() {
    futures_testing::tests(testcase!(|args: &mut ArbitraryDefault<DiatomicWaker>| {
        let mut sink = args.0.sink_ref();
        let source = sink.source_ref();

        let driver = drive_fn(move |()| {
            source.notify();
            std::task::Poll::Ready(())
        });

        let future = async move {
            let mut i = 0;
            sink.wait_until(|| {
                if i < 1 {
                    i += 1;
                    None
                } else {
                    Some(())
                }
            })
            .await;
        };

        (driver, future)
    }))
    .run();
}
