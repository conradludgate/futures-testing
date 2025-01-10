use std::{future::Future, pin::Pin};

use arbitrary::{Arbitrary, Unstructured};
use diatomic_waker::DiatomicWaker;
use futures_testing::{Driver, TestCase};

struct DiatomicWakerTestCase;

struct Args(DiatomicWaker);

impl<'a> Arbitrary<'a> for Args {
    fn arbitrary(_u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Args(DiatomicWaker::new()))
    }
}

impl TestCase<'_> for DiatomicWakerTestCase {
    type Future<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

    type Driver<'a> = Source<'a>;

    type Args = Args;

    fn init<'a>(&self, args: &'a mut Args) -> (Self::Driver<'a>, Self::Future<'a>) {
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
