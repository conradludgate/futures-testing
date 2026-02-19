use core::future::Future;
use core::marker::PhantomData;
use core::ops::{AsyncFnMut, ControlFlow};
use core::pin::pin;
use core::task::Context;
use std::{
    pin::Pin,
    task::{Poll, Waker},
};

use arbitrary::{Arbitrary, Unstructured};
use futures_util::Sink;

use crate::Driver;

pin_project_lite::pin_project!(
    /// See [`drive_poll_fn`]
    pub struct PollFnDriver<F, A> {
        f: F,
        _arg: PhantomData<A>,
    }
);

/// Construct a [`Driver`] from a synchronous [`FnMut`].
///
/// Use this when the driver logic doesn't need `.await` (e.g. `try_send` on a
/// channel).
///
/// The function receives an arbitrary argument and returns `Poll<ControlFlow<()>>`:
/// - `Poll::Ready(ControlFlow::Continue(()))` - progress made
/// - `Poll::Ready(ControlFlow::Break(()))` - driver is done
/// - `Poll::Pending` - no progress
pub fn drive_poll_fn<A, F>(f: F) -> PollFnDriver<F, A>
where
    A: for<'a> Arbitrary<'a>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    PollFnDriver {
        f,
        _arg: PhantomData,
    }
}

impl<'a, A, F> Driver<'a> for PollFnDriver<F, A>
where
    A: Arbitrary<'a>,
    F: FnMut(A) -> Poll<ControlFlow<()>>,
{
    fn poll(
        self: Pin<&mut Self>,
        args: &mut Unstructured<'a>,
    ) -> arbitrary::Result<Poll<ControlFlow<()>>> {
        Ok((self.project().f)(args.arbitrary()?))
    }
}

/// See [`drive_fn`]
pub struct AsyncFnDriver<F, A> {
    f: F,
    _arg: PhantomData<fn(A)>,
}

/// Construct a [`Driver`] from an [`AsyncFnMut`].
///
/// Use this when the driver needs `.await` (e.g. `tx.send(item).await`).
///
/// The async function receives an arbitrary argument and returns `ControlFlow<()>`:
/// - `ControlFlow::Continue(())` - progress made, future should be polled
/// - `ControlFlow::Break(())` - driver is done
pub fn drive_fn<A, F>(f: F) -> AsyncFnDriver<F, A>
where
    A: for<'a> Arbitrary<'a>,
    F: AsyncFnMut(A) -> ControlFlow<()>,
{
    AsyncFnDriver {
        f,
        _arg: PhantomData,
    }
}

impl<'a, A, F> Driver<'a> for AsyncFnDriver<F, A>
where
    A: Arbitrary<'a>,
    F: AsyncFnMut(A) -> ControlFlow<()> + Unpin,
{
    fn poll(
        self: Pin<&mut Self>,
        args: &mut Unstructured<'a>,
    ) -> arbitrary::Result<Poll<ControlFlow<()>>> {
        let this = self.get_mut();
        let cx = &mut Context::from_waker(Waker::noop());
        let arg: A = args.arbitrary()?;
        let mut fut = pin!((this.f)(arg));
        match fut.as_mut().poll(cx) {
            Poll::Ready(cf) => Ok(Poll::Ready(cf)),
            Poll::Pending => Ok(Poll::Pending),
        }
    }
}

/// Construct a [`Driver`] from a [`Sink`].
///
/// Use this when the driver side already implements [`Sink`] (e.g. the sender
/// half of a `futures::channel::mpsc`). Handles `poll_ready`, `start_send`,
/// and `poll_close` automatically.
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
    fn poll(
        self: Pin<&mut Self>,
        args: &mut Unstructured<'a>,
    ) -> arbitrary::Result<Poll<ControlFlow<()>>> {
        let mut this = self.project();
        let mut cx = Context::from_waker(Waker::noop());

        if *this.closed {
            return Ok(Poll::Ready(ControlFlow::Break(())));
        }

        // rare: close the sink
        *this.closing = *this.closing || args.ratio(1u8, 255u8)?;
        if *this.closing {
            if let Poll::Ready(res) = this.sink.poll_close(&mut cx) {
                res.unwrap();
                *this.closed = true;
                return Ok(Poll::Ready(ControlFlow::Break(())));
            }
        } else {
            let Poll::Ready(res) = this.sink.as_mut().poll_ready(&mut cx) else {
                return Ok(Poll::Pending);
            };
            res.unwrap();

            this.sink.as_mut().start_send(args.arbitrary()?).unwrap();
        }

        Ok(Poll::Pending)
    }
}
