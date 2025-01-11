use std::future::Future;

use diatomic_waker::DiatomicWaker;
use futures_testing::{drive_fn, ArbitraryDefault, Driver, TestCase};

struct DiatomicWakerTestCase;

impl<'b> TestCase<'b> for DiatomicWakerTestCase {
    // We don't need any randomness for DiatomicWaker, just a basic constructor.
    type Args = ArbitraryDefault<DiatomicWaker>;

    fn init<'a>(
        &self,
        args: &'a mut ArbitraryDefault<DiatomicWaker>,
    ) -> (impl Driver<'b>, impl Future) {
        let mut sink = args.0.sink_ref();
        let source = sink.source_ref();

        let driver = drive_fn(move |()| {
            source.notify();
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
    }
}

#[test]
fn oneshot() {
    futures_testing::tests(DiatomicWakerTestCase).run();
}
