use std::{future::Future, pin::Pin};

use futures_testing::{Driver, TestCase};

struct DiatomicWakerTestCase(diatomic_waker::DiatomicWaker);

impl TestCase<'_> for DiatomicWakerTestCase {
    type Future<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

    type Driver<'a> = Source<'a>;

    type Args = ();

    fn init(&mut self, _args: ()) -> (Self::Driver<'_>, Self::Future<'_>) {
        let mut sink = self.0.sink_ref();
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
    futures_testing::tests(DiatomicWakerTestCase(diatomic_waker::DiatomicWaker::new())).run();
}
