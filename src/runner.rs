use alloc::sync::Arc;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::AtomicBool;
use core::task::Context;
use std::{pin::Pin, sync::atomic::Ordering, task::Poll};

use arbitrary::Unstructured;
use futures_util::task::waker_ref;

use crate::{Driver, TestCase};

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
    let completions_needed = u.int_in_range(1..=4)?;
    let (driver, mut factory) = t.init(&mut args);
    let mut driver = pin!(driver);
    let mut waker = Arc::new(TestWaker {
        woken: AtomicBool::new(true),
    });
    let mut completions = 0;
    let mut driver_done = false;

    while completions < completions_needed && !driver_done {
        let mut future = pin!(factory(u.arbitrary()?));
        let mut v: u8 = u.arbitrary()?;

        let mut noop_count: u8 = 0;

        loop {
            noop_count = noop_count.wrapping_add(1);
            if noop_count == 0 {
                return Err(arbitrary::Error::NotEnoughData);
            }
            v = v.wrapping_mul(113).wrapping_add(1);

            match Choice::from(v) {
                Choice::ChangeWaker => {
                    if !waker.woken.swap(false, Ordering::SeqCst) {
                        continue;
                    }
                    waker = Arc::new(TestWaker {
                        woken: AtomicBool::new(true),
                    });
                }
                Choice::SpuriousPoll => {
                    waker.woken.store(false, Ordering::SeqCst);
                    if poll_fut(&mut waker, future.as_mut()).is_ready() {
                        completions += 1;
                        waker.woken.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Choice::Poll => {
                    if !waker.woken.swap(false, Ordering::SeqCst) {
                        continue;
                    }
                    if poll_fut(&mut waker, future.as_mut()).is_ready() {
                        completions += 1;
                        waker.woken.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Choice::Drive => {
                    if let Poll::Ready(cf) = driver.as_mut().poll(u)? {
                        let woken = waker.woken.load(Ordering::SeqCst);
                        assert!(woken, "future was not woken when driver made progress");
                        if cf.is_break() {
                            driver_done = true;
                        }
                    }
                }
                Choice::Cancel => break,
            }

            v = u.arbitrary()?;
            noop_count = 0;
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

enum Choice {
    ChangeWaker,
    SpuriousPoll,
    Poll,
    Drive,
    Cancel,
}

impl From<u8> for Choice {
    fn from(value: u8) -> Self {
        match value {
            0 => Choice::ChangeWaker,
            1 => Choice::SpuriousPoll,
            2 => Choice::Cancel,
            3..=129 => Choice::Poll,
            130..=255 => Choice::Drive,
        }
    }
}
