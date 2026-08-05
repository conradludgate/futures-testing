use core::marker::PhantomData;
use core::ops::ControlFlow;
use core::task::Context;
use std::{pin::Pin, task::Poll, task::Waker};

use arbitrary::Arbitrary;
use futures_util::Sink;
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::{ArbitraryGenerator, arbitrary_values_labelled};
use crate::Driver;

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
