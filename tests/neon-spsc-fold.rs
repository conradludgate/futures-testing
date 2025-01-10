use std::{future::Future, pin::Pin, task::Context};

use futures::task::noop_waker_ref;
use futures_testing::{Driver, TestCase};
use spsc_fold::channel;
use tokio::sync::mpsc;

struct SpscFoldRecvTestCase;

impl TestCase for SpscFoldRecvTestCase {
    type Future = Pin<Box<dyn Future<Output = u8>>>;

    type Driver = TestSender;

    type Args = ();

    fn init(&self, _args: ()) -> (Self::Driver, Self::Future) {
        let (mut sender, mut receiver) = channel();
        let (tx, mut rx) = mpsc::unbounded_channel::<u8>();
        let sender = Box::pin(async move {
            while let Some(t) = rx.recv().await {
                sender
                    .send(t, |t, u| match t.checked_add(u) {
                        Some(u) => {
                            *t = u;
                            Ok(())
                        }
                        None => Err(u),
                    })
                    .await
                    .expect("receiver should not be gone")
            }
        });
        let receiver = Box::pin(async move { receiver.recv().await.unwrap() });

        (TestSender(tx, sender), receiver)
    }
}

struct TestSender(mpsc::UnboundedSender<u8>, Pin<Box<dyn Future<Output = ()>>>);

impl Driver for TestSender {
    type Args = u8;

    fn poll(&mut self, args: u8) {
        self.0.send(args).unwrap();
        assert!(self
            .1
            .as_mut()
            .poll(&mut Context::from_waker(noop_waker_ref()))
            .is_pending());
    }
}

#[test]
fn check_recv() {
    futures_testing::tests(SpscFoldRecvTestCase).run();
}

struct SpscFoldSendTestCase;

impl TestCase for SpscFoldSendTestCase {
    type Future = Pin<Box<dyn Future<Output = ()>>>;

    type Driver = TestReceiver;

    type Args = Vec<u8>;

    fn init(&self, args: Vec<u8>) -> (Self::Driver, Self::Future) {
        let (mut sender, mut receiver) = channel();
        let sender = Box::pin(async move {
            for t in args {
                sender
                    .send(t, |t, u| match t.checked_add(u) {
                        Some(u) => {
                            *t = u;
                            Ok(())
                        }
                        None => Err(u),
                    })
                    .await
                    .expect("receiver should not be gone")
            }
        });
        let receiver = Box::pin(async move {
            loop {
                receiver.recv().await.unwrap();
            }
        });

        (TestReceiver(receiver), sender)
    }
}

struct TestReceiver(Pin<Box<dyn Future<Output = ()>>>);

impl Driver for TestReceiver {
    type Args = ();

    fn poll(&mut self, _args: ()) {
        assert!(self
            .0
            .as_mut()
            .poll(&mut Context::from_waker(noop_waker_ref()))
            .is_pending());
    }
}

#[test]
#[should_panic = "Waker passed to future was lost without being woken"]
fn check_send() {
    futures_testing::tests(SpscFoldSendTestCase).run();
}

mod spsc_fold {
    //! Taken from <https://github.com/neondatabase/neon/blob/735c66dc65f1163591a2745934f4be766072c88c/libs/utils/src/sync/spsc_fold.rs>

    use core::{future::poll_fn, task::Poll};
    use std::sync::{Arc, Mutex};

    use diatomic_waker::DiatomicWaker;

    pub struct Sender<T> {
        state: Arc<Inner<T>>,
    }

    pub struct Receiver<T> {
        state: Arc<Inner<T>>,
    }

    struct Inner<T> {
        wake_receiver: DiatomicWaker,
        wake_sender: DiatomicWaker,
        value: Mutex<State<T>>,
    }

    enum State<T> {
        NoData,
        HasData(T),
        TryFoldFailed, // transient state
        SenderWaitsForReceiverToConsume(T),
        SenderGone(Option<T>),
        ReceiverGone,
        AllGone,
        SenderDropping,   // transient state
        ReceiverDropping, // transient state
    }

    pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
        let inner = Inner {
            wake_receiver: DiatomicWaker::new(),
            wake_sender: DiatomicWaker::new(),
            value: Mutex::new(State::NoData),
        };

        let state = Arc::new(inner);
        (
            Sender {
                state: state.clone(),
            },
            Receiver { state },
        )
    }

    #[derive(Debug)]
    pub enum SendError {
        ReceiverGone,
    }

    impl<T: Send> Sender<T> {
        /// # Panics
        ///
        /// If `try_fold` panics,  any subsequent call to `send` panic.
        pub async fn send<F>(&mut self, value: T, try_fold: F) -> Result<(), SendError>
        where
            F: Fn(&mut T, T) -> Result<(), T>,
        {
            let mut value = Some(value);
            poll_fn(|cx| {
                let mut guard = self.state.value.lock().unwrap();
                match &mut *guard {
                    State::NoData => {
                        *guard = State::HasData(value.take().unwrap());
                        self.state.wake_receiver.notify();
                        Poll::Ready(Ok(()))
                    }
                    State::HasData(_) => {
                        let State::HasData(acc_mut) = &mut *guard else {
                            unreachable!("this match arm guarantees that the guard is HasData");
                        };
                        match try_fold(acc_mut, value.take().unwrap()) {
                            Ok(()) => {
                                // no need to wake receiver, if it was waiting it already
                                // got a wake-up when we transitioned from NoData to HasData
                                Poll::Ready(Ok(()))
                            }
                            Err(unfoldable_value) => {
                                value = Some(unfoldable_value);
                                let State::HasData(acc) =
                                    std::mem::replace(&mut *guard, State::TryFoldFailed)
                                else {
                                    unreachable!(
                                        "this match arm guarantees that the guard is HasData"
                                    );
                                };
                                *guard = State::SenderWaitsForReceiverToConsume(acc);
                                // SAFETY: send is single threaded due to `&mut self` requirement,
                                // therefore register is not concurrent.
                                unsafe {
                                    self.state.wake_sender.register(cx.waker());
                                }
                                Poll::Pending
                            }
                        }
                    }
                    State::SenderWaitsForReceiverToConsume(_data) => {
                        // Really, we shouldn't be polled until receiver has consumed and wakes us.
                        Poll::Pending
                    }
                    State::ReceiverGone => Poll::Ready(Err(SendError::ReceiverGone)),
                    State::SenderGone(_)
                    | State::AllGone
                    | State::SenderDropping
                    | State::ReceiverDropping
                    | State::TryFoldFailed => {
                        unreachable!();
                    }
                }
            })
            .await
        }
    }

    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            scopeguard::defer! {
                self.state.wake_receiver.notify()
            };
            let Ok(mut guard) = self.state.value.lock() else {
                return;
            };
            *guard = match std::mem::replace(&mut *guard, State::SenderDropping) {
                State::NoData => State::SenderGone(None),
                State::HasData(data) | State::SenderWaitsForReceiverToConsume(data) => {
                    State::SenderGone(Some(data))
                }
                State::ReceiverGone => State::AllGone,
                State::TryFoldFailed
                | State::SenderGone(_)
                | State::AllGone
                | State::SenderDropping
                | State::ReceiverDropping => {
                    unreachable!("unreachable state {:?}", guard.discriminant_str())
                }
            }
        }
    }

    #[derive(Debug)]
    pub enum RecvError {
        SenderGone,
    }

    impl<T: Send> Receiver<T> {
        pub async fn recv(&mut self) -> Result<T, RecvError> {
            poll_fn(|cx| {
                let mut guard = self.state.value.lock().unwrap();
                match &mut *guard {
                    State::NoData => {
                        // SAFETY: recv is single threaded due to `&mut self` requirement,
                        // therefore register is not concurrent.
                        unsafe {
                            self.state.wake_receiver.register(cx.waker());
                        }
                        Poll::Pending
                    }
                    guard @ State::HasData(_)
                    | guard @ State::SenderWaitsForReceiverToConsume(_)
                    | guard @ State::SenderGone(Some(_)) => {
                        let data = guard
                            .take_data()
                            .expect("in these states, data is guaranteed to be present");
                        self.state.wake_sender.notify();
                        Poll::Ready(Ok(data))
                    }
                    State::SenderGone(None) => Poll::Ready(Err(RecvError::SenderGone)),
                    State::ReceiverGone
                    | State::AllGone
                    | State::SenderDropping
                    | State::ReceiverDropping
                    | State::TryFoldFailed => {
                        unreachable!("unreachable state {:?}", guard.discriminant_str());
                    }
                }
            })
            .await
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            scopeguard::defer! {
                self.state.wake_sender.notify()
            };
            let Ok(mut guard) = self.state.value.lock() else {
                return;
            };
            *guard = match std::mem::replace(&mut *guard, State::ReceiverDropping) {
                State::NoData => State::ReceiverGone,
                State::HasData(_) | State::SenderWaitsForReceiverToConsume(_) => {
                    State::ReceiverGone
                }
                State::SenderGone(_) => State::AllGone,
                State::TryFoldFailed
                | State::ReceiverGone
                | State::AllGone
                | State::SenderDropping
                | State::ReceiverDropping => {
                    unreachable!("unreachable state {:?}", guard.discriminant_str())
                }
            }
        }
    }

    impl<T> State<T> {
        fn take_data(&mut self) -> Option<T> {
            match self {
                State::HasData(_) => {
                    let State::HasData(data) = std::mem::replace(self, State::NoData) else {
                        unreachable!("this match arm guarantees that the state is HasData");
                    };
                    Some(data)
                }
                State::SenderWaitsForReceiverToConsume(_) => {
                    let State::SenderWaitsForReceiverToConsume(data) =
                        std::mem::replace(self, State::NoData)
                    else {
                        unreachable!(
                        "this match arm guarantees that the state is SenderWaitsForReceiverToConsume"
                    );
                    };
                    Some(data)
                }
                State::SenderGone(data) => Some(data.take().unwrap()),
                State::NoData
                | State::TryFoldFailed
                | State::ReceiverGone
                | State::AllGone
                | State::SenderDropping
                | State::ReceiverDropping => None,
            }
        }
        fn discriminant_str(&self) -> &'static str {
            match self {
                State::NoData => "NoData",
                State::HasData(_) => "HasData",
                State::TryFoldFailed => "TryFoldFailed",
                State::SenderWaitsForReceiverToConsume(_) => "SenderWaitsForReceiverToConsume",
                State::SenderGone(_) => "SenderGone",
                State::ReceiverGone => "ReceiverGone",
                State::AllGone => "AllGone",
                State::SenderDropping => "SenderDropping",
                State::ReceiverDropping => "ReceiverDropping",
            }
        }
    }
}
