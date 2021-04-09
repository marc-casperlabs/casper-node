//! Weighted round-robin scheduling.
//!
//! This module implements a weighted round-robin scheduler that ensures no starvation occurs, but
//! still allows prioritizing events from one source over another. The module uses `tokio`'s
//! synchronization primitives under the hood.

use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    fs::File,
    hash::Hash,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    sync::Mutex,
};

use enum_iterator::IntoEnumIterator;
use serde::{ser::SerializeMap, Serialize, Serializer};
use tokio::sync::Notify;

/// Weighted round-robin scheduler.
///
/// The weighted round-robin scheduler keeps queues internally and returns an item from a queue when
/// asked. Each queue is assigned a weight, which is simply the maximum amount of items returned
/// from it before moving on to the next queue.
///
/// If a queue is empty, it is skipped until the next round. Queues are processed in the order they
/// are passed to the constructor function.
///
/// The scheduler keeps track internally which queue needs to be popped next.
#[derive(Debug)]
pub struct WeightedRoundRobin<I, K> {
    /// Lock-protected internal state.
    state: Mutex<InternalState<I, K>>,

    /// A list of slots that are round-robin'd.
    ///
    /// These function as a blueprint for instances of `state.active_slow`. Using a vec of K's
    /// variants ensures each slot has an identifying index 0..n (with `n` being the number of
    /// variants), which otherwise might not hold true,
    slots: Vec<Slot<K>>,

    /// A notification for clients waiting to pop a value from the queue.
    notify: Notify,
}

#[derive(Debug)]
struct InternalState<I, K> {
    /// The currently active slot.
    ///
    /// Once it has no tickets left, the next slot is loaded.
    active_slot: Slot<K>,

    /// The position in `slots` the `active_slot` was cloned from. Used to calculate the next slot.
    active_slot_idx: usize,

    /// Actual queues.
    queues: HashMap<K, VecDeque<I>>,

    count: usize,
}

/// An internal slot in the round-robin scheduler.
///
/// A slot marks the scheduling position, i.e. which queue we are currently polling and how many
/// tickets it has left before the next one is due.
#[derive(Copy, Clone, Debug)]
struct Slot<K> {
    /// The key, identifying a queue.
    key: K,

    /// Number of items to return before moving on to the next queue.
    tickets: usize,
}

impl<I, K> WeightedRoundRobin<I, K>
where
    K: Copy + Clone + Eq + Hash,
{
    /// Creates a new weighted round-robin scheduler.
    ///
    /// Creates a queue for each pair given in `weights`. The second component of each `weight` is
    /// the number of times to return items from one queue before moving on to the next one.
    pub(crate) fn new(weights: Vec<(K, NonZeroUsize)>) -> Self {
        assert!(!weights.is_empty(), "must provide at least one slot");

        let queues = weights
            .iter()
            .map(|(idx, _)| (*idx, Default::default()))
            .collect();
        let slots: Vec<Slot<K>> = weights
            .into_iter()
            .map(|(key, tickets)| Slot {
                key,
                tickets: tickets.get(),
            })
            .collect();
        let active_slot = slots[0];

        WeightedRoundRobin {
            state: Mutex::new(InternalState {
                active_slot,
                active_slot_idx: 0,
                queues,
                count: 0,
            }),
            slots,
            notify: Notify::new(),
        }
    }

    /// Pushes an item to a queue identified by key.
    ///
    /// ## Panics
    ///
    /// Panics if the state lock has been poisoned.
    #[inline]
    pub(crate) async fn push(&self, item: I, queue: K) {
        // Add the item, then release the lock. It's fine to do this, as the number of permits is
        // supposed to be less or equal than the number of items, not exact.
        {
            let mut guard = self.state.lock().expect("state lock poisoned");
            guard
                .queues
                .get_mut(&queue)
                .expect("the queue disappeared. this should not happen")
                .push_back(item);
            guard.count += 1;
        }

        // If there's a client waiting, notify it.
        self.notify.notify_one();
    }

    /// Returns the next item from queue.
    ///
    /// Asynchronously waits until a queue is non-empty.
    ///
    /// # Panics
    ///
    /// Panics if the internal state lock has been poisoned.
    pub(crate) async fn pop(&self) -> (I, K) {
        'wait: loop {
            let mut state = self.state.lock().expect("lock poisoned");

            if state.count == 0 {
                drop(state);
                self.notify.notified().await;
                // Currently spinlocks.
                continue 'wait;
            }

            // At this point, we know we have at least one item in a queue.
            'pop: loop {
                // let current_queue = state
                //     .queues
                //     .get(&state.active_slot.key)
                //     .expect("the queue disappeared. this should not happen");

                // if state.active_slot.tickets == 0 || current_queue.is_empty() {
                //     // Go to next queue slot if we've exhausted the current queue.
                //     state.active_slot_idx = (state.active_slot_idx + 1) % self.slots.len();
                //     state.active_slot = self.slots[state.active_slot_idx];
                //     continue 'pop;
                // }

                // // We have hit a queue that is not empty. Decrease tickets and pop.
                // state.active_slot.tickets -= 1;

                // let item = current_queue
                //     .pop_front()
                //     // We hold the lock and checked `is_empty` earlier.
                //     .expect("item disappeared. this should not happen");
                // return (item, inner.active_slot.key);
            }
        }
    }

    /// Drains all events from a specific queue.
    pub(crate) async fn drain_queue(&self, queue: K) -> Vec<I> {
        todo!()
        // let mut state = self.state.lock().expect("lock poisoned");

        // let events = self
        //     .queues
        //     .get(&queue)
        //     .expect("queue to be drained disappeared")
        //     .drain()
        //     .await;

        // // TODO: This is racy if someone is calling `pop` at the same time.
        // self.total
        //     .acquire_many(events.len() as u32)
        //     .await
        //     .expect("could not acquire tickets during drain")
        //     .forget();

        // events
    }

    /// Returns the number of events currently in the queue.
    #[cfg(test)]
    pub(crate) fn item_count(&self) -> usize {
        todo!()
        // self.total.available_permits()
    }

    /// Returns the number of events in each of the queues.
    ///
    /// This function may be slightly inaccurate, as it does not lock the queues to get a snapshot
    /// across all queues.
    pub(crate) fn event_queues_counts(&self) -> HashMap<K, usize> {
        todo!()
        // self.queues
        //     .iter()
        //     .map(|(key, queue)| (*key, queue.count()))
        //     .collect()
    }
}

impl<I, K> WeightedRoundRobin<I, K>
where
    I: Serialize,
    K: Copy + Clone + Eq + Hash + IntoEnumIterator + Serialize,
{
    /// Create a snapshot of the queue by first locking every queue, then serializing them.
    ///
    /// The serialized events are streamed directly into `serializer`.
    pub async fn snapshot<S: Serializer>(&self, serializer: S) -> Result<(), S::Error> {
        todo!()
        // let locks = self.lock_queues().await;

        // let mut map = serializer.serialize_map(Some(locks.len()))?;

        // // By iterating over the guards, they are dropped in order while we are still
        // serializing. for (kind, guard) in locks {
        //     let vd = &*guard;
        //     map.serialize_key(&kind)?;
        //     map.serialize_value(vd)?;
        // }
        // map.end()?;

        // Ok(())
    }
}

impl<I, K> WeightedRoundRobin<I, K>
where
    I: Debug,
    K: Copy + Clone + Eq + Hash + IntoEnumIterator + Debug,
{
    /// Dump the contents of the queues (`Debug` representation) to a given file.
    pub async fn debug_dump(&self, file: &mut File) -> Result<(), io::Error> {
        todo!()
        // let locks = self.lock_queues().await;

        // let mut writer = BufWriter::new(file);
        // for (kind, guard) in locks {
        //     let queue = &*guard;
        //     writer.write_all(format!("Queue: {:?} ({}) [\n", kind, queue.len()).as_bytes())?;
        //     for event in queue.iter() {
        //         writer.write_all(format!("\t{:?}\n", event).as_bytes())?;
        //     }
        //     writer.write_all(b"]\n")?;
        // }
        // writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::{future::FutureExt, join};

    use super::*;

    #[repr(usize)]
    #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
    enum QueueKind {
        One = 1,
        Two,
    }

    fn weights() -> Vec<(QueueKind, NonZeroUsize)> {
        unsafe {
            vec![
                (QueueKind::One, NonZeroUsize::new_unchecked(1)),
                (QueueKind::Two, NonZeroUsize::new_unchecked(2)),
            ]
        }
    }

    #[tokio::test]
    async fn should_respect_weighting() {
        let scheduler = WeightedRoundRobin::<char, QueueKind>::new(weights());
        // Push three items on to each queue
        let future1 = scheduler
            .push('a', QueueKind::One)
            .then(|_| scheduler.push('b', QueueKind::One))
            .then(|_| scheduler.push('c', QueueKind::One));
        let future2 = scheduler
            .push('d', QueueKind::Two)
            .then(|_| scheduler.push('e', QueueKind::Two))
            .then(|_| scheduler.push('f', QueueKind::Two));
        join!(future2, future1);

        // We should receive the popped values in the order a, d, e, b, f, c
        assert_eq!(('a', QueueKind::One), scheduler.pop().await);
        assert_eq!(('d', QueueKind::Two), scheduler.pop().await);
        assert_eq!(('e', QueueKind::Two), scheduler.pop().await);
        assert_eq!(('b', QueueKind::One), scheduler.pop().await);
        assert_eq!(('f', QueueKind::Two), scheduler.pop().await);
        assert_eq!(('c', QueueKind::One), scheduler.pop().await);
    }
}
