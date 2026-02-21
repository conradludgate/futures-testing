use alloc::sync::Arc;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::AtomicBool;
use core::task::Context;
use std::{pin::Pin, sync::atomic::Ordering, task::Poll};

use arbitrary::{Arbitrary, Unstructured};
use futures_util::task::waker_ref;

use crate::{Driver, TestCase};

macro_rules! trace {
    ($($tt:tt)*) => {
        #[cfg(feature = "tracing")]
        tracing::trace!($($tt)*)
    };
}

struct TestWaker {
    woken: AtomicBool,
}

impl futures_util::task::ArcWake for TestWaker {
    fn wake_by_ref(this: &Arc<Self>) {
        this.woken.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn test<T: TestCase>(t: &mut T, u: &mut Unstructured<'_>) -> arbitrary::Result<()> {
    let mut args = u.arbitrary()?;
    let completions_needed = u.int_in_range(1..=8)?;
    let (driver, mut factory) = t.init(&mut args);
    let mut driver = pin!(driver);
    let mut waker = Arc::new(TestWaker {
        woken: AtomicBool::new(true),
    });
    let mut completions = 0;
    let mut driver_done = false;

    while completions < completions_needed && !driver_done {
        #[cfg(feature = "tracing")]
        let _span =
            tracing::trace_span!("iteration", iteration = completions + 1, completions_needed)
                .entered();

        let mut future = pin!(factory(u.arbitrary()?));
        let mut v: ChoiceState = u.arbitrary()?;

        loop {
            match v.next()? {
                Choice::ChangeWaker => {
                    if !waker.woken.swap(false, Ordering::SeqCst) {
                        continue;
                    }
                    trace!("change_waker");
                    waker = Arc::new(TestWaker {
                        woken: AtomicBool::new(true),
                    });
                }
                Choice::Poll { spurious } => {
                    let was_woken = waker.woken.swap(false, Ordering::SeqCst);
                    if !(was_woken || spurious) {
                        continue;
                    }
                    let poll = poll_fut(&mut waker, future.as_mut());
                    trace!(
                        spurious = spurious & !was_woken,
                        ready = poll.is_ready(),
                        "poll"
                    );
                    if poll.is_ready() {
                        completions += 1;
                        waker.woken.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Choice::Drive => {
                    let poll = driver.as_mut().poll(u)?;
                    trace!(
                        ready = poll.is_ready(),
                        done = matches!(poll, Poll::Ready(cf) if cf.is_break()),
                        "drive"
                    );
                    if let Poll::Ready(cf) = poll {
                        let woken = waker.woken.load(Ordering::SeqCst);
                        assert!(woken, "future was not woken when driver made progress");
                        if cf.is_break() {
                            driver_done = true;
                        }
                    }
                }
                Choice::Cancel => {
                    trace!("cancel");
                    break;
                }
            }

            v = u.arbitrary()?;
        }
    }

    Ok(())
}

fn poll_fut(waker: &mut Arc<TestWaker>, f: Pin<&mut impl Future>) -> Poll<()> {
    let waker_ref = waker_ref(waker);
    let mut cx = Context::from_waker(&waker_ref);
    if f.poll(&mut cx).is_ready() {
        return Poll::Ready(());
    }

    if let Some(waker) = Arc::get_mut(waker) {
        let woken = *waker.woken.get_mut();
        if !woken {
            panic!("Waker passed to future was lost without being woken");
        }
    }

    Poll::Pending
}

#[derive(PartialEq, Debug)]
enum Choice {
    ChangeWaker,
    Poll { spurious: bool },
    Drive,
    Cancel,
}

impl From<u8> for Choice {
    fn from(value: u8) -> Self {
        match value {
            0 => Choice::ChangeWaker,
            1 => Choice::Poll { spurious: true },
            2 => Choice::Cancel,
            3..=129 => Choice::Poll { spurious: false },
            130..=255 => Choice::Drive,
        }
    }
}

struct ChoiceState {
    value: u8,
    count: u8,
}

impl ChoiceState {
    pub fn next(&mut self) -> arbitrary::Result<Choice> {
        self.count = self.count.wrapping_add(1);
        if self.count == 0 {
            return Err(arbitrary::Error::IncorrectFormat);
        }
        self.value = self.value.wrapping_mul(113).wrapping_add(1);
        Ok(Choice::from(self.value))
    }
}

impl<'a> Arbitrary<'a> for ChoiceState {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let value = u.bytes(1)?[0];
        Ok(ChoiceState { value, count: 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drive_poll_fn, testcase};

    #[test]
    fn exhausted_buffer_returns_error_instead_of_livelock() {
        let mut t = testcase!(|| {
            let driver = drive_poll_fn(|()| Poll::Pending);
            let factory = async move |_: ()| {};
            (driver, factory)
        });

        // Completely empty buffer
        let mut u = Unstructured::new(&[]);
        let result = test(&mut t, &mut u);
        assert!(matches!(result, Err(arbitrary::Error::NotEnoughData)));

        // Buffer that gets exhausted exactly inside the inner loop
        let data = [1, 0];
        let mut u = Unstructured::new(&data);
        let result = test(&mut t, &mut u);
        assert!(matches!(result, Err(arbitrary::Error::NotEnoughData)));
    }

    #[test]
    fn choice_state_next_exhaustion() {
        let mut choice = ChoiceState { value: 0, count: 0 };
        for _ in 0..255 {
            assert!(choice.next().is_ok());
        }
        assert!(matches!(
            choice.next(),
            Err(arbitrary::Error::IncorrectFormat)
        ));
    }

    #[test]
    fn test_runner_completion_count() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        struct MyTestCase {
            counter: Arc<AtomicUsize>,
        }

        impl crate::TestCase for MyTestCase {
            type Args<'a> = ();
            type FactoryItem<'a> = ();

            fn init<'a>(
                &self,
                _args: &mut Self::Args<'a>,
            ) -> (impl crate::Driver<'a>, impl core::ops::AsyncFnMut(Self::FactoryItem<'a>)) {
                let driver = crate::drive_poll_fn(|()| Poll::Pending);
                let counter = self.counter.clone();
                let factory = move |_: ()| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    core::future::ready(())
                };
                (driver, factory)
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let mut t = MyTestCase {
            counter: counter.clone(),
        };

        // 1st byte: 2 -> completions_needed = 3 (since 2 % 8 = 2, 1 + 2 = 3)
        // 2nd byte: 50 -> ChoiceState, next value = 50*113+1 = 19 -> Choice::Poll (spurious: false)
        // 3rd byte: 50 -> ChoiceState
        // 4th byte: 50 -> ChoiceState
        let data = [2, 50, 50, 50];
        let mut u = Unstructured::new(&data);

        test(&mut t, &mut u).unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
