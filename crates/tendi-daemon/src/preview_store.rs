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
mod tests {
    use super::*;

    #[test]
    fn consuming_one_preview_preserves_other_plans() {
        let mut previews = PreviewStore::default();
        previews.insert("a".into(), 10).unwrap();
        previews.insert("b".into(), 20).unwrap();
        assert_eq!(previews.remove("a"), Some(10));
        assert_eq!(previews.remove("a"), None);
        assert_eq!(previews.get("b"), Some(&20));
    }

    #[test]
    fn full_store_rejects_new_previews_without_evicting_existing_plans() {
        let mut previews = PreviewStore {
            capacity: 1,
            ..Default::default()
        };
        previews.insert("a".into(), 10).unwrap();
        assert!(previews.insert("b".into(), 20).is_err());
        assert_eq!(previews.get("a"), Some(&10));
    }

    #[test]
    fn expired_preview_cannot_be_read_or_applied() {
        let mut previews = PreviewStore::<()>::default();
        previews
            .entries
            .insert("expired".into(), (Instant::now() - previews.ttl, ()));
        assert_eq!(previews.get("expired"), None);
        assert_eq!(previews.remove("expired"), None);
    }
}
