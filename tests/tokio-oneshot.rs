use std::future::Future;

use futures_testing::{drive_fn, Driver, TestCase};

struct OneShotTestCase;

impl<'b> TestCase<'b> for OneShotTestCase {
    type Args = ();

    fn init<'a>(&self, _args: &'a mut ()) -> (impl Driver<'b>, impl Future) {
        let (tx, rx) = tokio::sync::oneshot::channel();

        let mut tx = Some(tx);
        let driver = drive_fn(move |()| {
            if let Some(tx) = tx.take() {
                tx.send(()).unwrap();
            }
        });

        let future = rx;

        (driver, future)
    }
}

#[test]
fn oneshot() {
    futures_testing::tests(OneShotTestCase).run();
}
