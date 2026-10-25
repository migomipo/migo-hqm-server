//! A bounded history of the most recent items.

use std::collections::VecDeque;

/// Keeps the most recent `capacity` items, dropping the oldest when a new one is pushed.
///
/// Items are indexed by age, so the newest item is at age 0.
#[derive(Debug, Clone)]
pub(crate) struct History<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> History<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Changes how many items are kept, dropping the oldest ones that no longer fit.
    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.items.truncate(capacity);
    }

    /// Adds an item as the newest, dropping the oldest if the history is full.
    pub(crate) fn push(&mut self, item: T) {
        if self.capacity == 0 {
            return;
        }
        if self.items.len() == self.capacity {
            self.items.pop_back();
        }
        self.items.push_front(item);
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn newest(&self) -> Option<&T> {
        self.items.front()
    }

    pub(crate) fn newest_mut(&mut self) -> Option<&mut T> {
        self.items.front_mut()
    }

    /// Returns the item pushed `age` pushes before the newest one.
    pub(crate) fn get(&self, age: usize) -> Option<&T> {
        self.items.get(age)
    }

    pub(crate) fn iter_newest_first(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub(crate) fn iter_oldest_first(&self) -> impl Iterator<Item = &T> {
        self.items.iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_of(capacity: usize, items: impl IntoIterator<Item = u32>) -> History<u32> {
        let mut history = History::new(capacity);
        for item in items {
            history.push(item);
        }
        history
    }

    fn newest_first(history: &History<u32>) -> Vec<u32> {
        history.iter_newest_first().copied().collect()
    }

    #[test]
    fn pushing_past_capacity_drops_the_oldest() {
        let history = history_of(3, 1..=5);

        assert_eq!(newest_first(&history), [5, 4, 3]);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn items_are_indexed_by_age() {
        let history = history_of(3, 1..=3);

        assert_eq!(history.newest(), Some(&3));
        assert_eq!(history.get(0), Some(&3));
        assert_eq!(history.get(2), Some(&1));
        assert_eq!(history.get(3), None);
        assert_eq!(
            history.iter_oldest_first().copied().collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn a_history_without_capacity_keeps_nothing() {
        let history = history_of(0, 1..=3);

        assert_eq!(history.len(), 0);
        assert_eq!(history.newest(), None);
    }

    #[test]
    fn shrinking_the_capacity_drops_the_oldest() {
        let mut history = history_of(5, 1..=5);

        history.set_capacity(2);
        assert_eq!(newest_first(&history), [5, 4]);

        history.set_capacity(3);
        history.push(6);
        history.push(7);
        assert_eq!(newest_first(&history), [7, 6, 5]);
    }

    #[test]
    fn the_newest_item_can_be_updated_in_place() {
        let mut history = history_of(3, 1..=2);

        *history.newest_mut().unwrap() = 20;
        assert_eq!(newest_first(&history), [20, 1]);

        history.clear();
        assert_eq!(history.newest_mut(), None);
    }
}
