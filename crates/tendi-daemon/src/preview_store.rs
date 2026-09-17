use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Independent previews have independent lifetimes. Applying one consumes only
/// that ID; creating an unrelated preview never replaces a user's pending plan.
#[derive(Debug)]
pub(crate) struct PreviewStore<T> {
    entries: BTreeMap<String, (Instant, T)>,
    capacity: usize,
    ttl: Duration,
}

impl<T> Default for PreviewStore<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity: 64,
            ttl: Duration::from_secs(30 * 60),
        }
    }
}

impl<T> PreviewStore<T> {
    pub fn insert(&mut self, id: String, value: T) -> Result<(), &'static str> {
        self.entries
            .retain(|_, (created, _)| created.elapsed() < self.ttl);
        if self.entries.len() >= self.capacity {
            return Err("too many pending previews; apply an existing preview or wait for expiry");
        }
        self.entries.insert(id, (Instant::now(), value));
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&T> {
        self.entries
            .get(id)
            .filter(|(created, _)| created.elapsed() < self.ttl)
            .map(|(_, value)| value)
    }

    pub fn remove(&mut self, id: &str) -> Option<T> {
        self.entries
            .remove(id)
            .filter(|(created, _)| created.elapsed() < self.ttl)
            .map(|(_, value)| value)
    }
}

#[cfg(test)]
#[path = "preview_store_tests.rs"]
mod tests;
