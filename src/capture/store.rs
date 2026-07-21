use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{CaptureRecord, CaptureSequence};

#[derive(Clone, Copy, Debug)]
pub(crate) struct CaptureRetentionPolicy {
    pub max_records: usize,
    pub max_bytes: usize,
}

impl Default for CaptureRetentionPolicy {
    fn default() -> Self {
        Self {
            max_records: 10_000,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

pub(crate) struct CaptureStore {
    captures: BTreeMap<CaptureSequence, Arc<CaptureRecord>>,
    policy: CaptureRetentionPolicy,
    retained_bytes: usize,
    revision: u64,
}

impl CaptureStore {
    pub fn new(policy: CaptureRetentionPolicy) -> Self {
        Self {
            captures: BTreeMap::new(),
            policy,
            retained_bytes: 0,
            revision: 0,
        }
    }

    pub fn insert(&mut self, capture: Arc<CaptureRecord>) -> Vec<CaptureSequence> {
        let sequence = capture.sequence();
        let capture_bytes = capture.retained_bytes();
        if let Some(replaced) = self.captures.insert(sequence, capture) {
            replaced.disable_capture();
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(replaced.retained_bytes());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(capture_bytes);
        self.revision = self.revision.wrapping_add(1);
        self.enforce_cached_retention()
    }

    pub fn get(&self, sequence: CaptureSequence) -> Option<Arc<CaptureRecord>> {
        self.captures.get(&sequence).map(Arc::clone)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Arc<CaptureRecord>> {
        self.captures.values()
    }

    pub fn len(&self) -> usize {
        self.captures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.captures.is_empty()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn remove_sequences(
        &mut self,
        sequences: impl IntoIterator<Item = CaptureSequence>,
    ) -> usize {
        let unique = sequences.into_iter().collect::<BTreeSet<_>>();
        let mut removed = 0;
        for sequence in unique {
            if let Some(capture) = self.captures.remove(&sequence) {
                capture.disable_capture();
                self.retained_bytes = self.retained_bytes.saturating_sub(capture.retained_bytes());
                removed += 1;
            }
        }
        if removed > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        removed
    }

    pub fn clear(&mut self) -> usize {
        let removed = self.captures.len();
        for capture in self.captures.values() {
            capture.disable_capture();
        }
        self.captures = BTreeMap::new();
        self.retained_bytes = 0;
        if removed > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        removed
    }

    fn enforce_cached_retention(&mut self) -> Vec<CaptureSequence> {
        let mut evicted = Vec::new();
        while self.captures.len() > self.policy.max_records
            || self.retained_bytes > self.policy.max_bytes
        {
            let Some(sequence) = self.evict_oldest() else {
                break;
            };
            evicted.push(sequence);
        }
        evicted
    }

    pub fn evict_oldest(&mut self) -> Option<CaptureSequence> {
        let (&sequence, _) = self.captures.first_key_value()?;
        let capture = self.captures.remove(&sequence)?;
        capture.disable_capture();
        self.retained_bytes = self.retained_bytes.saturating_sub(capture.retained_bytes());
        self.revision = self.revision.wrapping_add(1);
        Some(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedExchange;
    use hyper::Method;

    #[test]
    fn iteration_is_sequence_order_even_when_inserted_in_reverse() {
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        store.insert(capture(2, "two"));
        store.insert(capture(0, "zero"));
        store.insert(capture(1, "one"));

        assert_eq!(
            store
                .iter()
                .map(|capture| capture.sequence().value())
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn retention_evicts_oldest_sequence_by_count() {
        let mut store = CaptureStore::new(CaptureRetentionPolicy {
            max_records: 2,
            max_bytes: usize::MAX,
        });
        store.insert(capture(2, "two"));
        store.insert(capture(1, "one"));
        let evicted = store.insert(capture(3, "three"));

        assert_eq!(evicted, vec![CaptureSequence::new(1)]);
        assert!(store.get(CaptureSequence::new(1)).is_none());
        assert!(store.get(CaptureSequence::new(2)).is_some());
    }

    #[test]
    fn retention_evicts_until_byte_budget_is_satisfied() {
        let first = capture(0, "first");
        let second = capture(1, "second");
        let max_bytes = second.retained_bytes();
        let mut store = CaptureStore::new(CaptureRetentionPolicy {
            max_records: 10,
            max_bytes,
        });

        store.insert(first);
        let evicted = store.insert(second);

        assert_eq!(evicted, vec![CaptureSequence::new(0)]);
        assert_eq!(store.len(), 1);
        assert!(store.retained_bytes() <= max_bytes);
    }

    fn capture(sequence: u64, uri: &str) -> Arc<CaptureRecord> {
        CaptureRecord::from_completed(CapturedExchange {
            sequence: CaptureSequence::new(sequence),
            method: Method::GET,
            uri: uri.to_string(),
            mapped_uri: None,
            local_path: None,
            status: None,
            req_headers: Vec::new(),
            res_headers: Vec::new(),
            req_body: None,
            res_body: None,
        })
    }
}
