use std::{
    // collections::HashSet,
    future::Future,
    hash::Hash,
    pin::pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Wake, Waker},
};

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

struct Wakers {
    // wakers: Mutex<HashSet<WeakWaker>>,
}

impl Wakers {
    fn new_waker(self: &Arc<Self>) -> Arc<TestWaker> {
        let waker = Arc::new(TestWaker {
            // wakers: self.clone(),
            woken: Mutex::new(false),
        });
        // let weak = WeakWaker(Arc::downgrade(&waker));
        // self.wakers.lock().unwrap().insert(weak);
        waker
    }
}

struct TestWaker {
    // wakers: Arc<Wakers>,
    woken: Mutex<bool>,
}

impl Wake for TestWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.woken.lock().unwrap() = true;
    }
}

// struct WeakWaker(Weak<TestWaker>);

// impl Hash for WeakWaker {
//     fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
//         self.0.as_ptr().hash(state);
//     }
// }

// impl PartialEq for WeakWaker {
//     fn eq(&self, other: &Self) -> bool {
//         Weak::ptr_eq(&self.0, &other.0)
//     }
// }

// impl Eq for WeakWaker {}

pub fn tests<T: TestCase>(
    t: T,
) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>> {
    arbtest(move |u| test(&t, u))
}

fn test<T: TestCase>(t: &T, u: &mut Unstructured) -> arbitrary::Result<()> {
    let (mut driver, future) = t.init(u.arbitrary()?);
    let mut future = pin!(future);

    let wakers = Arc::new(Wakers {
        // wakers: Mutex::new(HashSet::new()),
    });

    u.arbitrary_loop(None, None, |u| {
        if u.arbitrary()? {
            let waker = wakers.new_waker();

            if future
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(waker.clone())))
                .is_ready()
            {
                // finished testing
                return Ok(std::ops::ControlFlow::Break(()));
            }

            if Arc::strong_count(&waker) == 1 {
                // if the strong count is 1, then the waker has not been saved.
                let woken = *waker.woken.lock().unwrap();
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
