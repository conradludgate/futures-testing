//! A testing framework for [`Future`]s.
//!
//! This framework ensures that futures can always make progress. It's surprisingly easy
//! to forget to schedule the [`Waker`](std::task::Waker) when writing a future, but forgetting to do that
//! can cause your tasks to deadlock.
//!
//! Additionally, futures must be able to handle spurious wake ups, which is a common occurence
//! when running tasks within a `select`/`join`. This test framework also injects those spurious wake ups.
//!
//! ```
//! use std::ops::ControlFlow;
//! use futures_testing::{drive_poll_fn, testcase};
//! use futures::StreamExt;
//!
//! let testcase = testcase!(|_args: &mut ()| {
//!     let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);
//!
//!     // Define the driver - sends items to the channel.
//!     let driver = drive_poll_fn(move |item: u8| {
//!         match tx.try_send(item) {
//!             Ok(()) => std::task::Poll::Ready(ControlFlow::Continue(())),
//!             Err(_) => std::task::Poll::Pending,
//!         }
//!     });
//!
//!     let factory = async move |_: ()| {
//!         let _ = rx.next().await;
//!     };
//!
//!     (driver, factory)
//! });
//!
//! // Run the tests
//! futures_testing::tests(testcase).run();
//! ```

extern crate alloc;
use core::ops::ControlFlow;
use std::{pin::Pin, task::Poll};

mod driver;
mod runner;

pub use arbitrary;
use arbitrary::{Arbitrary, Unstructured};
use arbtest::{arbtest, ArbTest};
pub use driver::{drive_fn, drive_poll_fn, drive_sink, AsyncFnDriver, PollFnDriver, SinkDriver};

/// A `TestCase` defines what [`Future`] needs to be tested for wake correctness, along with the [`Driver`] that manages it.
pub trait TestCase {
    /// The args that are used to seed the current test.
    type Args<'a>: Arbitrary<'a>;

    /// The type of item passed to the factory on each invocation, generated via [`Arbitrary`].
    type FactoryItem<'a>: Arbitrary<'a>;

    /// `init` will construct a new instance of the future to test.
    ///
    /// # Implementation notes
    ///
    /// This function should be deterministic. Any randomness should be derived from the [`TestCase::Args`] or from
    /// [`Driver::Args`]. You should not use interior mutability inside of `self`.
    fn init<'a>(
        &self,
        args: &mut Self::Args<'a>,
    ) -> (impl Driver<'a>, impl AsyncFnMut(Self::FactoryItem<'a>));
}

/// A `Driver` is responsible for making a leaf future make progress.
///
/// For example:
/// * if the leaf future is the receiver of a channel, the driver could be the channel sender.
/// * if the leaf future is a timeout, the driver could be the timer system.
pub trait Driver<'a> {
    /// Drive the corresponding leaf future to make some progress.
    ///
    /// Returns:
    /// - `Poll::Ready(ControlFlow::Continue(()))` - progress made, future may be ready
    /// - `Poll::Ready(ControlFlow::Break(()))` - driver is done, exit after current future completes
    /// - `Poll::Pending` - no progress
    ///
    /// # Implementation notes
    /// This function is allowed to block.
    fn poll(
        self: Pin<&mut Self>,
        args: &mut Unstructured<'a>,
    ) -> arbitrary::Result<Poll<ControlFlow<()>>>;
}

#[macro_export]
macro_rules! testcase {
    (|$args:ident: &mut $arg_ty:ty| -> $item_ty:ty $body:block) => {{
        struct TestCase;
        impl $crate::TestCase for TestCase {
            type Args<'a> = $arg_ty;
            type FactoryItem<'a> = $item_ty;
            fn init<'a>(
                &self,
                $args: &mut $arg_ty,
            ) -> (impl $crate::Driver<'a>, impl AsyncFnMut($item_ty)) {
                $body
            }
        }
        TestCase
    }};
    (|$args:ident: &mut $arg_ty:ty| $body:expr) => {
        testcase!(|$args: &mut $arg_ty| -> () { $body })
    };
}

/// Construct the test runner for this [`TestCase`].
///
/// See [`arbtest`](mod@arbtest) for more information about how to run tests.
/// use futures_testing::{Driver, TestCase};
///
/// ```
/// use std::ops::ControlFlow;
/// use futures_testing::{drive_poll_fn, testcase};
/// use futures::StreamExt;
///
/// let testcase = testcase!(|_args: &mut ()| {
///     let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);
///
///     // Define the driver - sends items to the channel.
///     let driver = drive_poll_fn(move |item: u8| {
///         match tx.try_send(item) {
///             Ok(()) => std::task::Poll::Ready(ControlFlow::Continue(())),
///             Err(_) => std::task::Poll::Pending,
///         }
///     });
///
///     let factory = async move |_: ()| {
///         let _ = rx.next().await;
///     };
///
///     (driver, factory)
/// });
///
/// // Run the tests
/// futures_testing::tests(testcase).run();
/// ```
pub fn tests<T: TestCase>(
    mut t: T,
) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>> {
    arbtest(move |u| runner::test(&mut t, u))
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
