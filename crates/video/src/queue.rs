//! Bounded frame queue.
//!
//! When the encoder is behind, a new frame is dropped. The queue does not
//! grow and it does not discard a frame the encoder has already accepted.

use std::collections::VecDeque;

/// How many frames may wait for the encoder.
pub const FRAME_QUEUE_CAPACITY: usize = 2;

/// Result of offering one frame to a [`DropQueue`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushResult {
    Queued,
    Dropped,
}

/// Fixed-capacity queue that drops the incoming frame when it is full.
#[derive(Debug)]
pub struct DropQueue<T> {
    capacity: usize,
    items: VecDeque<T>,
    dropped: u64,
}

impl<T> DropQueue<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            items: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn push(&mut self, item: T) -> PushResult {
        if self.items.len() >= self.capacity {
            self.dropped = self.dropped.saturating_add(1);
            PushResult::Dropped
        } else {
            self.items.push_back(item);
            PushResult::Queued
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{DropQueue, PushResult, FRAME_QUEUE_CAPACITY};

    #[test]
    fn a_full_queue_drops_the_new_frame() {
        let mut queue = DropQueue::new(FRAME_QUEUE_CAPACITY);
        assert_eq!(queue.push(1), PushResult::Queued);
        assert_eq!(queue.push(2), PushResult::Queued);
        assert_eq!(queue.push(3), PushResult::Dropped);
        assert_eq!(queue.dropped(), 1);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);
    }
}
