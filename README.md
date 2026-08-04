# futures-testing

Every leaf future in Rust must manage its own waker. Forget to store it, cache a stale one, or
fail to handle a spurious poll, and your future silently deadlocks. These bugs are hard
to write deterministic tests for because they depend on poll ordering.

This crate fuzzes the poll ordering. It randomly interleaves polling, driving, cancellation,
spurious wakeups, and waker swaps, then asserts that the waker contract is upheld after every step.

## What it catches

- **Lost wakers** -- returning `Pending` without retaining or waking the `Waker`.
- **Stale wakers** -- caching the first waker instead of accepting a fresh one each poll.
- **Spurious poll intolerance** -- panicking or producing wrong results when polled without a prior wakeup.
- **Cancel-safety violations** -- state corruption when a future is dropped mid-await and retried.

## How it works

You provide two things:

1. A **driver** -- the other side of the leaf future (e.g. a channel sender for a receiver future).
2. A **factory** -- an async closure that produces the future under test.

The runner calls the factory multiple times per test, randomly choosing when to poll, drive,
swap wakers, inject spurious polls, or cancel. When the driver reports progress, the runner
asserts the future's waker was called. Under the hood it uses
[Hegel](https://hegel.dev) to generate and shrink the interleaving. Failing
examples are persisted locally and can be replayed from a printed failure blob.

```rust
use std::ops::ControlFlow;
use futures_testing::{drive_poll_fn_with, generators as gs, testcase};
use futures::StreamExt;

#[test]
fn mpsc_receiver_handles_wakers_correctly() {
    futures_testing::tests(testcase!(|| {
        let (mut tx, mut rx) = futures::channel::mpsc::channel::<u8>(4);

        let driver = drive_poll_fn_with(gs::integers::<u8>(), move |item| match tx.try_send(item) {
            Ok(()) => std::task::Poll::Ready(ControlFlow::Continue(())),
            Err(_) => std::task::Poll::Pending,
        });

        let factory = async move |_: ()| {
            let _ = rx.next().await;
        };

        (driver, factory)
    }))
    .run();
}
```

## Configuration and replay

`tests(...)` returns Hegel's runner. This crate defaults to 1,000 test cases
and prints a replay blob when it finds a violation. Custom settings replace
those defaults:

```rust,ignore
use futures_testing::Settings;

futures_testing::tests(testcase)
    .settings(Settings::new().test_cases(5_000).print_blob(true))
    .run();
```

Paste a printed blob into `reproduce_failure` to keep a deterministic
regression test:

```rust,ignore
futures_testing::tests(testcase)
    .reproduce_failure("AXic...")
    .run();
```

This replaces arbtest's `.seed(...)` API. Hegel also maintains a local failure
database outside CI so recently discovered failures are replayed first.

## Input generation

The `_with` driver constructors accept native Hegel generators, allowing driver
inputs to shrink semantically:

```rust,ignore
use futures_testing::{drive_poll_fn_with, generators as gs};

let driver = drive_poll_fn_with(gs::integers::<u8>(), |item| {
    // drive the future with item
});
```

The original `drive_poll_fn`, `drive_fn`, and `drive_sink` constructors remain
as compatibility adapters for types that only implement `Arbitrary`. New code
should prefer the `_with` forms.

## Features

- `tracing` -- emit trace-level logs for each action the runner takes, useful for debugging failures.
