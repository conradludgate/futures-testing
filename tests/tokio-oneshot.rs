use futures_testing::{Driver, TestCase};

struct OneShotTestCase;

impl TestCase for OneShotTestCase {
    type Future = tokio::sync::oneshot::Receiver<()>;

    type Driver = OneShotSender;

    type Args = ();

    fn init(&self, _args: ()) -> (Self::Driver, Self::Future) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (OneShotSender(Some(tx)), rx)
    }
}

struct OneShotSender(Option<tokio::sync::oneshot::Sender<()>>);

impl Driver for OneShotSender {
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
