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

    for choice in u.arbitrary_iter::<Choice<_>>()? {
        match choice? {
            Choice::Poll => {
                let mut waker = Arc::new(TestWaker {
                    woken: AtomicBool::new(false),
                });

                if future
                    .as_mut()
                    .poll(&mut Context::from_waker(&Waker::from(waker.clone())))
                    .is_ready()
                {
                    // finished testing
                    return Ok(());
                }

                // if we can get mut access to this waker, then it was not registered anywhere
                if let Some(waker) = Arc::get_mut(&mut waker) {
                    let woken = *waker.woken.get_mut();
                    // if the waker was woken, then it's acceptable to be unregistered.
                    if !woken {
                        panic!("Waker passed to future was lost without being woken");
                    }
                }
            }
            Choice::Drive(args) => driver.poll(args),
        }
    }

    Err(arbitrary::Error::NotEnoughData)
}

enum Choice<A> {
    Drive(A),
    Poll,
}

#[automatically_derived]
impl<'arbitrary, A: arbitrary::Arbitrary<'arbitrary>> arbitrary::Arbitrary<'arbitrary>
    for Choice<A>
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'arbitrary>) -> arbitrary::Result<Self> {
        match <u8 as arbitrary::Arbitrary>::arbitrary(u)? % 2 {
            0 => Ok(Choice::Drive(arbitrary::Arbitrary::arbitrary(u)?)),
            1 => Ok(Choice::Poll),
            _ => unreachable!(),
        }
    }

    fn arbitrary_take_rest(mut u: arbitrary::Unstructured<'arbitrary>) -> arbitrary::Result<Self> {
        match <u8 as arbitrary::Arbitrary>::arbitrary(&mut u)? % 2 {
            0 => Ok(Choice::Drive(arbitrary::Arbitrary::arbitrary_take_rest(u)?)),
            1 => Ok(Choice::Poll),
            _ => unreachable!(),
        }
    }

    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        Self::try_size_hint(depth).unwrap_or_default()
    }

    #[inline]
    fn try_size_hint(
        depth: usize,
    ) -> Result<(usize, Option<usize>), arbitrary::MaxRecursionReached> {
        Ok(arbitrary::size_hint::and(
            <u8 as arbitrary::Arbitrary>::try_size_hint(depth)?,
            arbitrary::size_hint::try_recursion_guard(depth, |depth| {
                Ok(arbitrary::size_hint::or_all(&[
                    Ok(arbitrary::size_hint::and_all(&[
                        <A as arbitrary::Arbitrary>::try_size_hint(depth)?,
                    ]))?,
                    Ok(arbitrary::size_hint::and_all(&[]))?,
                ]))
            })?,
        ))
    }
}
