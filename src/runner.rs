use alloc::sync::Arc;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::AtomicBool;
use core::task::Context;
use std::{pin::Pin, sync::atomic::Ordering, task::Poll};

use arbitrary::Unstructured;
use futures_util::task::waker_ref;
use hegel::TestCase as HegelTestCase;
use hegel::generators as gs;

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

pub(crate) fn test<T: TestCase>(t: &mut T, tc: &HegelTestCase) -> arbitrary::Result<()> {
    // Keep `arbitrary` as the data-generation compatibility layer for user
    // supplied Args and FactoryItem values. Scheduling and built-in driver
    // inputs are separate native Hegel draws, so Hegel can shrink them itself.
    let data = tc.draw_silent(gs::binary().min_size(1).max_size(65_536));
    tc.target_labelled(data.len() as f64, "arbitrary data size");
    let mut u = Unstructured::new(&data);
    let completions_needed = tc.draw_silent(gs::integers::<u8>().max_value(7));
    let actions: Vec<Choice> = tc
        .draw_silent(gs::vecs(gs::integers::<u8>()).max_size(4_096))
        .into_iter()
        .map(Choice::from)
        .collect();
    tc.target_labelled(actions.len() as f64, "schedule length");
    tc.note(&format!("arbitrary data bytes = {}", data.len()));
    tc.note(&format!("completions needed = {completions_needed}"));
    tc.note(&format!("schedule = {actions:?}"));
    let mut actions = actions.into_iter();

    run(t, &mut u, tc, completions_needed, || actions.next())
}

fn run<T, F>(
    t: &mut T,
    u: &mut Unstructured<'_>,
    tc: &HegelTestCase,
    completions_needed: u8,
    mut next_choice: F,
) -> arbitrary::Result<()>
where
    T: TestCase,
    F: FnMut() -> Option<Choice>,
{
    let mut args = u.arbitrary()?;
    let (driver, mut factory) = t.init(&mut args);
    let mut driver = pin!(driver);
    let mut waker = Arc::new(TestWaker {
        woken: AtomicBool::new(true),
    });
    let mut completions = 0;
    let mut driver_done = false;

    while completions <= completions_needed && !driver_done {
        #[cfg(feature = "tracing")]
        let _span =
            tracing::trace_span!("iteration", iteration = completions + 1, completions_needed)
                .entered();

        let mut future = pin!(factory(u.arbitrary()?));
        let mut waker_registered = false;
        loop {
            let Some(choice) = next_choice() else {
                return Ok(());
            };
            match choice {
                Choice::ChangeWaker => {
                    if !waker.woken.swap(false, Ordering::SeqCst) {
                        continue;
                    }
                    trace!("change_waker");
                    waker = Arc::new(TestWaker {
                        woken: AtomicBool::new(true),
                    });
                    waker_registered = false;
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
                    waker_registered = true;
                }
                Choice::Drive => {
                    let poll = driver.as_mut().poll(tc);
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
                    trace!("cancel");
                    break;
                }
            }
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
        let mut t = testcase!(|| {
            let driver = drive_poll_fn_with(gs::unit(), |()| Poll::Pending);
            let factory = async move |_: ()| {};
            (driver, factory)
        });

        // A finite Hegel-generated schedule may end while a future is pending.
        hegel::Hegel::new(move |tc| {
            let mut u = Unstructured::new(&[]);
            let result = run(&mut t, &mut u, &tc, 0, || None);
            assert!(result.is_ok());
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
            type Args<'a> = ();
            type FactoryItem<'a> = ();

            fn init<'a>(
                &self,
                _args: &mut Self::Args<'a>,
            ) -> (
                impl crate::Driver<'a>,
                impl core::ops::AsyncFnMut(Self::FactoryItem<'a>),
            ) {
                let driver = crate::drive_poll_fn_with(gs::unit(), |()| Poll::Pending);
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

        hegel::Hegel::new(move |tc| {
            let mut u = Unstructured::new(&[]);
            run(&mut t, &mut u, &tc, 2, || {
                Some(Choice::Poll { spurious: false })
            })
            .unwrap();
        })
        .settings(hegel::Settings::new().test_cases(1).database(None))
        .run();

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
