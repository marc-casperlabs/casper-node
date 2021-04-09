//! A counting single queue.
//!
//! Counting track their item count in a non-locking manner to allow for rough diagnostics without
//! having to lock them in their entirety.

#[derive(Debug)]
struct CountingQueue<I> {}

/// State that wraps queue and its event count.
///
/// This is essentially a single queue for internal use. Note that it does not enforce correct
/// locking or consistency to support different access patterns.
///
/// In general, `count` should only be modified when holding a lock.
#[derive(Debug)]
struct CountingQueue<I> {
    /// A queue's event counter.
    ///
    /// Do not modify this unless you are holding the `queue` lock.
    count: AtomicUsize,

    /// Individual queues.
    items: VecDeque<I>,
}
