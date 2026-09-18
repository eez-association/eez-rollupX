//! Ordinary sequencing reserves the entire shortest postable interval before
//! accepting data. It uses the same outgoing network rules as final posting.

use eez_payload_codec::BlockDaSizer;

use super::*;

fn interval_bounds(next: u64, target: u64, k: u64) -> Result<(u64, u64), String> {
    if k == 0 {
        return Err("sequencing grid is zero".into());
    }
    let residue = target % k;
    let offset = if next % k <= residue {
        residue - next % k
    } else {
        k - (next % k - residue)
    };
    let boundary = next
        .checked_add(offset)
        .ok_or("sequencing boundary overflow")?;
    Ok((boundary.saturating_sub(k), boundary))
}

fn allowance_through_future_forks(
    timestamp: u64,
    activations: impl Iterator<Item = u64>,
    resolve: impl Fn(u64) -> Result<usize, String>,
) -> Result<usize, String> {
    let mut allowance = resolve(timestamp)?;
    for activation in activations.filter(|activation| *activation > timestamp) {
        allowance = allowance.min(resolve(activation)?);
    }
    Ok(allowance)
}

fn item_limit(
    capacity: usize,
    previous: usize,
    reserved: usize,
    total_blocks: usize,
) -> Result<usize, String> {
    let fixed = previous
        .checked_add(reserved)
        .ok_or("sequencing byte accounting overflow")?;
    let fits = |item: usize| -> Result<bool, String> {
        let payload = eez_payload_codec::derivable::interval_payload_bound(
            total_blocks,
            fixed
                .checked_add(item)
                .ok_or("sequencing byte accounting overflow")?,
        )
        .map_err(|e| e.to_string())?;
        Ok(eez_blob_codec::native_stream_len(payload, 0).map_err(|e| e.to_string())? <= capacity)
    };
    if !fits(0)? {
        return Err(
            "sequencing_da_blocked: retained interval already exceeds the active allowance".into(),
        );
    }
    let (mut low, mut high) = (0, capacity);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(mid)? {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok(low)
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    pub(super) fn sequencing_da_limit(
        &self,
        rollup_id: u64,
        parent: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
        fresh: bool,
    ) -> Result<usize, String> {
        if let Some(source) = &self.inner.witness_source {
            source.ensure_ready()?;
        }
        let policy = &self.inner.policy;
        let rollup = self
            .inner
            .rollups
            .get(&rollup_id)
            .ok_or("unknown sequencing rollup")?;
        let timing = self.inner.emission.timing;
        let k = u64::from(timing.k());
        let next = parent
            .number()
            .checked_add(1)
            .ok_or("sequencing height overflow")?;
        let (base, boundary) = if fresh {
            (
                parent.number(),
                next.checked_add(k - 1)
                    .ok_or("sequencing boundary overflow")?,
            )
        } else {
            interval_bounds(
                next,
                policy
                    .sequencing_target(rollup_id)
                    .ok_or("sequencing scheduler target unavailable")?,
                k,
            )?
        };
        let remaining = boundary - next;
        let boundary_time = timestamp
            .checked_add(
                remaining
                    .checked_mul(timing.l2_block_time().as_secs())
                    .ok_or("sequencing timestamp overflow")?,
            )
            .ok_or("sequencing timestamp overflow")?;
        let chain = self.inner.cross_chain.local.l1_entry.chain_provider();
        let head_number = chain
            .headers
            .best_block_number()
            .map_err(|e| e.to_string())?;
        let head = chain
            .headers
            .header_by_number(head_number)
            .map_err(|e| e.to_string())?
            .ok_or("sequencing L1 head unavailable")?;
        let target_time = boundary_time.max(
            head.timestamp
                .checked_add(timing.l1_block_time().as_secs())
                .ok_or("sequencing L1 timestamp overflow")?,
        );
        // Accepted data may wait for a later fork. Reserve against announced
        // reductions as well; future increases do not constrain current capacity.
        let spec = chain.evm_config.chain_spec();
        let activations = spec
            .blob_params
            .scheduled
            .iter()
            .map(|(timestamp, _)| *timestamp)
            .chain(
                spec.hardforks
                    .forks_iter()
                    .filter_map(|(_, condition)| condition.as_timestamp()),
            );
        let blobs = allowance_through_future_forks(target_time, activations, |timestamp| {
            self.resolved_blob_allowance(timestamp, 0, false)
        })?;
        let capacity = blobs
            .checked_mul(eez_blob_codec::DATA_BYTES_PER_BLOB)
            .ok_or("sequencing capacity overflow")?;
        let empty = BlockDaSizer::new(eez_driver::BUILDER_EXTRA_DATA)
            .item_bytes()
            .map_err(|e| e.to_string())?;
        let mut previous = 0usize;
        if !fresh {
            if next == boundary {
                // The normal driver emits a prebuilt empty Sync here. Also
                // prevent an independent engine payload job from filling it.
                return Ok(empty);
            }
            let mut hash = parent.hash();
            let mut number = parent.number();
            while number > base {
                let block = rollup
                    .l2_provider
                    .find_block_by_hash(hash, BlockSource::Any)
                    .map_err(|e| e.to_string())?
                    .ok_or("sequencing ancestor unavailable")?;
                if block.header().number != number {
                    return Err("sequencing ancestor number mismatch".into());
                }
                let encoded = self
                    .cached_block_rlp(rollup_id, hash)
                    .unwrap_or_else(|| alloy_rlp::encode(&block));
                previous = previous
                    .checked_add(
                        BlockDaSizer::retained_item_bytes(&encoded).map_err(|e| e.to_string())?,
                    )
                    .ok_or("sequencing prefix overflow")?;
                hash = block.header().parent_hash;
                number -= 1;
            }
            if base > rollup.l1_head.cursor() {
                // A just-committed parent need not have reached the provider's
                // height/header indexes yet. Use the supplied header directly,
                // or the pending-aware block lookup for an older ancestor.
                let transactions_root = if base == parent.number() {
                    parent.transactions_root
                } else {
                    let block = rollup
                        .l2_provider
                        .find_block_by_hash(hash, BlockSource::Any)
                        .map_err(|e| e.to_string())?
                        .ok_or("sequencing boundary unavailable")?;
                    if block.header().number != base {
                        return Err("sequencing boundary number mismatch".into());
                    }
                    block.header().transactions_root
                };
                if transactions_root != alloy_consensus::constants::EMPTY_ROOT_HASH
                    && !rollup
                        .optimistic
                        .pending_attempt()
                        .is_some_and(|a| a.sync_height == base && a.manifest.sync_hash == hash)
                {
                    return Err("sequencing_da_blocked: preceding boundary is nonempty and has no settlement ownership".into());
                }
            }
        }
        let reserve_blocks = if fresh { k - 1 } else { remaining };
        let reserved = usize::try_from(reserve_blocks)
            .ok()
            .and_then(|n| n.checked_mul(empty))
            .ok_or("sequencing reservation overflow")?;
        let limit = item_limit(
            capacity,
            previous,
            reserved,
            usize::try_from(k).map_err(|_| "sequencing interval count overflow")?,
        )?;
        if limit < empty {
            return Err("sequencing_da_blocked: allowance cannot reserve an empty interval".into());
        }
        tracing::event!(name: "eez.composer.sequencing.da_budget", tracing::Level::DEBUG,
            rollup_id, parent = %parent.hash(), boundary, target_time, blobs, capacity,
            previous, reserved, maximum_block_item_bytes = limit, fresh,
            "resolved ordinary sequencing data budget");
        Ok(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_bounds_follow_every_scheduler_residue_and_check_overflow() {
        for k in [2, 5, 6, 12] {
            for residue in 0..k {
                for next in 1..=(k * 4) {
                    let (base, boundary) = interval_bounds(next, 10 * k + residue, k).unwrap();
                    assert!(base < next || next == boundary);
                    assert!(boundary >= next && boundary - next < k);
                    assert_eq!(boundary % k, residue);
                    assert_eq!(boundary - base, k.min(boundary));
                }
            }
        }
        assert!(interval_bounds(1, 1, 0).is_err());
        assert!(interval_bounds(u64::MAX, 0, 2).is_err());
    }

    #[test]
    fn future_reductions_constrain_admission_but_old_limits_and_future_increases_do_not() {
        let resolve = |timestamp| {
            Ok(match timestamp {
                0..10 => 9,
                10..20 => 6,
                20..30 => 2,
                _ => 10,
            })
        };
        for timestamp in [0, 9, 10, 19, 20, 29] {
            assert_eq!(
                allowance_through_future_forks(timestamp, [30, 20, 10].into_iter(), resolve)
                    .unwrap(),
                2
            );
        }
        assert_eq!(
            allowance_through_future_forks(30, [10, 20, 30, 40].into_iter(), resolve).unwrap(),
            10
        );
        assert!(
            allowance_through_future_forks(0, [10].into_iter(), |t| if t == 10 {
                Err("unknown fork rules".into())
            } else {
                Ok(6)
            })
            .is_err()
        );
        assert!(
            allowance_through_future_forks(0, [].into_iter(), |_| Err("no current rules".into()))
                .is_err()
        );
    }

    #[test]
    fn interval_budget_accounts_for_framing_and_all_reserved_blocks() {
        let empty = BlockDaSizer::new(&[]).item_bytes().unwrap();
        for blobs in [1, 2, 6, 9, 10] {
            for k in [5, 6, 12] {
                let capacity = blobs * eez_blob_codec::DATA_BYTES_PER_BLOB;
                let reserved = (k - 1) * empty;
                let limit = item_limit(capacity, 0, reserved, k).unwrap();
                let length = |items| {
                    eez_blob_codec::native_stream_len(
                        eez_payload_codec::derivable::interval_payload_bound(k, items).unwrap(),
                        0,
                    )
                    .unwrap()
                };
                assert!(length(limit + reserved) <= capacity);
                assert!(length(limit + reserved + 1) > capacity);
                assert_eq!(
                    item_limit(capacity, empty, reserved - empty, k).unwrap(),
                    limit
                );
            }
        }
        assert!(item_limit(1, 0, empty, 2).is_err());
        assert!(item_limit(1, usize::MAX, 1, 2).is_err());
    }
}
