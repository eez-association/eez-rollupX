use super::*;

fn fact(
    identity: u8,
    number: u64,
    parent: u8,
    hash: u8,
    pre: u8,
    post: u8,
    bytes: usize,
) -> PreparedOrdinaryBlock {
    PreparedOrdinaryBlock::for_test(
        B256::repeat_byte(identity),
        number,
        B256::repeat_byte(hash),
        B256::repeat_byte(parent),
        B256::repeat_byte(pre),
        B256::repeat_byte(post),
        bytes,
    )
}

fn chain(identity: u8, start: u64, count: u64, bytes: usize) -> Vec<PreparedOrdinaryBlock> {
    (0..count)
        .map(|offset| {
            let number = start + offset;
            fact(
                identity,
                number,
                number as u8 - 1,
                number as u8,
                number as u8 - 1,
                number as u8,
                bytes,
            )
        })
        .collect()
}

#[test]
fn insertion_is_atomic_and_enforces_identity_hash_and_state_continuity() {
    let valid = chain(1, 1, 3, 8);
    let one = valid[0].retained_bytes();
    let mut cache = PreparedCache::new(3, one * 3).unwrap();
    cache.insert_range(valid.clone()).unwrap();
    let before = cache.usage();
    for invalid in [
        vec![valid[0].clone(), fact(2, 2, 1, 2, 1, 2, 8)],
        vec![valid[0].clone(), fact(1, 3, 1, 3, 1, 3, 8)],
        vec![valid[0].clone(), fact(1, 2, 1, 2, 9, 2, 8)],
    ] {
        assert!(cache.insert_range(invalid).is_err());
        assert_eq!(cache.usage(), before);
    }
    assert!(matches!(
        cache.insert_range(vec![fact(1, 4, 3, 4, 3, 4, one * 4)]),
        Err(CacheError::Oversized { .. })
    ));
    assert_eq!(cache.usage(), before);
}

#[test]
fn range_resolution_bounds_count_before_walk_and_checks_endpoints() {
    let facts = chain(7, 10, 4, 4);
    let mut cache = PreparedCache::new(10, 100_000).unwrap();
    cache.insert_range(facts.clone()).unwrap();
    let resolved = cache
        .resolve_range(
            B256::repeat_byte(7),
            10,
            B256::repeat_byte(10),
            13,
            B256::repeat_byte(13),
        )
        .unwrap();
    assert_eq!(resolved, facts);
    assert!(matches!(
        cache.resolve_range(B256::repeat_byte(7), 1, B256::ZERO, 65_536, B256::ZERO),
        Err(CacheError::Count { count: 65_536 })
    ));
    assert!(matches!(
        cache.resolve_range(
            B256::repeat_byte(8),
            10,
            B256::repeat_byte(10),
            13,
            B256::repeat_byte(13)
        ),
        Err(CacheError::Miss { number: 10, .. })
    ));
    assert!(matches!(
        cache.resolve_range(
            B256::repeat_byte(7),
            10,
            B256::repeat_byte(10),
            13,
            B256::ZERO
        ),
        Err(CacheError::Miss { number: 13, hash }) if hash == B256::ZERO
    ));
}

#[test]
fn lru_eviction_is_byte_and_entry_bounded_and_replacement_is_stable() {
    let sample = fact(1, 1, 0, 1, 0, 1, 1);
    let bytes = sample.retained_bytes();
    let mut cache = PreparedCache::new(2, bytes * 2).unwrap();
    cache.insert_range(vec![sample.clone()]).unwrap();
    cache.insert_range(vec![fact(1, 2, 1, 2, 1, 2, 1)]).unwrap();
    cache
        .resolve_range(
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1),
        )
        .unwrap();
    cache.insert_range(vec![fact(1, 3, 2, 3, 2, 3, 1)]).unwrap();
    assert!(matches!(
        cache.resolve_range(
            B256::repeat_byte(1),
            2,
            B256::repeat_byte(2),
            2,
            B256::repeat_byte(2)
        ),
        Err(CacheError::Miss { .. })
    ));
    assert!(
        cache
            .resolve_range(
                B256::repeat_byte(1),
                1,
                B256::repeat_byte(1),
                1,
                B256::repeat_byte(1)
            )
            .is_ok()
    );
    let before = cache.usage();
    cache.insert_range(vec![sample]).unwrap();
    assert_eq!(cache.usage(), before);
}

#[test]
fn leases_bound_ownership_protect_overlap_and_expire_without_authority() {
    let facts = chain(1, 1, 3, 1);
    let bytes = facts[0].retained_bytes();
    let mut cache = PreparedCache::new(3, bytes * 3).unwrap();
    cache.insert_range(facts.clone()).unwrap();
    let first = cache
        .retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[..2],
            Duration::from_secs(60),
        )
        .unwrap();
    let second = cache
        .retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[1..],
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(cache.lease_count(), 2);
    assert!(matches!(
        cache.retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[..1],
            Duration::from_secs(60)
        ),
        Err(CacheError::Capacity)
    ));
    cache.release_lease(first, B256::repeat_byte(1)).unwrap();
    assert_eq!(cache.lease_count(), 1);
    // The overlap remains reserved by the second lease, so only fact 1 is
    // eligible when a new entry needs space.
    cache.insert_range(vec![fact(1, 4, 3, 4, 3, 4, 1)]).unwrap();
    assert!(matches!(
        cache.resolve_range(
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1)
        ),
        Err(CacheError::Miss { .. })
    ));
    assert!(
        cache
            .resolve_range(
                B256::repeat_byte(1),
                2,
                B256::repeat_byte(2),
                3,
                B256::repeat_byte(3)
            )
            .is_ok()
    );
    cache.release_lease(second, B256::repeat_byte(1)).unwrap();

    let expiring = cache
        .retain_facts(None, B256::repeat_byte(1), &facts[1..2], Duration::ZERO)
        .unwrap();
    assert_eq!(cache.lease_count(), 0);
    assert!(matches!(
        cache.release_lease(expiring, B256::repeat_byte(1)),
        Err(CacheError::Endpoint)
    ));
}

#[test]
fn exact_lease_replacement_preserves_overlapping_reservations() {
    let facts = chain(1, 1, 3, 1);
    let bytes = facts[0].retained_bytes();
    let mut cache = PreparedCache::new(3, bytes * 3).unwrap();
    cache.insert_range(facts.clone()).unwrap();
    let first = cache
        .retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[..2],
            Duration::from_secs(60),
        )
        .unwrap();
    let second = cache
        .retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[1..],
            Duration::from_secs(60),
        )
        .unwrap();

    cache
        .retain_exact(
            first,
            B256::repeat_byte(1),
            &facts[2..],
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(
        cache.entries[&(B256::repeat_byte(1), B256::repeat_byte(1))].reservations,
        0
    );
    assert_eq!(
        cache.entries[&(B256::repeat_byte(1), B256::repeat_byte(2))].reservations,
        1
    );
    assert_eq!(
        cache.entries[&(B256::repeat_byte(1), B256::repeat_byte(3))].reservations,
        2
    );

    cache.release_lease(first, B256::repeat_byte(1)).unwrap();
    cache.insert_range(vec![fact(1, 4, 3, 4, 3, 4, 1)]).unwrap();
    assert!(matches!(
        cache.resolve_range(
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1),
            1,
            B256::repeat_byte(1)
        ),
        Err(CacheError::Miss { .. })
    ));
    assert!(
        cache
            .resolve_range(
                B256::repeat_byte(1),
                2,
                B256::repeat_byte(2),
                3,
                B256::repeat_byte(3)
            )
            .is_ok()
    );
    cache.release_lease(second, B256::repeat_byte(1)).unwrap();
}

#[test]
fn failed_refcount_transitions_leave_leases_and_entries_unchanged() {
    let facts = chain(1, 1, 3, 1);
    let bytes = facts[0].retained_bytes();
    let mut cache = PreparedCache::new(3, bytes * 3).unwrap();
    cache.insert_range(facts.clone()).unwrap();
    let lease = cache
        .retain_facts(
            None,
            B256::repeat_byte(1),
            &facts[..2],
            Duration::from_secs(60),
        )
        .unwrap();

    let first_key = (B256::repeat_byte(1), B256::repeat_byte(1));
    let second_key = (B256::repeat_byte(1), B256::repeat_byte(2));
    let third_key = (B256::repeat_byte(1), B256::repeat_byte(3));
    cache.entries.get_mut(&second_key).unwrap().reservations = 0;
    let lease_keys = cache.leases[&lease].keys.clone();

    assert_eq!(
        cache.retain_exact(
            lease,
            B256::repeat_byte(1),
            &facts[2..],
            Duration::from_secs(60)
        ),
        Err(CacheError::Endpoint)
    );
    assert_eq!(cache.leases[&lease].keys, lease_keys);
    assert_eq!(cache.entries[&first_key].reservations, 1);
    assert_eq!(cache.entries[&second_key].reservations, 0);
    assert_eq!(cache.entries[&third_key].reservations, 0);

    assert_eq!(
        cache.release_lease(lease, B256::repeat_byte(1)),
        Err(CacheError::Endpoint)
    );
    assert_eq!(cache.leases[&lease].keys, lease_keys);
    assert_eq!(cache.entries[&first_key].reservations, 1);
    assert_eq!(cache.entries[&second_key].reservations, 0);
    assert_eq!(cache.entries[&third_key].reservations, 0);
}
