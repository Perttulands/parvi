//! A fixed number of the latest items, oldest dropped first.

use std::collections::VecDeque;

pub struct Ring<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> Ring<T> {
    /// Allocates room for `capacity` items up front, so a full ring costs
    /// what an empty one does.
    pub fn new(capacity: usize) -> Ring<T> {
        let capacity = capacity.max(1);
        Ring {
            items: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Adds `item`, giving back the oldest if the ring was full.
    pub fn push(&mut self, item: T) -> Option<T> {
        let dropped = if self.items.len() == self.capacity {
            self.items.pop_front()
        } else {
            None
        };
        self.items.push_back(item);
        dropped
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_ring_drops_its_oldest() {
        let mut ring = Ring::new(3);
        for i in 0..5 {
            ring.push(i);
        }
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), [2, 3, 4]);
    }
}
