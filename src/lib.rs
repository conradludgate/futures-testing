//! A property-based testing framework for [`Future`]s.
//!
//! # What it tests
//!
//! - **Waker registration** -- a future that returns `Pending` must retain or wake the
//!   [`Waker`](std::task::Waker) it was given. Forgetting it causes deadlocks.
//! - **Waker freshness** -- a future must accept a new waker on every poll, not cache
//!   a stale one.
//! - **Spurious wakeup tolerance** -- futures must handle being polled without the
//!   driver having made progress, as commonly happens inside `select`/`join`.
//! - **Cancel safety** -- the factory is called multiple times, exercising
//!   cancellation between iterations.
//!
//! # Architecture
//!
//! A test is defined by two collaborating pieces returned from [`TestCase::init`]:
//!
//! ```text
//!   Driver -----> Leaf Future <----- Future
//!  (e.g. tx)     (e.g. channel)    (from factory)
//!       |                              ^
//!       |                              |
//!       +--- drives progress     factory called
//!            wakes waker         multiple times
//!
//!   Runner randomly interleaves:
//!     poll | drive | spurious poll | swap waker | cancel
//! ```
//!
//! - **Driver** -- represents the other side of the leaf future under test (e.g. a
//!   channel sender for a receiver future). When it reports progress
//!   (`Poll::Ready`), the framework asserts the future's waker was called.
//!   `Poll::Pending` means no progress was made and skips that assertion.
//! - **Factory** -- an async closure called multiple times to produce the futures
//!   under test. Each call may receive an arbitrary item (see [`TestCase::FactoryItem`]).
//!
//! Under the hood the runner uses [`arbtest`] to fuzz the interleaving of these
//! actions, catching waker bugs that deterministic tests would miss.
//!
//! # Example
//!
//! ```
//! use std::ops::ControlFlow;
//! use futures_testing::{drive_poll_fn, testcase};
//! use futures::StreamExt;
//!
//! let testcase = testcase!(|| {
//!     let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);
//!
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

/// Defines a future to test for waker correctness, along with the [`Driver`] that
/// makes it progress.
///
/// Use the [`testcase!`] macro for a concise way to implement this trait.
pub trait TestCase {
    /// Shared state constructed once per test iteration. Both the driver and the
    /// factory close over references to this value.
    ///
    /// Use `()` when no shared state is needed, or [`ArbitraryDefault<T>`] when you
    /// need a `Default`-constructed `T` that doesn't implement [`Arbitrary`].
    type Args<'a>: Arbitrary<'a>;

    /// An arbitrary value passed to the factory on each invocation. Use `()` if the
    /// future under test doesn't need external input; use a concrete type (e.g. `u8`)
    /// when the future itself consumes data.
    type FactoryItem<'a>: Arbitrary<'a>;

    /// Construct a ([`Driver`], factory) pair for one test iteration.
    ///
    /// The factory is an async closure that will be called multiple times per
    /// iteration, each time receiving a new [`FactoryItem`](Self::FactoryItem).
    /// Cancellation between calls exercises cancel-safety.
    ///
    /// This function should be deterministic -- derive any randomness from `args`.
    fn init<'a>(
        &self,
        args: &mut Self::Args<'a>,
    ) -> (impl Driver<'a>, impl AsyncFnMut(Self::FactoryItem<'a>));
}

/// The other side of the leaf future under test, responsible for making it
/// progress.
///
/// For example:
/// * if the leaf future is the receiver of a channel, the driver is the sender.
/// * if the leaf future is a timeout, the driver is the timer system.
///
/// See [`drive_poll_fn`], [`drive_fn`], and [`drive_sink`] for convenient
/// constructors.
pub trait Driver<'a> {
    /// Drive the leaf future to make progress.
    ///
    /// **Key invariant:** when this returns `Poll::Ready`, the framework asserts
    /// that the future's waker was called. Return `Poll::Pending` if no progress
    /// was made (e.g. the channel is full) to skip that assertion.
    ///
    /// - `Poll::Ready(ControlFlow::Continue(()))` -- progress made
    /// - `Poll::Ready(ControlFlow::Break(()))` -- driver is done, exit after
    ///   current future completes
    /// - `Poll::Pending` -- no progress
    ///
    /// This function is allowed to block.
    fn poll(
        self: Pin<&mut Self>,
        args: &mut Unstructured<'a>,
    ) -> arbitrary::Result<Poll<ControlFlow<()>>>;
}

/// Shorthand for implementing [`TestCase`].
///
/// Four forms are supported:
///
/// ```ignore
/// // No shared state, FactoryItem defaults to ()
/// testcase!(|| {
///     // ... return (driver, factory)
/// })
///
/// // No shared state, explicit FactoryItem
/// testcase!(|| -> ItemType {
///     // ... return (driver, factory)
/// })
///
/// // With shared state, FactoryItem defaults to ()
/// testcase!(|args: &mut ArgsType| {
///     // ... return (driver, factory)
/// })
///
/// // With shared state, explicit FactoryItem
/// testcase!(|args: &mut ArgsType| -> ItemType {
///     // ... return (driver, factory)
/// })
/// ```
///
/// The body must return a `(Driver, factory)` tuple where `factory` is an
/// `async move |item: ItemType| { ... }` closure.
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
    (|| -> $item_ty:ty $body:block) => {
        testcase!(|_args: &mut ()| -> $item_ty $body)
    };
    (|| $body:expr) => {
        testcase!(|_args: &mut ()| -> () { $body })
    };
}

/// Construct the test runner for this [`TestCase`].
///
/// Returns an [`ArbTest`] runner -- see [`arbtest`](mod@arbtest) for
/// configuration options like `.seed()` and `.budget_ms()`.
pub fn tests<T: TestCase>(
    mut t: T,
) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>> {
    arbtest(move |u| runner::test(&mut t, u))
}

/// An [`Arbitrary`] wrapper that constructs `T` via [`Default`].
///
/// [`TestCase::Args`] must implement [`Arbitrary`], but shared-state types like
/// `AtomicBool` or `Mutex<Option<Waker>>` typically don't. Wrap them in
/// `ArbitraryDefault` to bridge the gap:
///
/// ```ignore
/// testcase!(|args: &mut ArbitraryDefault<AtomicBool>| {
///     let ready = &args.0;
///     // ...
/// })
/// ```
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
