use core::{
    future::Future,
    pin::pin,
    sync::atomic::AtomicBool,
    task::{Context, Waker},
};

extern crate alloc;

use alloc::{sync::Arc, task::Wake};

use arbitrary::{Arbitrary, Unstructured};
use arbtest::{arbtest, ArbTest};

pub trait TestCase {
    type Future: Future;
    type Driver: Driver;
    type Args: for<'a> Arbitrary<'a>;

    fn init(&self, args: Self::Args) -> (Self::Driver, Self::Future);
}

pub trait Driver {
    type Args: for<'a> Arbitrary<'a>;

    fn poll(&mut self, args: Self::Args);
}

struct TestWaker {
    woken: AtomicBool,
}

impl Wake for TestWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub fn tests<T: TestCase>(
    t: T,
) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>> {
    arbtest(move |u| test(&t, u))
}

fn test<T: TestCase>(t: &T, u: &mut Unstructured) -> arbitrary::Result<()> {
    let (mut driver, future) = t.init(u.arbitrary()?);
    let mut future = pin!(future);

    u.arbitrary_loop(None, None, |u| {
        if u.arbitrary()? {
            let mut waker = Arc::new(TestWaker {
                woken: AtomicBool::new(false),
            });

            if future
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(waker.clone())))
                .is_ready()
            {
                // finished testing
                return Ok(std::ops::ControlFlow::Break(()));
            }

            // if we can get mut access to this waker, then it was not registered anywhere
            if let Some(waker) = Arc::get_mut(&mut waker) {
                let woken = *waker.woken.get_mut();
                // if the waker was woken, then it's acceptable to be unregistered.
                if !woken {
                    panic!("Waker passed to future was lost without being woken");
                }
            }
        } else {
            driver.poll(u.arbitrary()?);
        }

        Ok(std::ops::ControlFlow::Continue(()))
    })
}
