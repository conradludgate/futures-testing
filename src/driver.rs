use core::future::Future;
use core::marker::PhantomData;
use core::ops::{AsyncFnMut, ControlFlow};
use core::pin::pin;
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
/// their value as an opaque byte sequence while shrinking.
pub struct ArbitraryGenerator<A> {
    _arg: PhantomData<fn(A)>,
}

/// Construct a Hegel generator backed by [`Arbitrary`].
pub fn arbitrary_values<A>() -> ArbitraryGenerator<A>
where
    A: for<'a> Arbitrary<'a>,
{
    ArbitraryGenerator { _arg: PhantomData }
}

impl<A> Generator<A> for ArbitraryGenerator<A>
where
    A: for<'a> Arbitrary<'a>,
{
    fn do_draw(&self, tc: &TestCase) -> A {
        let data = tc.draw_silent(gs::binary().min_size(1).max_size(65_536));
        let mut u = Unstructured::new(&data);
        match A::arbitrary(&mut u) {
            Ok(value) => value,
            Err(_) => tc.reject(),
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
    drive_poll_fn_with(arbitrary_values(), f)
}

impl<'a, A, G, F> Driver<'a> for PollFnDriver<F, G, A>
where
    G: Generator<A>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    fn poll(self: Pin<&mut Self>, tc: &TestCase) -> Poll<ControlFlow<()>> {
        let this = self.project();
        (this.f)(tc.draw_silent(&*this.generator))
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
    drive_fn_with(arbitrary_values(), f)
}

impl<'a, A, G, F> Driver<'a> for AsyncFnDriver<F, G, A>
where
    G: Generator<A>,
    F: AsyncFnMut(A) -> ControlFlow<()> + Unpin,
{
    fn poll(self: Pin<&mut Self>, tc: &TestCase) -> Poll<ControlFlow<()>> {
        let this = self.project();
        let cx = &mut Context::from_waker(Waker::noop());
        let arg = tc.draw_silent(&*this.generator);
        let mut fut = pin!((this.f)(arg));
        fut.as_mut().poll(cx)
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
    drive_sink_with(sink, arbitrary_values())
}

impl<'a, S, G, A> Driver<'a> for SinkDriver<S, G, A>
where
    G: Generator<A>,
    S: Sink<A, Error: std::fmt::Debug>,
{
    fn poll(self: Pin<&mut Self>, tc: &TestCase) -> Poll<ControlFlow<()>> {
        let mut this = self.project();
        let mut cx = Context::from_waker(Waker::noop());

        if *this.closed {
            return Poll::Ready(ControlFlow::Break(()));
        }

        *this.closing = *this.closing || tc.draw_silent(gs::integers::<u8>()) == u8::MAX;
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

            let item = tc.draw_silent(&*this.generator);
            this.sink.as_mut().start_send(item).unwrap();
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_driver_adapter_remains_available() {
        hegel::Hegel::new(|tc| {
            let mut driver = pin!(drive_poll_fn(|_: u8| {
                Poll::Ready(ControlFlow::Continue(()))
            }));
            assert!(Driver::poll(driver.as_mut(), &tc).is_ready());
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();
    }
}
