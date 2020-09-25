//! A semaphore that owns a value.

use std::ops::Deref;

/// A semaphore guarding access to a read-only type.
///
/// Typical semaphores (like `tokio::sync::Semaphore`) are independant of a contained type. This
/// implementation owns the value similar to a `std::sync::Mutex`, thus only allowing access through
/// this semaphore.
#[derive(Debug)]
pub(crate) struct Semaphore<T> {
    /// Semaphore used to actually restrict access.
    permits: tokio::sync::Semaphore,
    /// Item that access is restricted to.
    item: T,
}

impl<T> Semaphore<T> {
    /// Creates a new semaphore.
    pub(crate) fn new(permits: usize, item: T) -> Self {
        Semaphore {
            permits: tokio::sync::Semaphore::new(permits),
            item,
        }
    }

    /// Acquires a permit from the semaphore.
    pub(crate) async fn acquire(&self) -> SemaphoreGuard<'_, T> {
        let permit = self.permits.acquire().await;
        SemaphoreGuard {
            _permit: permit,
            item: &self.item,
        }
    }

    /// Deconstructs the semaphore, returning the item.
    pub(crate) fn into_inner(self) -> T {
        self.item
    }
}

/// Semaphore permit with item reference.
pub(crate) struct SemaphoreGuard<'a, T> {
    /// Keep a semaphore permit, but only to have it released on drop.
    _permit: tokio::sync::SemaphorePermit<'a>,
    /// Actual item reference.
    item: &'a T,
}

impl<'a, T> Deref for SemaphoreGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

#[cfg(test)]
mod tests {
    use super::Semaphore;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test(threaded_scheduler)]
    async fn test_access_does_not_exceed_limit() {
        const PERMITS: usize = 2;
        const TOTAL_TASKS: usize = 10_000;

        /// Execution statistics.
        #[derive(Debug, Default)]
        struct Stats {
            /// Number of tasks running in parallel.
            parallel_tasks: AtomicUsize,
            /// Maximum number of parallel tasks ever seen.
            max_parallel: AtomicUsize,
        }

        impl Stats {
            /// Increments stats due to entering a critical section.
            fn inc(&self) {
                // Store the number of previous parallel tasks.
                let other_tasks = self.parallel_tasks.fetch_add(1, Ordering::SeqCst);

                // Record the new max.
                self.max_parallel
                    .fetch_max(other_tasks + 1, Ordering::SeqCst);
            }

            /// Decrement status due to exiting a critical section.
            fn dec(&self) {
                self.parallel_tasks.fetch_sub(1, Ordering::SeqCst);
            }
        }

        // Diagnostics.
        let stats = Arc::new(Stats::default());

        // Actual "value" we are protecting with the semaphore.
        let counter = Arc::new(Semaphore::new(PERMITS, AtomicUsize::new(0)));

        let mut handles = Vec::new();
        for _ in 0..TOTAL_TASKS {
            let counter = counter.clone();
            let stats = stats.clone();
            handles.push(tokio::spawn(async move {
                // Aquire the the counter semaphore.
                let ctr = counter.acquire().await;

                // We're in the critical section now.
                stats.inc();
                ctr.fetch_add(1, Ordering::SeqCst);
                stats.dec();
            }));
        }

        // Wait for all tasks to finish.
        for handle in handles {
            handle.await.expect("await failed");
        }

        let final_stats = Arc::try_unwrap(stats).expect("stats have more than one reference");
        let final_count = Arc::try_unwrap(counter)
            .expect("counter has more than one references")
            .into_inner()
            .into_inner();

        assert_eq!(final_stats.max_parallel.into_inner(), PERMITS);
        assert_eq!(final_count, TOTAL_TASKS);
    }
}
