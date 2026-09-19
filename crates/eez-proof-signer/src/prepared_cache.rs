//! Volatile, byte-accounted cache of fully validated ordinary block facts.
//!
//! Cache membership is never authority. Facts are keyed by validation identity
//! and full block hash; range resolution rechecks number, endpoint, parent and
//! state-root continuity before returning immutable facts to a final request.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use alloy_primitives::B256;

use crate::validate::PreparedOrdinaryBlock;

pub(crate) const DEFAULT_MAX_ENTRIES: usize = 131_072;
pub(crate) const DEFAULT_MAX_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug)]
struct Entry {
    fact: PreparedOrdinaryBlock,
    bytes: usize,
    last_used: u64,
    reservations: u8,
}

#[derive(Debug)]
struct Lease {
    identity: B256,
    keys: HashSet<(B256, B256)>,
    expires: Instant,
}

#[derive(Debug)]
pub(crate) struct PreparedCache {
    entries: HashMap<(B256, B256), Entry>,
    accounted_bytes: usize,
    maximum_entries: usize,
    maximum_bytes: usize,
    clock: u64,
    lease_nonce: u64,
    lease_epoch: B256,
    leases: HashMap<B256, Lease>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum CacheError {
    #[error("prepared range is empty")]
    Empty,
    #[error("prepared range contains another validation identity")]
    Identity,
    #[error("prepared range is not contiguous at block {number}")]
    Discontinuous { number: u64 },
    #[error("prepared range state roots do not telescope at block {number}")]
    StateRoot { number: u64 },
    #[error("prepared block {number} requires {required} bytes, cache limit is {maximum}")]
    Oversized {
        number: u64,
        required: usize,
        maximum: usize,
    },
    #[error("prepared range cannot fit cache capacity")]
    Capacity,
    #[error("cached range count {count} exceeds 65535")]
    Count { count: u64 },
    #[error("cached ordinary block {number} ({hash}) is missing")]
    Miss { number: u64, hash: B256 },
    #[error("cached range endpoint or identity mismatch")]
    Endpoint,
}

impl PreparedCache {
    pub(crate) fn new(maximum_entries: usize, maximum_bytes: usize) -> eyre::Result<Self> {
        eyre::ensure!(
            maximum_entries > 0,
            "prepared cache entry limit must be nonzero"
        );
        eyre::ensure!(
            maximum_bytes > 0,
            "prepared cache byte limit must be nonzero"
        );
        let startup_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let lease_epoch = alloy_primitives::keccak256(
            [
                b"eez.prepared-lease-epoch".as_slice(),
                &startup_nanos.to_be_bytes(),
                &std::process::id().to_be_bytes(),
            ]
            .concat(),
        );
        Ok(Self {
            entries: HashMap::new(),
            accounted_bytes: 0,
            maximum_entries,
            maximum_bytes,
            clock: 0,
            lease_nonce: 0,
            lease_epoch,
            leases: HashMap::new(),
        })
    }

    pub(crate) fn with_defaults() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
            .expect("nonzero static prepared-cache limits")
    }

    /// Publish one completely validated contiguous range atomically. Existing
    /// identical facts are replaced only after all capacity checks succeed.
    #[cfg(test)]
    pub(crate) fn insert_range(
        &mut self,
        facts: Vec<PreparedOrdinaryBlock>,
    ) -> Result<(), CacheError> {
        self.expire_leases(Instant::now());
        self.insert_range_unexpired(facts)
    }

    fn insert_range_unexpired(
        &mut self,
        facts: Vec<PreparedOrdinaryBlock>,
    ) -> Result<(), CacheError> {
        verify_facts(&facts)?;
        for fact in &facts {
            let bytes = fact.retained_bytes();
            if bytes > self.maximum_bytes {
                return Err(CacheError::Oversized {
                    number: fact.number(),
                    required: bytes,
                    maximum: self.maximum_bytes,
                });
            }
        }

        let incoming = facts
            .iter()
            .map(|fact| (fact.validation_identity(), fact.block_hash()))
            .collect::<HashSet<_>>();
        let replacement_bytes = incoming
            .iter()
            .filter_map(|key| self.entries.get(key).map(|entry| entry.bytes))
            .sum::<usize>();
        let incoming_bytes = facts
            .iter()
            .map(PreparedOrdinaryBlock::retained_bytes)
            .sum::<usize>();
        let mut projected_entries =
            self.entries.len() + facts.len() - replacement_bytes_count(&self.entries, &incoming);
        let mut projected_bytes = self.accounted_bytes - replacement_bytes + incoming_bytes;

        let mut candidates = self
            .entries
            .iter()
            .filter(|(key, entry)| !incoming.contains(key) && entry.reservations == 0)
            .map(|(key, entry)| (*key, entry.last_used, entry.bytes))
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(_, last_used, _)| *last_used);
        let mut evictions = Vec::new();
        for (key, _, bytes) in candidates {
            if projected_entries <= self.maximum_entries && projected_bytes <= self.maximum_bytes {
                break;
            }
            evictions.push(key);
            projected_entries -= 1;
            projected_bytes -= bytes;
        }
        if projected_entries > self.maximum_entries || projected_bytes > self.maximum_bytes {
            return Err(CacheError::Capacity);
        }

        for key in evictions {
            if let Some(removed) = self.entries.remove(&key) {
                self.accounted_bytes -= removed.bytes;
            }
        }
        for fact in facts {
            self.clock = self.clock.wrapping_add(1);
            let key = (fact.validation_identity(), fact.block_hash());
            let bytes = fact.retained_bytes();
            let reservations = self.entries.get(&key).map_or(0, |entry| entry.reservations);
            if let Some(replaced) = self.entries.insert(
                key,
                Entry {
                    fact,
                    bytes,
                    last_used: self.clock,
                    reservations,
                },
            ) {
                self.accounted_bytes -= replaced.bytes;
            }
            self.accounted_bytes += bytes;
        }
        debug_assert_eq!(self.entries.len(), projected_entries);
        debug_assert_eq!(self.accounted_bytes, projected_bytes);
        Ok(())
    }

    pub(crate) fn publish_with_lease(
        &mut self,
        facts: Vec<PreparedOrdinaryBlock>,
        requested_id: Option<B256>,
        identity: B256,
        lifetime: Duration,
    ) -> Result<B256, CacheError> {
        let now = Instant::now();
        self.expire_leases(now);
        if facts.len() > 65_535
            || facts
                .iter()
                .any(|fact| fact.validation_identity() != identity)
        {
            return Err(CacheError::Identity);
        }
        match requested_id {
            Some(id) => {
                let lease = self.leases.get(&id).ok_or(CacheError::Endpoint)?;
                if lease.identity != identity {
                    return Err(CacheError::Identity);
                }
                let new_keys = facts
                    .iter()
                    .filter(|fact| !lease.keys.contains(&(identity, fact.block_hash())))
                    .count();
                let count = lease.keys.len().saturating_add(new_keys);
                if count > 65_535 {
                    return Err(CacheError::Count {
                        count: count as u64,
                    });
                }
            }
            None if self.leases.len()
                >= usize::try_from(eez_control_rpc::MAX_PREPARATION_LEASES)
                    .expect("lease limit fits usize") =>
            {
                return Err(CacheError::Capacity);
            }
            None => {}
        }
        self.insert_range_unexpired(facts.clone())?;
        self.retain_facts_unexpired(requested_id, identity, &facts, lifetime, now)
    }

    /// Resolve a declared inclusive linked range. Count is charged and bounded
    /// before walking local links, so a short cache cannot hide unbounded work.
    pub(crate) fn resolve_range(
        &mut self,
        identity: B256,
        first_number: u64,
        first_hash: B256,
        last_number: u64,
        last_hash: B256,
    ) -> Result<Vec<PreparedOrdinaryBlock>, CacheError> {
        let count = last_number
            .checked_sub(first_number)
            .and_then(|n| n.checked_add(1))
            .ok_or(CacheError::Endpoint)?;
        if count > 65_535 {
            return Err(CacheError::Count { count });
        }
        if !self.entries.contains_key(&(identity, first_hash)) {
            return Err(CacheError::Miss {
                number: first_number,
                hash: first_hash,
            });
        }
        let capacity = usize::try_from(count).map_err(|_| CacheError::Count { count })?;
        let mut result = Vec::with_capacity(capacity);
        let mut number = last_number;
        let mut hash = last_hash;
        loop {
            self.clock = self.clock.wrapping_add(1);
            let entry = self
                .entries
                .get_mut(&(identity, hash))
                .ok_or(CacheError::Miss { number, hash })?;
            if entry.fact.number() != number {
                return Err(CacheError::Endpoint);
            }
            entry.last_used = self.clock;
            hash = entry.fact.parent_hash();
            result.push(entry.fact.clone());
            if number == first_number {
                break;
            }
            number = number.checked_sub(1).ok_or(CacheError::Endpoint)?;
        }
        result.reverse();
        if result
            .first()
            .is_none_or(|fact| fact.block_hash() != first_hash)
        {
            return Err(CacheError::Endpoint);
        }
        verify_facts(&result)?;
        Ok(result)
    }

    /// Create or atomically replace one bounded reservation. The returned ID is
    /// opaque resource ownership only; it never participates in validation.
    #[cfg(test)]
    pub(crate) fn retain_facts(
        &mut self,
        requested_id: Option<B256>,
        identity: B256,
        facts: &[PreparedOrdinaryBlock],
        lifetime: Duration,
    ) -> Result<B256, CacheError> {
        let now = Instant::now();
        self.expire_leases(now);
        self.retain_facts_unexpired(requested_id, identity, facts, lifetime, now)
    }

    fn retain_facts_unexpired(
        &mut self,
        requested_id: Option<B256>,
        identity: B256,
        facts: &[PreparedOrdinaryBlock],
        lifetime: Duration,
        now: Instant,
    ) -> Result<B256, CacheError> {
        let expires = now.checked_add(lifetime).ok_or(CacheError::Capacity)?;
        let keys = facts
            .iter()
            .map(|fact| (fact.validation_identity(), fact.block_hash()))
            .collect::<HashSet<_>>();
        if facts.len() > 65_535
            || facts
                .iter()
                .any(|fact| fact.validation_identity() != identity)
        {
            return Err(CacheError::Identity);
        }
        for key in &keys {
            if !self.entries.contains_key(key) {
                return Err(CacheError::Miss {
                    number: 0,
                    hash: key.1,
                });
            }
        }
        match requested_id {
            Some(id) => {
                let lease = self.leases.get(&id).ok_or(CacheError::Endpoint)?;
                if lease.identity != identity {
                    return Err(CacheError::Identity);
                }
                let additions = keys
                    .iter()
                    .filter(|key| !lease.keys.contains(key))
                    .copied()
                    .collect::<HashSet<_>>();
                let count = lease.keys.len().saturating_add(additions.len());
                if count > 65_535 {
                    return Err(CacheError::Count {
                        count: count as u64,
                    });
                }
                self.adjust_reservations(&additions, true)?;
                let lease = self.leases.get_mut(&id).expect("checked above");
                lease.keys.extend(additions);
                lease.expires = expires;
                Ok(id)
            }
            None => {
                if self.leases.len()
                    >= usize::try_from(eez_control_rpc::MAX_PREPARATION_LEASES)
                        .expect("lease limit fits usize")
                {
                    return Err(CacheError::Capacity);
                }
                self.lease_nonce = self.lease_nonce.wrapping_add(1);
                let id = alloy_primitives::keccak256(
                    [
                        b"eez.prepared-lease".as_slice(),
                        self.lease_epoch.as_slice(),
                        identity.as_slice(),
                        &self.lease_nonce.to_be_bytes(),
                    ]
                    .concat(),
                );
                self.adjust_reservations(&keys, true)?;
                self.leases.insert(
                    id,
                    Lease {
                        identity,
                        keys,
                        expires,
                    },
                );
                Ok(id)
            }
        }
    }

    pub(crate) fn release_lease(
        &mut self,
        lease_id: B256,
        identity: B256,
    ) -> Result<(), CacheError> {
        self.expire_leases(Instant::now());
        let lease = self.leases.remove(&lease_id).ok_or(CacheError::Endpoint)?;
        if lease.identity != identity {
            self.leases.insert(lease_id, lease);
            return Err(CacheError::Identity);
        }
        if let Err(error) = self.adjust_reservations(&lease.keys, false) {
            self.leases.insert(lease_id, lease);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn retain_exact(
        &mut self,
        lease_id: B256,
        identity: B256,
        facts: &[PreparedOrdinaryBlock],
        lifetime: Duration,
    ) -> Result<(), CacheError> {
        let now = Instant::now();
        self.expire_leases(now);
        let expires = now.checked_add(lifetime).ok_or(CacheError::Capacity)?;
        let lease = self.leases.get(&lease_id).ok_or(CacheError::Endpoint)?;
        if lease.identity != identity {
            return Err(CacheError::Identity);
        }
        let keys = facts
            .iter()
            .map(|fact| (fact.validation_identity(), fact.block_hash()))
            .collect::<HashSet<_>>();
        if keys.len() > 65_535
            || facts
                .iter()
                .any(|fact| fact.validation_identity() != identity)
        {
            return Err(CacheError::Identity);
        }
        for key in &keys {
            if !self.entries.contains_key(key) {
                return Err(CacheError::Miss {
                    number: 0,
                    hash: key.1,
                });
            }
        }
        // Validate the complete refcount transition before changing either the
        // lease or any entry. This keeps replacement atomic even if an internal
        // invariant is violated instead of leaving a partially reserved cache.
        for key in lease.keys.iter().chain(keys.difference(&lease.keys)) {
            let entry = self.entries.get(key).ok_or(CacheError::Endpoint)?;
            let after_release = if lease.keys.contains(key) {
                entry
                    .reservations
                    .checked_sub(1)
                    .ok_or(CacheError::Endpoint)?
            } else {
                entry.reservations
            };
            if keys.contains(key) {
                after_release.checked_add(1).ok_or(CacheError::Capacity)?;
            }
        }
        let previous = self.leases.remove(&lease_id).expect("checked above");
        self.adjust_reservations(&previous.keys, false)?;
        self.adjust_reservations(&keys, true)?;
        self.leases.insert(
            lease_id,
            Lease {
                identity,
                keys,
                expires,
            },
        );
        Ok(())
    }

    fn expire_leases(&mut self, now: Instant) {
        let expired = self
            .leases
            .iter()
            .filter_map(|(id, lease)| (lease.expires <= now).then_some(*id))
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(lease) = self.leases.remove(&id)
                && self.adjust_reservations(&lease.keys, false).is_err()
            {
                // Preserve ownership if an internal refcount invariant is ever
                // violated; silently dropping the lease would make the state
                // less recoverable. Normal construction makes this unreachable.
                self.leases.insert(id, lease);
                debug_assert!(false, "expired lease refcount invariant failed");
            }
        }
    }

    fn adjust_reservations(
        &mut self,
        keys: &HashSet<(B256, B256)>,
        add: bool,
    ) -> Result<(), CacheError> {
        for key in keys {
            let entry = self.entries.get(key).ok_or(CacheError::Endpoint)?;
            if add {
                entry
                    .reservations
                    .checked_add(1)
                    .ok_or(CacheError::Capacity)?;
            } else {
                entry
                    .reservations
                    .checked_sub(1)
                    .ok_or(CacheError::Endpoint)?;
            }
        }
        for key in keys {
            let entry = self
                .entries
                .get_mut(key)
                .expect("complete reservation transition prechecked");
            entry.reservations = if add {
                entry
                    .reservations
                    .checked_add(1)
                    .expect("reservation increment prechecked")
            } else {
                entry
                    .reservations
                    .checked_sub(1)
                    .expect("reservation decrement prechecked")
            };
        }
        Ok(())
    }

    pub(crate) fn usage(&self) -> (usize, usize) {
        (self.entries.len(), self.accounted_bytes)
    }

    pub(crate) fn lease_count(&mut self) -> usize {
        self.expire_leases(Instant::now());
        self.leases.len()
    }
}

fn replacement_bytes_count(
    entries: &HashMap<(B256, B256), Entry>,
    incoming: &HashSet<(B256, B256)>,
) -> usize {
    incoming
        .iter()
        .filter(|key| entries.contains_key(key))
        .count()
}

fn verify_facts(facts: &[PreparedOrdinaryBlock]) -> Result<(), CacheError> {
    let Some(first) = facts.first() else {
        return Err(CacheError::Empty);
    };
    for pair in facts.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if current.validation_identity() != first.validation_identity() {
            return Err(CacheError::Identity);
        }
        if previous.number().checked_add(1) != Some(current.number())
            || current.parent_hash() != previous.block_hash()
        {
            return Err(CacheError::Discontinuous {
                number: current.number(),
            });
        }
        if current.pre_state_root() != previous.post_state_root() {
            return Err(CacheError::StateRoot {
                number: current.number(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
