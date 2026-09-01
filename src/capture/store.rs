use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{CaptureChangeKind, CaptureRecord, CaptureSequence};

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
        if let Some(replaced) = self.captures.insert(sequence, Arc::clone(&capture)) {
            replaced.remove_from_store(CaptureChangeKind::ExplicitDelete);
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(replaced.retained_bytes());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(capture_bytes);
        self.revision = self.revision.wrapping_add(1);
        capture.bind_change_feed();
        self.enforce_cached_retention()
    }

    pub fn get(&self, sequence: CaptureSequence) -> Option<Arc<CaptureRecord>> {
        self.captures.get(&sequence).map(Arc::clone)
    }
    pub fn next_at_or_before(&self, sequence: CaptureSequence) -> Option<Arc<CaptureRecord>> {
        self.captures
            .range(..=sequence)
            .next_back()
            .map(|(_, capture)| Arc::clone(capture))
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

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub(crate) fn retention_policy(&self) -> CaptureRetentionPolicy {
        self.policy
    }

    pub fn remove_sequences(
        &mut self,
        sequences: impl IntoIterator<Item = CaptureSequence>,
    ) -> usize {
        let unique = sequences.into_iter().collect::<BTreeSet<_>>();
        let mut removed = 0;
        for sequence in unique {
            if let Some(capture) = self.captures.remove(&sequence) {
                capture.remove_from_store(CaptureChangeKind::ExplicitDelete);
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
            capture.remove_from_store(CaptureChangeKind::Clear);
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
        capture.remove_from_store(CaptureChangeKind::RetentionEviction);
        self.retained_bytes = self.retained_bytes.saturating_sub(capture.retained_bytes());
        self.revision = self.revision.wrapping_add(1);
        Some(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{
        BodySide, CaptureChangeError, CaptureChangeKind, CaptureHandle, CapturePolicy,
        CapturePublisher, CaptureSnapshotMode, CapturedExchange, RequestCaptureInput,
        ResponseCaptureInput,
    };
    use hyper::{HeaderMap, Method};
    use tokio::sync::mpsc;

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

    #[test]
    fn get_returns_the_stored_arc_by_sequence() {
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        let expected = capture(42, "stored");
        store.insert(Arc::clone(&expected));

        let actual = store
            .get(CaptureSequence::new(42))
            .expect("stored capture should be retrievable");

        assert!(Arc::ptr_eq(&actual, &expected));
        assert!(store.get(CaptureSequence::new(41)).is_none());
    }

    #[tokio::test]
    async fn admission_reports_mutations_that_happened_before_store_binding() {
        let (tx, mut rx) = mpsc::channel(1);
        let publisher = CapturePublisher::new(tx, CapturePolicy::default());
        let feed = publisher.change_feed();
        let mut subscriber = feed.subscribe();
        let headers = HeaderMap::new();
        let (handle, record) = admit(&publisher, &mut rx, &headers).await;

        handle.append(BodySide::Request, b"before insertion");
        assert_eq!(feed.epoch(), 0, "an unbound record is not yet public");
        let expected_revision = record.snapshot(CaptureSnapshotMode::MetadataOnly).revision;
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        store.insert(Arc::clone(&record));

        let admission = subscriber
            .recv()
            .await
            .expect("admission change should be published");
        assert_eq!(admission.kind, CaptureChangeKind::Admitted);
        assert_eq!(admission.sequence, record.sequence());
        assert_eq!(admission.revision, expected_revision);
        assert_eq!(feed.epoch(), admission.epoch);
    }

    #[tokio::test]
    async fn each_record_mutation_emits_one_ordered_change_to_every_subscriber() {
        let (tx, mut rx) = mpsc::channel(1);
        let publisher = CapturePublisher::new(tx, CapturePolicy::default());
        let feed = publisher.change_feed();
        let mut first = feed.subscribe();
        let mut second = feed.subscribe();
        let headers = HeaderMap::new();
        let (handle, record) = admit(&publisher, &mut rx, &headers).await;
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        store.insert(Arc::clone(&record));
        let first_admission = first.recv().await.expect("first admission");
        let second_admission = second.recv().await.expect("second admission");
        assert_eq!(first_admission, second_admission);

        let epoch_before = feed.epoch();
        handle.set_response(ResponseCaptureInput {
            status: 204,
            headers: &headers,
        });
        handle.append(BodySide::Request, b"x");
        handle.complete(BodySide::Request);
        assert_eq!(
            feed.epoch(),
            epoch_before + 3,
            "three externally visible revision mutations produce exactly three epochs"
        );

        let mut first_changes = Vec::new();
        let mut second_changes = Vec::new();
        for _ in 0..3 {
            first_changes.push(first.recv().await.expect("first subscriber change"));
            second_changes.push(second.recv().await.expect("second subscriber change"));
        }
        assert_eq!(first_changes, second_changes);
        assert_eq!(
            first_changes
                .iter()
                .map(|change| change.kind)
                .collect::<Vec<_>>(),
            vec![
                CaptureChangeKind::RecordUpdated,
                CaptureChangeKind::RecordUpdated,
                CaptureChangeKind::RecordUpdated,
            ]
        );
        assert_eq!(
            first_changes
                .iter()
                .map(|change| change.revision)
                .collect::<Vec<_>>(),
            vec![
                first_admission.revision + 1,
                first_admission.revision + 2,
                first_admission.revision + 3,
            ]
        );
        assert!(
            first_changes
                .windows(2)
                .all(|pair| pair[0].epoch + 1 == pair[1].epoch)
        );
    }

    #[tokio::test]
    async fn subscriber_reports_an_explicit_gap_after_change_ring_overflow() {
        const RETAINED_CHANGES: usize = 4_096;
        const PUBLISHED_CHANGES: usize = RETAINED_CHANGES + 1;

        let (tx, mut rx) = mpsc::channel(PUBLISHED_CHANGES);
        let publisher = CapturePublisher::new(
            tx,
            CapturePolicy {
                queue_capacity: PUBLISHED_CHANGES,
                max_concurrent_exchanges: PUBLISHED_CHANGES,
                ..CapturePolicy::default()
            },
        );
        let feed = publisher.change_feed();
        let mut lagged = feed.subscribe();
        let headers = HeaderMap::new();
        let mut store = CaptureStore::new(CaptureRetentionPolicy {
            max_records: PUBLISHED_CHANGES,
            max_bytes: usize::MAX,
        });

        for _ in 0..PUBLISHED_CHANGES {
            let (_handle, record) = admit(&publisher, &mut rx, &headers).await;
            store.insert(record);
        }

        assert_eq!(feed.epoch(), PUBLISHED_CHANGES as u64);
        assert_eq!(
            lagged.recv().await,
            Err(CaptureChangeError::Gap {
                expected_epoch: 1,
                oldest_available_epoch: 2,
            })
        );
    }

    #[tokio::test]
    async fn eviction_delete_and_clear_publish_distinguishable_removal_changes() {
        let (tx, mut rx) = mpsc::channel(3);
        let publisher = CapturePublisher::new(
            tx,
            CapturePolicy {
                queue_capacity: 3,
                max_concurrent_exchanges: 3,
                ..CapturePolicy::default()
            },
        );
        let feed = publisher.change_feed();
        let mut subscriber = feed.subscribe();
        let headers = HeaderMap::new();
        let mut store = CaptureStore::new(CaptureRetentionPolicy {
            max_records: 1,
            max_bytes: usize::MAX,
        });

        let (_first_handle, first_record) = admit(&publisher, &mut rx, &headers).await;
        let first_sequence = first_record.sequence();
        store.insert(first_record);
        assert_eq!(
            subscriber.recv().await.expect("first admission").kind,
            CaptureChangeKind::Admitted
        );

        let (_second_handle, second_record) = admit(&publisher, &mut rx, &headers).await;
        let second_sequence = second_record.sequence();
        store.insert(second_record);
        assert_eq!(
            subscriber.recv().await.expect("second admission").kind,
            CaptureChangeKind::Admitted
        );
        let eviction = subscriber.recv().await.expect("retention eviction");
        assert_eq!(eviction.sequence, first_sequence);
        assert_eq!(eviction.kind, CaptureChangeKind::RetentionEviction);

        assert_eq!(store.remove_sequences([second_sequence]), 1);
        let deletion = subscriber.recv().await.expect("explicit deletion");
        assert_eq!(deletion.sequence, second_sequence);
        assert_eq!(deletion.kind, CaptureChangeKind::ExplicitDelete);

        let (_third_handle, third_record) = admit(&publisher, &mut rx, &headers).await;
        let third_sequence = third_record.sequence();
        store.insert(third_record);
        assert_eq!(
            subscriber.recv().await.expect("third admission").kind,
            CaptureChangeKind::Admitted
        );
        assert_eq!(store.clear(), 1);
        let clearing = subscriber.recv().await.expect("clear removal");
        assert_eq!(clearing.sequence, third_sequence);
        assert_eq!(clearing.kind, CaptureChangeKind::Clear);
    }

    #[test]
    fn cursor_lookup_returns_newest_capture_at_or_before_the_requested_sequence() {
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        for sequence in [2, 5, 9, 14] {
            store.insert(capture(sequence, "cursor"));
        }

        let cases = [
            (0, None),
            (2, Some(2)),
            (4, Some(2)),
            (9, Some(9)),
            (13, Some(9)),
            (u64::MAX, Some(14)),
        ];
        for (cursor, expected) in cases {
            assert_eq!(
                store
                    .next_at_or_before(CaptureSequence::new(cursor))
                    .map(|capture| capture.sequence().value()),
                expected
            );
        }
    }

    #[test]
    fn cursor_lookup_returns_the_stored_arc_without_cloning_a_store_snapshot() {
        let mut store = CaptureStore::new(CaptureRetentionPolicy::default());
        let expected = capture(10_000, "bounded-last");
        store.insert(capture(1, "bounded-first"));
        store.insert(Arc::clone(&expected));

        let actual = store
            .next_at_or_before(CaptureSequence::new(20_000))
            .expect("newest capture");

        assert!(Arc::ptr_eq(&actual, &expected));
    }

    async fn admit(
        publisher: &CapturePublisher,
        rx: &mut mpsc::Receiver<Arc<CaptureRecord>>,
        headers: &HeaderMap,
    ) -> (CaptureHandle, Arc<CaptureRecord>) {
        let handle = publisher
            .try_start(RequestCaptureInput {
                method: Method::GET,
                original_uri: "https://example.com/",
                effective_uri: "https://example.com/",
                local_path: None,
                headers,
            })
            .expect("capture admitted");
        let record = rx.recv().await.expect("capture published");
        (handle, record)
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
