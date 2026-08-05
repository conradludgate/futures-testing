use core::future::Future;
use core::marker::PhantomData;
use core::ops::{AsyncFnMut, ControlFlow};
use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Context;
use std::{pin::Pin, task::Poll, task::Waker};

use arbitrary::{Arbitrary, Unstructured};
use futures_util::Sink;
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use crate::Driver;

/// Adapt an [`Arbitrary`] type into a Hegel generator.
///
/// Prefer a native Hegel generator when one exists. This adapter preserves
/// compatibility for types which only implement `Arbitrary`, but Hegel sees
/// their value as an opaque byte sequence while shrinking. It uses
/// [`Arbitrary::arbitrary_take_rest`], rejects data-dependent construction
/// errors, and targets the first valid draw toward a smaller byte buffer.
pub struct ArbitraryGenerator<A> {
    target_label: &'static str,
    target_pending: AtomicBool,
    _arg: PhantomData<fn(A)>,
}

/// Construct a Hegel generator backed by [`Arbitrary`].
#[must_use]
pub fn arbitrary_values<A>() -> ArbitraryGenerator<A>
where
    A: for<'a> Arbitrary<'a>,
{
    arbitrary_values_labelled("arbitrary data size")
}

#[doc(hidden)]
#[must_use]
pub fn arbitrary_values_labelled<A>(target_label: &'static str) -> ArbitraryGenerator<A>
where
    A: for<'a> Arbitrary<'a>,
{
    ArbitraryGenerator {
        target_label,
        target_pending: AtomicBool::new(true),
        _arg: PhantomData,
    }
}

impl<A> Generator<A> for ArbitraryGenerator<A>
where
    A: for<'a> Arbitrary<'a>,
{
    fn do_draw(&self, tc: &TestCase) -> A {
        let (min_size, max_size) = A::size_hint(0);
        let mut data = gs::binary().min_size(min_size);
        if let Some(max_size) = max_size {
            data = data.max_size(max_size);
        }
        let data = tc.draw_silent(data);

        // Hegel maximizes targets, so a negative size asks it to find the
        // smallest byte buffer which still constructs a valid value.
        if self.target_pending.swap(false, Ordering::Relaxed) {
            let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
            tc.target_labelled(-f64::from(size), self.target_label);
        }
        match A::arbitrary_take_rest(Unstructured::new(&data)) {
            Ok(value) => value,
            Err(arbitrary::Error::NotEnoughData | arbitrary::Error::IncorrectFormat) => tc.reject(),
            Err(arbitrary::Error::EmptyChoose) => {
                panic!("Arbitrary implementation attempted to choose from an empty collection")
            }
            Err(error) => panic!("Arbitrary implementation failed: {error}"),
        }
    }
}

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

struct SinkActionGenerator<'a, G> {
    items: &'a G,
}

impl<A, G> Generator<Option<A>> for SinkActionGenerator<'_, G>
where
    G: Generator<A>,
{
    fn do_draw(&self, tc: &TestCase) -> Option<A> {
        if tc.draw_silent(gs::integers::<u8>()) == u8::MAX {
            None
        } else {
            Some(tc.draw_silent(self.items))
        }
    }
}

pin_project_lite::pin_project!(
    /// See [`drive_sink_with`].
    pub struct SinkDriver<S, G, A> {
        #[pin]
        sink: S,
        generator: G,
        closing: bool,
        closed: bool,
        _arg: PhantomData<fn(A)>,
    }
);

/// Construct a sink driver using an explicit Hegel item generator.
pub fn drive_sink_with<A, S, G>(sink: S, generator: G) -> SinkDriver<S, G, A>
where
    G: Generator<A>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    SinkDriver {
        sink,
        generator,
        closing: false,
        closed: false,
        _arg: PhantomData,
    }
}

/// Construct a sink driver using `Arbitrary` for its items.
///
/// Prefer [`drive_sink_with`] for Hegel-native generation and shrinking.
pub fn drive_sink<A, S>(sink: S) -> SinkDriver<S, ArbitraryGenerator<A>, A>
where
    A: for<'a> Arbitrary<'a>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    drive_sink_with(
        sink,
        arbitrary_values_labelled("driver arbitrary data size"),
    )
}

impl<S, G, A> Driver<'_> for SinkDriver<S, G, A>
where
    G: Generator<A>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    type Action = Option<A>;

    fn actions(&self) -> impl Generator<Self::Action> {
        SinkActionGenerator {
            items: &self.generator,
        }
    }

    fn poll(self: Pin<&mut Self>, action: Self::Action) -> Poll<ControlFlow<()>> {
        let mut this = self.project();
        let mut cx = Context::from_waker(Waker::noop());

        if *this.closed {
            return Poll::Ready(ControlFlow::Break(()));
        }

        *this.closing = *this.closing || action.is_none();
        if *this.closing {
            if let Poll::Ready(res) = this.sink.poll_close(&mut cx) {
                res.unwrap();
                *this.closed = true;
                return Poll::Ready(ControlFlow::Break(()));
            }
        } else {
            let Poll::Ready(res) = this.sink.as_mut().poll_ready(&mut cx) else {
                return Poll::Pending;
            };
            res.unwrap();

            let Some(item) = action else {
                unreachable!();
            };
            this.sink.as_mut().start_send(item).unwrap();
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RequiresTakeRest;

    impl<'a> Arbitrary<'a> for RequiresTakeRest {
        fn arbitrary(_u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
            panic!("ordinary Arbitrary entry point was used")
        }

        fn arbitrary_take_rest(_u: Unstructured<'a>) -> arbitrary::Result<Self> {
            Ok(Self)
        }
    }

    #[test]
    fn arbitrary_driver_adapter_remains_available() {
        hegel::Hegel::new(|tc| {
            let mut driver = pin!(drive_poll_fn(|_: u8| {
                Poll::Ready(ControlFlow::Continue(()))
            }));
            let action = tc.draw_silent(driver.as_ref().get_ref().actions());
            assert!(Driver::poll(driver.as_mut(), action).is_ready());
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();
    }

    #[test]
    fn arbitrary_adapter_uses_take_rest() {
        hegel::Hegel::new(|tc| {
            let _: RequiresTakeRest = tc.draw_silent(arbitrary_values());
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();
    }
}
