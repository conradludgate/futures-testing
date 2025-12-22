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
//! use std::future::Future;
//! use futures_testing::{drive_fn, Driver, testcase};
//!
//! let testcase = testcase!(|_args: &mut ()| {
//!     let (tx, rx) = tokio::sync::oneshot::channel();
//!
//!     // Define the driver, in this case the channel sender.
//!     let mut tx = Some(tx);
//!     let driver = drive_fn(move |()| {
//!         if let Some(tx) = tx.take() {
//!             tx.send(()).unwrap();
//!             return std::task::Poll::Ready(()); // the receiver should be woken.
//!         }
//!         std::task::Poll::Pending
//!     });
//!
//!     (driver, rx)
//! });
//!
//! // Run the tests
//! futures_testing::tests(testcase).run();
//! ```

extern crate alloc;
use alloc::sync::Arc;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::pin;
use core::sync::atomic::AtomicBool;
use core::task::Context;
use std::{
    pin::Pin,
    sync::atomic::Ordering,
    task::{Poll, Waker},
};

pub use arbitrary;
use arbitrary::{Arbitrary, Unstructured};
use arbtest::{arbtest, ArbTest};
use futures_util::task::waker_ref;
use futures_util::Sink;

/// A `TestCase` defines what [`Future`] needs to be tested for wake correctness, along with the [`Driver`] that manages it.
pub trait TestCase {
    /// The args that are used to seed the current test.
    type Args<'a>: Arbitrary<'a>;

    /// `init` will construct a new instance of the future to test.
    ///
    /// # Implementation notes
    ///
    /// This function should be deterministic. Any randomness should be derived from the [`TestCase::Args`] or from
    /// [`Driver::Args`]. You should not use interior mutability inside of `self`.
    fn init<'a>(&self, args: &mut Self::Args<'a>) -> (impl Driver<'a>, impl Future);
}

/// A `Driver` is responsible for making a leaf future make progress.
///
/// For example:
/// * if the leaf future is the receiver of a channel, the driver could be the channel sender.
/// * if the leaf future is a timeout, the driver could be the timer system.
pub trait Driver<'a> {
    /// Drive the corresponding leaf future to make some progress.
    ///
    /// It should return [`Poll::Ready`] if the future is ready to be polled again, [`Poll::Pending`] if unknown.
    ///
    /// # Implementation notes
    /// This function is allowed to block.
    fn poll(self: Pin<&mut Self>, args: &mut Unstructured<'a>) -> arbitrary::Result<Poll<()>>;
}

pin_project_lite::pin_project!(
    /// See [`drive_fn`]
    pub struct FnDriver<F, A> {
        f: F,
        _arg: PhantomData<A>,
    }
);

/// A convenient method for constructing a [`Driver`] from a [`FnMut`]
pub fn drive_fn<A, F>(f: F) -> FnDriver<F, A>
where
    A: for<'a> Arbitrary<'a>,
    F: FnMut(A) -> Poll<()>,
{
    FnDriver {
        f,
        _arg: PhantomData,
    }
}

impl<'a, A, F> Driver<'a> for FnDriver<F, A>
where
    A: Arbitrary<'a>,
    F: FnMut(A) -> Poll<()>,
{
    fn poll(self: Pin<&mut Self>, args: &mut Unstructured<'a>) -> arbitrary::Result<Poll<()>> {
        Ok((self.project().f)(args.arbitrary()?))
    }
}

/// A convenient method for constructing a [`Driver`] from a [`Sink`]
pub fn drive_sink<A, S>(sink: S) -> SinkDriver<S, A>
where
    A: for<'a> Arbitrary<'a>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    SinkDriver {
        sink,
        closing: false,
        closed: false,
        _arg: PhantomData,
    }
}

pin_project_lite::pin_project!(
    pub struct SinkDriver<S, A> {
        #[pin]
        sink: S,
        closing: bool,
        closed: bool,
        _arg: PhantomData<A>,
    }
);

impl<'a, S, A> Driver<'a> for SinkDriver<S, A>
where
    A: Arbitrary<'a>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    fn poll(self: Pin<&mut Self>, args: &mut Unstructured<'a>) -> arbitrary::Result<Poll<()>> {
        let mut this = self.project();
        let mut cx = Context::from_waker(Waker::noop());

        if *this.closed {
            return Ok(Poll::Pending);
        }

        // rare: close the sink
        *this.closing = *this.closing || args.ratio(1u8, 255u8)?;
        if *this.closing {
            if let Poll::Ready(res) = this.sink.poll_close(&mut cx) {
                res.unwrap();
                *this.closed = true;
            }
        } else {
            let Poll::Ready(res) = this.sink.as_mut().poll_ready(&mut cx) else {
                return Ok(Poll::Pending);
            };
            res.unwrap();

            this.sink.as_mut().start_send(args.arbitrary()?).unwrap();
        }

        // we don't know if the future should be ready.
        Ok(Poll::Pending)
    }
}

#[macro_export]
macro_rules! testcase {
    (|$args:ident: &mut $arg_ty:ty| $body:expr) => {{
        struct TestCase;
        impl $crate::TestCase for TestCase {
            type Args<'a> = $arg_ty;
            fn init<'a>(&self, $args: &mut $arg_ty) -> (impl Driver<'a>, impl Future) {
                $body
            }
        }
        TestCase
    }};
}

struct TestWaker {
    woken: AtomicBool,
}

impl futures_util::task::ArcWake for TestWaker {
    fn wake_by_ref(this: &Arc<Self>) {
        this.woken.store(true, Ordering::SeqCst);
    }
}

/// Construct the test runner for this [`TestCase`].
///
/// See [`arbtest`](mod@arbtest) for more information about how to run tests.
/// use futures_testing::{Driver, TestCase};
///
/// ```
/// use std::future::Future;
/// use futures_testing::{drive_fn, Driver, testcase};
///
/// let testcase = testcase!(|_args: &mut ()| {
///     let (tx, rx) = tokio::sync::oneshot::channel();
///
///     // Define the driver, in this case the channel sender.
///     let mut tx = Some(tx);
///     let driver = drive_fn(move |()| {
///         if let Some(tx) = tx.take() {
///             tx.send(()).unwrap();
///             return std::task::Poll::Ready(()); // the receiver should be woken.
///         }
///         std::task::Poll::Pending
///     });
///
///     (driver, rx)
/// });
///
/// // Run the tests
/// futures_testing::tests(testcase).run();
/// ```
pub fn tests<T: TestCase>(
    mut t: T,
) -> ArbTest<impl FnMut(&mut Unstructured<'_>) -> arbitrary::Result<()>> {
    arbtest(move |u| test(&mut t, u))
}

fn test<T: TestCase>(t: &mut T, u: &mut Unstructured<'_>) -> arbitrary::Result<()> {
    let mut args = u.arbitrary()?;
    let (driver, future) = t.init(&mut args);
    let mut driver = pin!(driver);
    let mut future = pin!(future);
    let mut waker = Arc::new(TestWaker {
        woken: AtomicBool::new(true),
    });

    while !u.is_empty() {
        match u.arbitrary()? {
            Choice::ChangeWaker => {
                waker = Arc::new(TestWaker {
                    woken: AtomicBool::new(true),
                });
            }
            Choice::SpuriousPoll => {
                waker.woken.store(false, Ordering::SeqCst);
                if poll_fut(&mut waker, future.as_mut()).is_ready() {
                    // finished testing
                    return Ok(());
                }
            }
            Choice::Poll => {
                let woken = waker.woken.swap(false, Ordering::SeqCst);
                if woken && poll_fut(&mut waker, future.as_mut()).is_ready() {
                    // finished testing
                    return Ok(());
                }
            }
            Choice::Drive => {
                if driver.as_mut().poll(u)?.is_ready() {
                    let woken = waker.woken.load(Ordering::SeqCst);
                    assert!(woken, "future was not woken when driver made progress");
                }
            }
        }
    }

    Err(arbitrary::Error::NotEnoughData)
}

/// poll a [`Future`] and make sure the [`std::task::Waker`] was registered correctly.
fn poll_fut(waker: &mut Arc<TestWaker>, f: Pin<&mut impl Future>) -> Poll<()> {
    let waker_ref = waker_ref(waker);
    let mut cx = Context::from_waker(&waker_ref);
    if f.poll(&mut cx).is_ready() {
        // finished testing
        return Poll::Ready(());
    }

    // if we can get mut access to this waker, then it was not registered anywhere
    if let Some(waker) = Arc::get_mut(waker) {
        let woken = *waker.woken.get_mut();
        // if the waker was woken, then it's acceptable to be unregistered.
        if !woken {
            panic!("Waker passed to future was lost without being woken");
        }
    }

    Poll::Pending
}

enum Choice {
    ChangeWaker,
    SpuriousPoll,
    Poll,
    Drive,
}

impl<'a> arbitrary::Arbitrary<'a> for Choice {
    #[inline]
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // we want change waker and spurious poll to be rare.
        match <u8 as arbitrary::Arbitrary>::arbitrary(u)? {
            0 => Ok(Choice::ChangeWaker),
            1 => Ok(Choice::SpuriousPoll),
            2..=128 => Ok(Choice::Poll),
            129..=255 => Ok(Choice::Drive),
        }
    }

    #[inline]
    fn size_hint(_depth: usize) -> (usize, Option<usize>) {
        (1, Some(1))
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
