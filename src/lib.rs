//! A testing framework for [`Future`]s.
//!
//! This framework ensures that futures can always make progress. It's surprisingly easy
//! to forget to schedule the [`Waker`] when writing a future, but forgetting to do that
//! can cause your tasks to deadlock.
//!
//! Additionally, futures must be able to handle spurious wake ups, which is a common occurence
//! when running tasks within a `select`/`join`. This test framework also injects those spurious wake ups.
//!
//! ```
//! use std::future::Future;
//! use futures_testing::{drive_fn, Driver, TestCase};
//!
//! struct OneShotTestCase;
//!
//! // Define the test case for a oneshot channel receiver.
//! impl<'b> TestCase<'b> for OneShotTestCase {
//!     type Args = ();
//!     fn init<'a>(&self, _args: &'a mut ()) -> (impl Driver<'b>, impl Future) {
//!         let (tx, rx) = tokio::sync::oneshot::channel();
//!
//!         // Define the driver, in this case the channel sender.
//!         let mut tx = Some(tx);
//!         let driver = drive_fn(move |()| {
//!             if let Some(tx) = tx.take() {
//!                 tx.send(()).unwrap();
//!             }
//!         });
//!
//!         (driver, rx)
//!     }
//! }
//!
//! // Run the tests
//! futures_testing::tests(OneShotTestCase).run();
//! ```

use core::{
    future::Future,
    pin::pin,
    sync::atomic::AtomicBool,
    task::{Context, Waker},
};
use std::marker::PhantomData;

extern crate alloc;

use alloc::{sync::Arc, task::Wake};

use arbitrary::{Arbitrary, Unstructured};
use arbtest::{arbtest, ArbTest};

pub use arbitrary;

/// A `TestCase` defines what [`Future`] needs to be tested for wake correctness, along with the [`Driver`] that manages it.
pub trait TestCase<'b> {
    /// The args that are used to seed the current test.
    type Args: Arbitrary<'b>;

    /// `init` will construct a new instance of the future to test.
    ///
    /// # Implementation notes
    ///
    /// This function should be deterministic. Any randomness should be derived from the [`TestCase::Args`] or from
    /// [`Driver::Args`]. You should not use interior mutability inside of `self`.
    fn init<'a>(&self, args: &'a mut Self::Args) -> (impl Driver<'b>, impl Future);
}

/// A `Driver` is responsible for making a leaf future make progress.
///
/// For example:
/// * if the leaf future is the receiver of a channel, the driver could be the channel sender.
/// * if the leaf future is a timeout, the driver could be the timer system.
pub trait Driver<'b> {
    /// The args that are used to seed the next polling of this driver.
    type Args: Arbitrary<'b>;

    /// Drive the corresponding leaf future to make some progress.
    ///
    /// # Implementation notes
    /// This function is allowed to block.
    fn poll(&mut self, args: Self::Args);
}

/// See [`drive_fn`]
pub struct FnDriver<F, A>(F, PhantomData<A>);

/// A convenient method for constructing a [`Driver`] from a [`FnMut`]
pub fn drive_fn<A, F>(f: F) -> FnDriver<F, A>
where
    A: for<'b> Arbitrary<'b>,
    F: FnMut(A),
{
    FnDriver(f, PhantomData)
}

impl<'b, A, F> Driver<'b> for FnDriver<F, A>
where
    A: Arbitrary<'b>,
    F: FnMut(A),
{
    type Args = A;
    fn poll(&mut self, args: Self::Args) {
        self.0(args)
    }
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

/// Construct the test runner for this [`TestCase`].
///
/// See [`arbtest`] for more information about how to run tests.
/// use futures_testing::{Driver, TestCase};
///
/// ```
/// use std::future::Future;
/// use futures_testing::{drive_fn, Driver, TestCase};
///
/// struct OneShotTestCase;
///
/// // Define the test case for a oneshot channel receiver.
/// impl<'b> TestCase<'b> for OneShotTestCase {
///     type Args = ();
///     fn init<'a>(&self, _args: &'a mut ()) -> (impl Driver<'b>, impl Future) {
///         let (tx, rx) = tokio::sync::oneshot::channel();
///
///         // Define the driver, in this case the channel sender.
///         let mut tx = Some(tx);
///         let driver = drive_fn(move |()| {
///             if let Some(tx) = tx.take() {
///                 tx.send(()).unwrap();
///             }
///         });
///
///         (driver, rx)
///     }
/// }
///
/// // Run the tests
/// futures_testing::tests(OneShotTestCase).run();
/// ```
pub fn tests<T>(mut t: T) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>>
where
    T: for<'b> TestCase<'b>,
{
    arbtest(move |u| test(&mut t, u))
}

fn test<'b, T>(t: &mut T, u: &mut Unstructured<'b>) -> arbitrary::Result<()>
where
    T: TestCase<'b>,
{
    let mut args = u.arbitrary()?;
    let (mut driver, future) = t.init(&mut args);
    let mut future = pin!(future);

    while !u.is_empty() {
        match u.arbitrary::<Choice<_>>()? {
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

impl<'a, A: arbitrary::Arbitrary<'a>> arbitrary::Arbitrary<'a> for Choice<A> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        match <u8 as arbitrary::Arbitrary>::arbitrary(u)? % 2 {
            0 => Ok(Choice::Drive(u.arbitrary()?)),
            1 => Ok(Choice::Poll),
            _ => unreachable!(),
        }
    }

    fn arbitrary_take_rest(mut u: arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
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
            (1, Some(1)),
            arbitrary::size_hint::try_recursion_guard(depth, |depth| {
                <A as arbitrary::Arbitrary>::try_size_hint(depth)
            })?,
        ))
    }
}

/// A useful [`Arbitrary`] wrapper for if you just need the [default][`Default`] constructor
/// for some [`TestCase`] arguments.
pub struct ArbitraryDefault<T>(pub T);

impl<'a, A: Default> arbitrary::Arbitrary<'a> for ArbitraryDefault<A> {
    fn arbitrary(_u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self(A::default()))
    }

    #[inline]
    fn size_hint(_depth: usize) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}
