//! A one-place mailbox between threads: a newer item replaces an unread one.

use std::sync::{Condvar, Mutex};

/// Holds at most one item. `put` never waits, and a waiting `take` wakes.
///
/// This is the "latest wins" hand-off: when the worker is slower than the
/// frames arriving, it always gets the newest one and the others are dropped.
pub struct Slot<T> {
    state: Mutex<State<T>>,
    signal: Condvar,
}

struct State<T> {
    item: Option<T>,
    closed: bool,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self { state: Mutex::new(State { item: None, closed: false }), signal: Condvar::new() }
    }
}

impl<T> Slot<T> {
    /// Leave an item, returning the unread one it replaced.
    pub fn put(&self, item: T) -> Option<T> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let replaced = state.item.replace(item);
        self.signal.notify_one();
        replaced
    }

    /// Wait for an item. Returns `None` once the slot is closed.
    pub fn take(&self) -> Option<T> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.closed {
                return None;
            }
            if let Some(item) = state.item.take() {
                return Some(item);
            }
            state = self.signal.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Take the item if there is one, without waiting.
    pub fn try_take(&self) -> Option<T> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).item.take()
    }

    pub fn is_empty(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).item.is_none()
    }

    /// Wake every waiting `take` and make it return `None` from now on.
    pub fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.signal.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn newest_item_wins() {
        let slot = Slot::default();
        assert_eq!(slot.put(1), None);
        assert_eq!(slot.put(2), Some(1));
        assert_eq!(slot.try_take(), Some(2));
        assert!(slot.is_empty());
    }

    #[test]
    fn close_wakes_a_waiting_take() {
        let slot: Arc<Slot<u32>> = Arc::new(Slot::default());
        let waiter = {
            let slot = slot.clone();
            std::thread::spawn(move || slot.take())
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        slot.close();
        assert_eq!(waiter.join().unwrap(), None);
    }

    #[test]
    fn take_receives_what_was_put() {
        let slot: Arc<Slot<u32>> = Arc::new(Slot::default());
        let waiter = {
            let slot = slot.clone();
            std::thread::spawn(move || slot.take())
        };
        slot.put(7);
        assert_eq!(waiter.join().unwrap(), Some(7));
    }
}
