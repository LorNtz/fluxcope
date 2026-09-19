#![cfg(unix)]

use super::{
    BodyCacheKey, DecodedBodyCache,
    body_test_support::{key, scope, status},
    should_cache_decoded_body,
};
use crate::capture::{
    BodySide, BodyStreamState, CaptureChange, CaptureChangeError, CaptureChangeKind,
    CaptureSequence, DecodedBytes,
};
use hyper::body::Bytes;

#[test]
fn decoded_cache_accepts_only_terminal_stream_states() {
    for (stream, expected) in [
        (BodyStreamState::Pending, false),
        (BodyStreamState::Streaming, false),
        (BodyStreamState::Complete, true),
        (BodyStreamState::Failed, true),
        (BodyStreamState::Cancelled, true),
    ] {
        assert_eq!(
            should_cache_decoded_body(&status(stream, 4)),
            expected,
            "{stream:?}"
        );
    }
}

#[test]
fn decoded_cache_keys_exact_generation_revision_side_and_representation() {
    let mut cache = DecodedBodyCache::new(64);
    let first = key(scope(19010), 3, BodySide::Request);
    let other_generation = key(scope(19011), 3, BodySide::Request);
    let other_revision = key(scope(19010), 4, BodySide::Request);
    let other_side = key(scope(19010), 3, BodySide::Response);
    cache.insert(first.clone(), Bytes::from_static(b"first"));

    assert_eq!(cache.get(&first).as_deref(), Some(b"first".as_slice()));
    assert!(cache.get(&other_generation).is_none());
    assert!(cache.get(&other_revision).is_none());
    assert!(cache.get(&other_side).is_none());
}

#[test]
fn decoded_cache_preserves_utf8_validity_and_decode_metadata() {
    let mut cache = DecodedBodyCache::new(64);
    let cache_key = key(scope(19010), 3, BodySide::Response);
    cache.insert_decoded(
        cache_key.clone(),
        DecodedBytes::new(
            Bytes::from_static(&[0xff, 0x00]),
            vec!["gzip".to_owned()],
            true,
        ),
    );

    let cached = cache.get_decoded(&cache_key).expect("decoded cache hit");
    assert!(!cached.is_utf8());
    assert_eq!(cached.encoding_chain, ["gzip"]);
    assert!(cached.output_limited);
}

#[test]
fn decoded_cache_replacement_and_lru_eviction_are_byte_accounted() {
    let mut cache = DecodedBodyCache::new(9);
    let a = key(scope(19010), 1, BodySide::Request);
    let b = key(scope(19010), 2, BodySide::Request);
    let c = key(scope(19010), 3, BodySide::Request);
    cache.insert(a.clone(), Bytes::from_static(b"aaa"));
    cache.insert(b.clone(), Bytes::from_static(b"bbb"));
    cache.insert(c.clone(), Bytes::from_static(b"ccc"));
    assert_eq!(cache.test_snapshot().bytes, 9);
    assert_eq!(cache.get(&a).as_deref(), Some(b"aaa".as_slice()));

    cache.insert(c.clone(), Bytes::from_static(b"CCCC"));
    assert_eq!(cache.test_snapshot().bytes, 7);
    assert!(cache.get(&b).is_none(), "least-recently-used entry evicted");
    assert_eq!(cache.get(&a).as_deref(), Some(b"aaa".as_slice()));
    assert_eq!(cache.get(&c).as_deref(), Some(b"CCCC".as_slice()));
}

#[test]
fn capture_changes_purge_obsolete_cache_entries_and_a_feed_gap_invalidates_all() {
    let instance = scope(19010);
    let mut cache = DecodedBodyCache::new(64);
    let rev1 = key(instance.clone(), 1, BodySide::Request);
    let rev2 = key(instance.clone(), 2, BodySide::Request);
    let unrelated = BodyCacheKey {
        capture_id: CaptureSequence::new(8),
        ..rev1.clone()
    };
    for key in [rev1.clone(), rev2.clone(), unrelated.clone()] {
        cache.insert(key, Bytes::from_static(b"body"));
    }
    cache.apply_change(CaptureChange {
        epoch: 1,
        sequence: CaptureSequence::new(7),
        revision: 2,
        kind: CaptureChangeKind::RecordUpdated,
    });
    assert!(cache.get(&rev1).is_none());
    assert!(cache.get(&rev2).is_some());
    assert!(cache.get(&unrelated).is_some());

    for kind in [
        CaptureChangeKind::RetentionEviction,
        CaptureChangeKind::ExplicitDelete,
    ] {
        cache.insert(rev2.clone(), Bytes::from_static(b"body"));
        cache.apply_change(CaptureChange {
            epoch: 2,
            sequence: CaptureSequence::new(7),
            revision: 2,
            kind,
        });
        assert!(cache.get(&rev2).is_none(), "{kind:?}");
    }

    cache.insert(rev2, Bytes::from_static(b"body"));
    cache.apply_change(CaptureChange {
        epoch: 3,
        sequence: CaptureSequence::new(0),
        revision: 0,
        kind: CaptureChangeKind::Clear,
    });
    assert_eq!(cache.test_snapshot().entries, 0);

    cache.insert(unrelated, Bytes::from_static(b"body"));
    cache.apply_feed_error(CaptureChangeError::Gap {
        expected_epoch: 4,
        oldest_available_epoch: 9,
    });
    assert_eq!(cache.test_snapshot().entries, 0);
    assert_eq!(cache.test_snapshot().bytes, 0);
}
