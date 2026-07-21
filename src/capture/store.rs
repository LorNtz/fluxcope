use std::collections::{BTreeMap, BTreeSet};

use super::{CaptureSequence, CapturedExchange};

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

#[derive(Debug)]
pub(crate) struct CaptureStore {
    captures: BTreeMap<CaptureSequence, CapturedExchange>,
    retained_bytes: usize,
    policy: CaptureRetentionPolicy,
}

impl CaptureStore {
    pub fn new(policy: CaptureRetentionPolicy) -> Self {
        Self {
            captures: BTreeMap::new(),
            retained_bytes: 0,
            policy,
        }
    }

    pub fn insert(&mut self, capture: CapturedExchange) -> Vec<CaptureSequence> {
        let sequence = capture.sequence;
        let capture_bytes = capture.retained_bytes();
        if let Some(replaced) = self.captures.insert(sequence, capture) {
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(replaced.retained_bytes());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(capture_bytes);
        self.enforce_retention()
    }

    pub fn get(&self, sequence: CaptureSequence) -> Option<&CapturedExchange> {
        self.captures.get(&sequence)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &CapturedExchange> {
        self.captures.values()
    }

    pub fn len(&self) -> usize {
        self.captures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.captures.is_empty()
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
                self.retained_bytes = self.retained_bytes.saturating_sub(capture.retained_bytes());
                removed += 1;
            }
        }
        removed
    }

    pub fn clear(&mut self) -> usize {
        let removed = self.captures.len();
        if removed > 0 {
            self.captures = BTreeMap::new();
            self.retained_bytes = 0;
        }
        removed
    }

    fn enforce_retention(&mut self) -> Vec<CaptureSequence> {
        let mut evicted = Vec::new();
        while self.captures.len() > self.policy.max_records
            || self.retained_bytes > self.policy.max_bytes
        {
            let Some((&sequence, _)) = self.captures.first_key_value() else {
                break;
            };
            let capture = self
                .captures
                .remove(&sequence)
                .expect("oldest capture came from the same map");
            self.retained_bytes = self.retained_bytes.saturating_sub(capture.retained_bytes());
            evicted.push(sequence);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                .map(|capture| capture.sequence.value())
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

    fn capture(sequence: u64, uri: &str) -> CapturedExchange {
        CapturedExchange {
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
        }
    }
}
