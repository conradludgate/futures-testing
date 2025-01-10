use std::{future::Future, pin::Pin};

use diatomic_waker::DiatomicWaker;
use futures_testing::{ArbitraryDefault, Driver, TestCase};

struct DiatomicWakerTestCase;

impl TestCase<'_> for DiatomicWakerTestCase {
    type Future<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

    type Driver<'a> = Source<'a>;

    // We don't need any randomness for DiatomicWaker, just a basic constructor.
    type Args = ArbitraryDefault<DiatomicWaker>;

    fn init<'a>(
        &self,
        args: &'a mut ArbitraryDefault<DiatomicWaker>,
    ) -> (Self::Driver<'a>, Self::Future<'a>) {
        let mut sink = args.0.sink_ref();
        let source = sink.source_ref();

        let sink = Box::pin(async move {
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
        });

        (Source(source), sink)
    }
}

struct Source<'a>(diatomic_waker::WakeSourceRef<'a>);

impl Driver<'_> for Source<'_> {
    type Args = ();

    fn poll(&mut self, _args: ()) {
        self.0.notify();
    }
}

#[test]
fn oneshot() {
    futures_testing::tests(DiatomicWakerTestCase).run();
}
