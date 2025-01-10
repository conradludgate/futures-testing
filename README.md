# Testing framework for leaf futures

All leaf futures in Rust must manage their own wakers, and cannot rely on anything else to schedule their progression.
It's sometimes easy to forget to schedule this waker, which can cause the future to never make progress.

