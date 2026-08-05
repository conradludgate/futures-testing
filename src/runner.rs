use alloc::sync::Arc;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::AtomicBool;
use core::task::Context;
use std::{pin::Pin, sync::atomic::Ordering, task::Poll};

use futures_util::task::waker_ref;
use hegel::TestCase as HegelTestCase;
use hegel::generators::{self as gs, Generator};

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

pub(crate) fn test<T: TestCase>(t: &T, tc: &HegelTestCase) {
    let completions_needed = tc.draw_silent(gs::integers::<u8>().max_value(7));
    // Like `TestCase::repeat`, leave collection sizing to Hegel. Generating
    // commands directly also lets Hegel shrink the schedule structure and
    // each command together.
    let actions = tc.draw_silent(gs::vecs(gs::integers::<u8>().map(Choice::from)));
    let schedule_length = u32::try_from(actions.len()).unwrap_or(u32::MAX);
    tc.target_labelled(f64::from(schedule_length), "schedule length");
    tc.note(&format!("completions needed = {completions_needed}"));
    let mut actions = actions.into_iter();

    run(t, tc, completions_needed, || actions.next());
}

fn run<T, F>(t: &T, tc: &HegelTestCase, completions_needed: u8, mut next_choice: F)
where
    T: TestCase,
    F: FnMut() -> Option<Choice>,
{
    let mut args = tc.draw_silent(t.args());
    let factory_items = t.factory_items();
    let (driver, mut factory) = t.init(&mut args);
    let mut driver = pin!(driver);
    let mut waker = Arc::new(TestWaker {
        woken: AtomicBool::new(true),
    });
    let mut completions = 0;
    let mut driver_done = false;
    let mut step = 0_u32;
    let mut skipped = 0_u32;

    'schedule: while completions <= completions_needed && !driver_done {
        #[cfg(feature = "tracing")]
        let _span =
            tracing::trace_span!("iteration", iteration = completions + 1, completions_needed)
                .entered();

        let mut future = pin!(factory(tc.draw_silent(&factory_items)));
        let mut waker_registered = false;
        loop {
            let Some(choice) = next_choice() else {
                break 'schedule;
            };
            step += 1;
            // `repeat` emits a note before running each iteration. Do the same
            // for schedule commands so a failure is anchored to its cause.
            tc.note(&format!("// Schedule step #{step}: {choice:?}"));
            match choice {
                Choice::ChangeWaker => {
                    if !waker.woken.swap(false, Ordering::SeqCst) {
                        skipped += 1;
                        tc.note("  skipped (current waker has not been woken)");
                        continue;
                    }
                    tc.note("  applied");
                    trace!("change_waker");
                    waker = Arc::new(TestWaker {
                        woken: AtomicBool::new(true),
                    });
                    waker_registered = false;
                }
                Choice::Poll { spurious } => {
                    let was_woken = waker.woken.swap(false, Ordering::SeqCst);
                    if !(was_woken || spurious) {
                        skipped += 1;
                        tc.note("  skipped (future was not woken)");
                        continue;
                    }
                    let poll = poll_fut(&mut waker, future.as_mut());
                    tc.note(if poll.is_ready() {
                        "  returned Ready"
                    } else {
                        "  returned Pending"
                    });
                    trace!(
                        spurious = spurious && !was_woken,
                        ready = poll.is_ready(),
                        "poll"
                    );
                    if poll.is_ready() {
                        completions += 1;
                        waker.woken.store(true, Ordering::SeqCst);
                        break;
                    }
                    waker_registered = true;
                }
                Choice::Drive => {
                    let action = tc.draw_silent(driver.as_ref().get_ref().actions());
                    let poll = driver.as_mut().poll(action);
                    tc.note(match poll {
                        Poll::Pending => "  returned Pending",
                        Poll::Ready(cf) if cf.is_break() => "  returned Ready(Break)",
                        Poll::Ready(_) => "  returned Ready(Continue)",
                    });
                    trace!(
                        ready = poll.is_ready(),
                        done = matches!(poll, Poll::Ready(cf) if cf.is_break()),
                        "drive"
                    );
                    if let Poll::Ready(cf) = poll {
                        if waker_registered {
                            let woken = waker.woken.load(Ordering::SeqCst);
                            assert!(woken, "future was not woken when driver made progress");
                        }
                        if cf.is_break() {
                            driver_done = true;
                        }
                    }
                }
                Choice::Cancel => {
                    tc.note("  applied");
                    trace!("cancel");
                    break;
                }
            }
        }
    }

    let efficiency = f64::from(step - skipped) / f64::from(step.max(1));
    tc.target_labelled(efficiency, "schedule efficiency");
}

fn poll_fut(waker: &mut Arc<TestWaker>, f: Pin<&mut impl Future>) -> Poll<()> {
    let waker_ref = waker_ref(waker);
    let mut cx = Context::from_waker(&waker_ref);
    if f.poll(&mut cx).is_ready() {
        return Poll::Ready(());
    }

    if let Some(waker) = Arc::get_mut(waker) {
        let woken = *waker.woken.get_mut();
        assert!(woken, "Waker passed to future was lost without being woken");
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
            // Put productive actions first because Hegel shrinks integers
            // toward zero. The bucket sizes match the old byte encoding.
            0..=126 => Choice::Poll { spurious: false },
            127..=252 => Choice::Drive,
            253 => Choice::ChangeWaker,
            254 => Choice::Poll { spurious: true },
            255 => Choice::Cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drive_poll_fn_with, testcase};

    #[test]
    fn exhausted_schedule_terminates_instead_of_livelock() {
        let t = testcase!(|| {
            let driver = drive_poll_fn_with(gs::unit(), |()| Poll::Pending);
            let factory = async move |()| {};
            (driver, factory)
        });

        // A finite Hegel-generated schedule may end while a future is pending.
        hegel::Hegel::new(move |tc| {
            run(&t, &tc, 0, || None);
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();
    }

    #[test]
    fn choice_mapping_preserves_action_weighting() {
        assert_eq!(Choice::from(0), Choice::Poll { spurious: false });
        assert_eq!(Choice::from(126), Choice::Poll { spurious: false });
        assert_eq!(Choice::from(127), Choice::Drive);
        assert_eq!(Choice::from(252), Choice::Drive);
        assert_eq!(Choice::from(253), Choice::ChangeWaker);
        assert_eq!(Choice::from(254), Choice::Poll { spurious: true });
        assert_eq!(Choice::from(255), Choice::Cancel);
    }

    #[test]
    fn test_runner_completion_count() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        struct MyTestCase {
            counter: Arc<AtomicUsize>,
        }

        impl crate::TestCase for MyTestCase {
            type Args = ();
            type FactoryItem = ();

            fn args(&self) -> impl Generator<Self::Args> {
                gs::unit()
            }

            fn factory_items(&self) -> impl Generator<Self::FactoryItem> {
                gs::unit()
            }

            fn init<'a>(
                &self,
                _args: &'a mut Self::Args,
            ) -> (
                impl crate::Driver<'a>,
                impl core::ops::AsyncFnMut(Self::FactoryItem),
            ) {
                let driver = crate::drive_poll_fn_with(gs::unit(), |()| Poll::Pending);
                let counter = self.counter.clone();
                let factory = move |()| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    core::future::ready(())
                };
                (driver, factory)
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let t = MyTestCase {
            counter: counter.clone(),
        };

        hegel::Hegel::new(move |tc| {
            run(&t, &tc, 2, || Some(Choice::Poll { spurious: false }));
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
