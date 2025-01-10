use std::{
    // collections::HashSet,
    future::Future,
    hash::Hash,
    pin::pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Wake, Waker},
};

use arbitrary::{Arbitrary, Unstructured};
use rand::{rngs::StdRng, thread_rng, Rng, SeedableRng};

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

pub fn run_test<T: TestCase>(t: &T) {
    let seed: u64 = thread_rng().gen();
    for i in 0..1000 {
        run_test_single_seed(t, i ^ seed);
    }
}

pub fn run_test_single_seed<T: TestCase>(t: &T, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    test(t, &mut rng);
}

fn test<T: TestCase>(t: &T, rng: &mut impl Rng) {
    let mut buffer = vec![];

    let Some(init_args) = get(&mut buffer, rng) else {
        return;
    };
    let (mut driver, future) = t.init(init_args);
    let mut future = pin!(future);

    let wakers = Arc::new(Wakers {
        // wakers: Mutex::new(HashSet::new()),
    });

    loop {
        let Some(poll_future) = get(&mut buffer, rng) else {
            return;
        };
        if poll_future {
            let waker = wakers.new_waker();

            if future
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(waker.clone())))
                .is_ready()
            {
                return;
            }

            if Arc::strong_count(&waker) == 1 {
                // if the strong count is 1, then the waker has not been saved.
                let woken = *waker.woken.lock().unwrap();
                if !woken {
                    panic!("Waker passed to future was lost without being woken");
                }
            }
        } else {
            let Some(args) = get(&mut buffer, rng) else {
                return;
            };
            driver.poll(args);
        }
    }
}

fn get<A: for<'a> Arbitrary<'a>, R: Rng>(buf: &mut Vec<u8>, rng: &mut R) -> Option<A> {
    loop {
        let mut unstructured = Unstructured::new(buf);
        match A::arbitrary(&mut unstructured) {
            Err(arbitrary::Error::NotEnoughData) => {}
            Ok(_t) if buf.is_empty() => {}
            Ok(t) => {
                let index = buf.len() - unstructured.len();
                buf.splice(..index, []);
                return Some(t);
            }
            // get a different seed
            Err(arbitrary::Error::IncorrectFormat) => return None,
            Err(_) => panic!("could not construct test case"),
        }

        let i = buf.len();
        buf.resize(i * 2 + 1, 0);
        rng.fill_bytes(&mut buf[i..]);
    }
}
