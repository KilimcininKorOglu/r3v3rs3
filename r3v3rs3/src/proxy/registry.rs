use std::{
    collections::HashMap,
    fmt,
    hash::Hash,
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// Values by key that stay registered while another owner holds them.
pub struct WeakRegistry<K, V> {
    entries: Mutex<HashMap<K, Weak<V>>>,
}

impl<K, V> Default for WeakRegistry<K, V> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: fmt::Debug, V> fmt::Debug for WeakRegistry<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_list().entries(entries.keys()).finish()
    }
}

impl<K: Eq + Hash + Clone, V> WeakRegistry<K, V> {
    /// Returns the value of `key` when `reuse` accepts it. Otherwise registers and returns the
    /// value of `create`.
    pub fn get_or_create(
        &self,
        key: K,
        reuse: impl FnOnce(&V) -> bool,
        create: impl FnOnce() -> Arc<V>,
    ) -> Arc<V> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, value| value.strong_count() > 0);
        if let Some(existing) = entries.get(&key).and_then(Weak::upgrade) {
            if reuse(&existing) {
                return existing;
            }
        }
        let value = create();
        entries.insert(key, Arc::downgrade(&value));
        value
    }

    pub fn get(&self, key: &K) -> Option<Arc<V>> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .and_then(Weak::upgrade)
    }

    /// The registered values that an owner still holds.
    pub fn live(&self) -> Vec<(K, Arc<V>)> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.upgrade()?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_value_is_not_reused() {
        let registry = WeakRegistry::<u8, u32>::default();
        let first = registry.get_or_create(1, |_| true, || Arc::new(10));
        let same = registry.get_or_create(1, |_| true, || Arc::new(11));
        assert!(Arc::ptr_eq(&first, &same));

        let rejected = registry.get_or_create(1, |value| *value == 11, || Arc::new(12));
        assert_eq!(*rejected, 12);
        assert_eq!(registry.get(&1).as_deref(), Some(&12));

        drop((first, same, rejected));
        assert!(registry.get(&1).is_none());
        assert!(registry.live().is_empty());
        assert_eq!(*registry.get_or_create(1, |_| true, || Arc::new(13)), 13);
    }
}
