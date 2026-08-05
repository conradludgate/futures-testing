use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

use arbitrary::{Arbitrary, Unstructured};
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

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

#[cfg(test)]
mod tests {
    use super::*;
    use core::ops::ControlFlow;
    use core::pin::pin;
    use std::task::Poll;

    use crate::Driver;

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
        let driver = crate::drive_poll_fn(|_: u8| Poll::Ready(ControlFlow::Continue(())));
        let mut driver = pin!(driver);
        hegel::Hegel::new(|tc| {
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
