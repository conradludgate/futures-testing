use futures_testing::{Driver, TestCase};

struct OneShotTestCase;

impl TestCase<'_> for OneShotTestCase {
    type Future<'a> = tokio::sync::oneshot::Receiver<()>;

    type Driver<'a> = OneShotSender;

    type Args = ();

    fn init<'a>(&self, _args: &'a mut ()) -> (Self::Driver<'a>, Self::Future<'a>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (OneShotSender(Some(tx)), rx)
    }
}

struct OneShotSender(Option<tokio::sync::oneshot::Sender<()>>);

impl Driver<'_> for OneShotSender {
    type Args = ();

    fn poll(&mut self, args: ()) {
        if let Some(tx) = self.0.take() {
            tx.send(args).unwrap();
        }
    }
}

#[test]
fn oneshot() {
    futures_testing::tests(OneShotTestCase).run();
}
