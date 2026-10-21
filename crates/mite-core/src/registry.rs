//! A process-wide list of a plugin's live instances, so editors can see each other (collision
//! detection between tracks). Hosts usually load every instance of a plugin into one process,
//! where the plugin's statics are shared. Entries are weak, so an instance that goes away just
//! drops out.
//!
//! The list is locked, so only UI threads and instance construction may touch it, never the
//! audio thread.

use std::sync::{Arc, Mutex, Weak};

pub struct Registry<T> {
    entries: Mutex<Vec<(u32, Weak<T>)>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Registry<T> {
    pub const fn new() -> Self {
        Self { entries: Mutex::new(Vec::new()) }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Vec<(u32, Weak<T>)>> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|(_, w)| w.strong_count() > 0);
        entries
    }

    /// Adds `item` and returns its number: the lowest one no live instance has, from 1, so the
    /// names stay short as instances come and go.
    pub fn register(&self, item: &Arc<T>) -> u32 {
        let mut entries = self.live();
        if let Some(&(n, _)) = entries.iter().find(|(_, w)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(item))) {
            return n;
        }
        let number = (1..).find(|n| entries.iter().all(|(m, _)| m != n)).expect("a free number");
        entries.push((number, Arc::downgrade(item)));
        number
    }

    /// `item`'s number, if it is registered.
    pub fn number(&self, item: &Arc<T>) -> Option<u32> {
        self.live().iter().find(|(_, w)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(item))).map(|&(n, _)| n)
    }

    /// Every live instance but `me`, by number.
    pub fn others(&self, me: &Arc<T>) -> Vec<(u32, Arc<T>)> {
        let mut out: Vec<(u32, Arc<T>)> = self
            .live()
            .iter()
            .filter(|(_, w)| !std::ptr::eq(w.as_ptr(), Arc::as_ptr(me)))
            .filter_map(|(n, w)| w.upgrade().map(|a| (*n, a)))
            .collect();
        out.sort_by_key(|(n, _)| *n);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_see_each_other_and_drop_out_when_gone() {
        let registry = Registry::new();
        let (a, b) = (Arc::new("a"), Arc::new("b"));
        assert_eq!(registry.register(&a), 1);
        assert_eq!(registry.register(&b), 2);
        assert_eq!(registry.register(&a), 1, "registering twice keeps the number");
        assert_eq!(registry.others(&a).iter().map(|(n, s)| (*n, **s)).collect::<Vec<_>>(), vec![(2, "b")]);
        drop(b);
        assert!(registry.others(&a).is_empty());
        // The freed number is reused.
        let c = Arc::new("c");
        assert_eq!(registry.register(&c), 2);
        assert_eq!(registry.number(&c), Some(2));
        assert_eq!(registry.number(&Arc::new("d")), None);
    }
}
