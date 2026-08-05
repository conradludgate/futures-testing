use core::future::Future;
use core::marker::PhantomData;
use core::ops::{AsyncFnMut, ControlFlow};
use core::pin::pin;
use core::task::Context;
use std::{pin::Pin, task::Poll, task::Waker};

use arbitrary::Arbitrary;
use hegel::generators::Generator;

use super::{ArbitraryGenerator, arbitrary_values_labelled};
use crate::Driver;

pin_project_lite::pin_project!(
    /// See [`drive_poll_fn_with`].
    pub struct PollFnDriver<F, G, A> {
        f: F,
        generator: G,
        _arg: PhantomData<fn(A)>,
    }
);

/// Construct a synchronous driver using an explicit Hegel generator.
pub fn drive_poll_fn_with<A, G, F>(generator: G, f: F) -> PollFnDriver<F, G, A>
where
    G: Generator<A>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    PollFnDriver {
        f,
        generator,
        _arg: PhantomData,
    }
}

/// Construct a synchronous driver using `Arbitrary` for its input.
///
/// Prefer [`drive_poll_fn_with`] for Hegel-native generation and shrinking.
pub fn drive_poll_fn<A, F>(f: F) -> PollFnDriver<F, ArbitraryGenerator<A>, A>
where
    A: for<'a> Arbitrary<'a>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    drive_poll_fn_with(arbitrary_values_labelled("driver arbitrary data size"), f)
}

impl<A, G, F> Driver<'_> for PollFnDriver<F, G, A>
where
    G: Generator<A>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    type Action = A;

    fn actions(&self) -> impl Generator<Self::Action> {
        &self.generator
    }

    fn poll(self: Pin<&mut Self>, action: Self::Action) -> Poll<ControlFlow<()>> {
        let this = self.project();
        (this.f)(action)
    }
}

pin_project_lite::pin_project!(
    /// See [`drive_fn_with`].
    pub struct AsyncFnDriver<F, G, A> {
        f: F,
        generator: G,
        _arg: PhantomData<fn(A)>,
    }
);

/// Construct an asynchronous driver using an explicit Hegel generator.
pub fn drive_fn_with<A, G, F>(generator: G, f: F) -> AsyncFnDriver<F, G, A>
where
    G: Generator<A>,
    F: AsyncFnMut(A) -> ControlFlow<()>,
{
    AsyncFnDriver {
        f,
        generator,
        _arg: PhantomData,
    }
}

/// Construct an asynchronous driver using `Arbitrary` for its input.
///
/// Prefer [`drive_fn_with`] for Hegel-native generation and shrinking.
pub fn drive_fn<A, F>(f: F) -> AsyncFnDriver<F, ArbitraryGenerator<A>, A>
where
    A: for<'a> Arbitrary<'a>,
    F: AsyncFnMut(A) -> ControlFlow<()>,
{
    drive_fn_with(arbitrary_values_labelled("driver arbitrary data size"), f)
}

impl<A, G, F> Driver<'_> for AsyncFnDriver<F, G, A>
where
    G: Generator<A>,
    F: AsyncFnMut(A) -> ControlFlow<()> + Unpin,
{
    type Action = A;

    fn actions(&self) -> impl Generator<Self::Action> {
        &self.generator
    }

    fn poll(self: Pin<&mut Self>, action: Self::Action) -> Poll<ControlFlow<()>> {
        let this = self.project();
        let cx = &mut Context::from_waker(Waker::noop());
        let mut fut = pin!((this.f)(action));
        fut.as_mut().poll(cx)
    }
}
