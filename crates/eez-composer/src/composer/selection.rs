//! FIFO eligibility within a finite snapshot. Admission limits count accepted
//! groups; a blocked source chain never consumes a position. Execution still
//! visits outbound candidates before inbound candidates.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use alloy_primitives::Address;

use crate::{Direction, HeldTx};

pub(super) struct Selection {
    chains: Vec<(Address, Direction)>,
    waiting: VecDeque<usize>,
    eligible: BTreeSet<usize>,
    blocked: HashSet<(Address, Direction)>,
    visited: HashSet<usize>,
    last_visited: Option<(Address, Direction)>,
    last_accepted: Option<usize>,
    remaining: usize,
    /// First underpriced nonce in each source chain, in snapshot order.
    fee_blocked: Vec<usize>,
}

impl Selection {
    pub fn new(candidates: &[HeldTx], limit: usize, l1_base_fee: u64, l2_base_fee: u64) -> Self {
        // Fees cannot improve through speculative execution within this slot.
        // Filter before allocating positions: discovering an underpriced inbound
        // during phase two is too late to admit another outbound in phase one.
        // Only defer the blocked nonce and its successors, never its predecessors.
        let mut fee_nonces: HashMap<_, u64> = HashMap::new();
        for tx in candidates {
            let base_fee = match tx.direction {
                Direction::Inbound => l1_base_fee,
                Direction::Outbound => l2_base_fee,
            };
            if tx.max_fee_per_gas < u128::from(base_fee) {
                fee_nonces
                    .entry((tx.sender, tx.direction))
                    .and_modify(|nonce| *nonce = (*nonce).min(tx.nonce))
                    .or_insert(tx.nonce);
            }
        }
        let mut waiting = VecDeque::new();
        let mut fee_blocked = Vec::new();
        for (index, tx) in candidates.iter().enumerate() {
            match fee_nonces.get(&(tx.sender, tx.direction)) {
                Some(nonce) if tx.nonce >= *nonce => {
                    if tx.nonce == *nonce {
                        fee_blocked.push(index);
                    }
                }
                _ => waiting.push_back(index),
            }
        }
        let mut selection = Self {
            chains: candidates
                .iter()
                .map(|tx| (tx.sender, tx.direction))
                .collect(),
            waiting,
            eligible: BTreeSet::new(),
            blocked: HashSet::new(),
            visited: HashSet::new(),
            last_visited: None,
            last_accepted: None,
            remaining: limit,
            fee_blocked,
        };
        selection.fill();
        selection
    }

    /// Count capacity was spent while an otherwise eligible source remains.
    pub fn count_limited(&self) -> bool {
        self.remaining == 0
            && self
                .waiting
                .iter()
                .any(|index| !self.blocked.contains(&self.chains[*index]))
    }

    pub fn fee_blocked(&self) -> &[usize] {
        &self.fee_blocked
    }

    fn fill(&mut self) {
        while self.eligible.len() < self.remaining {
            let Some(index) = self.waiting.pop_front() else {
                break;
            };
            if !self.blocked.contains(&self.chains[index]) {
                self.eligible.insert(index);
            }
        }
    }

    pub fn consider(&mut self, index: usize) -> bool {
        if !self.eligible.contains(&index) {
            return false;
        }
        self.visited.insert(index);
        self.last_visited = Some(self.chains[index]);
        true
    }

    pub fn accept(&mut self, index: usize) {
        assert!(self.eligible.remove(&index), "accept only eligible groups");
        self.last_accepted = Some(self.last_accepted.map_or(index, |last| last.max(index)));
        self.remaining -= 1;
    }

    /// Canonical outbound-first execution can spend the budget before an older
    /// eligible inbound. Only that kind of cut needs reduced FIFO lookahead.
    pub fn fifo_bypassed(&self) -> bool {
        self.eligible
            .first()
            .zip(self.last_accepted)
            .is_some_and(|(first, last)| *first < last)
    }

    pub fn discard(&mut self, index: usize) {
        self.eligible.remove(&index);
        self.fill();
    }

    pub fn block(&mut self, index: usize) {
        let chain = self.chains[index];
        self.blocked.insert(chain);
        self.eligible.retain(|i| self.chains[*i] != chain);
        self.fill();
    }

    /// Includes outbound work that became eligible only during the inbound
    /// phase. Its source chain gets first discovery priority next opportunity.
    pub fn resume_chain(&self) -> Option<(Address, Direction)> {
        if self.remaining == 0 {
            return None;
        }
        // A deadline can expire before a source has been classified. Resume
        // at another chain, not the next nonce of that same expensive source.
        let fresh_chain = |index: &usize| {
            !self.blocked.contains(&self.chains[*index])
                && Some(self.chains[*index]) != self.last_visited
        };
        self.eligible
            .iter()
            .copied()
            .chain(self.waiting.iter().copied())
            .find(|i| !self.visited.contains(i) && fresh_chain(i))
            .map(|i| self.chains[i])
            .or_else(|| {
                self.eligible
                    .iter()
                    .find(|i| !self.visited.contains(i))
                    .map(|i| self.chains[*i])
                    .or_else(|| {
                        self.waiting
                            .iter()
                            .find(|i| !self.blocked.contains(&self.chains[**i]))
                            .map(|i| self.chains[*i])
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, Bytes};

    fn tx(sender: u8, nonce: u64, direction: Direction) -> HeldTx {
        HeldTx {
            raw_tx: Bytes::new(),
            hash: B256::ZERO,
            sender: Address::repeat_byte(sender),
            nonce,
            direction,
            attempts: 0,
            max_fee_per_gas: 1,
            priority_fee_per_gas: 1,
        }
    }

    #[test]
    fn underpriced_inbounds_do_not_strand_ready_outbounds_after_the_phase_boundary() {
        let mut users = (0..361)
            .map(|nonce| tx(1, nonce, Direction::Inbound))
            .collect::<Vec<_>>();
        users.extend((0..17).map(|nonce| tx(2, nonce, Direction::Outbound)));
        let mut selection = Selection::new(&users, 30, 2, 1);
        assert_eq!(selection.fee_blocked(), &[0]);
        // Canonical outbound-first execution fills the window immediately;
        // it does not need to simulate an inbound to free these positions.
        for index in 361..378 {
            assert!(selection.consider(index));
            selection.accept(index);
        }
        for index in 0..361 {
            assert!(!selection.consider(index));
        }
        assert_eq!(selection.remaining, 13);
        assert!(!selection.fifo_bypassed());
        assert_eq!(selection.resume_chain(), None);
    }

    #[test]
    fn count_cut_requires_exhaustion_and_an_unblocked_remaining_candidate() {
        let mut users = vec![tx(1, 0, Direction::Inbound), tx(2, 0, Direction::Outbound)];
        let mut selection = Selection::new(&users, 1, 1, 1);
        assert!(!selection.count_limited());
        assert!(selection.consider(0));
        selection.accept(0);
        assert!(selection.count_limited());
        users[1].max_fee_per_gas = 0;
        let mut selection = Selection::new(&users, 1, 1, 1);
        selection.accept(0);
        assert!(
            !selection.count_limited(),
            "fee-blocked work is not a count cut"
        );
        let mut selection = Selection::new(&users[..1], 1, 1, 1);
        selection.accept(0);
        assert!(
            !selection.count_limited(),
            "exact exhaustion without remaining work is not a cut"
        );
    }

    #[test]
    fn fee_deferral_preserves_predecessors_and_independent_source_chains() {
        let mut users = vec![
            tx(1, 0, Direction::Inbound),
            tx(1, 1, Direction::Inbound),
            tx(1, 2, Direction::Inbound),
            tx(1, 0, Direction::Outbound),
            tx(2, 0, Direction::Inbound),
        ];
        users[1].max_fee_per_gas = 0;
        let mut selection = Selection::new(&users, 3, 1, 1);
        assert_eq!(selection.fee_blocked(), &[1]);
        for index in [3, 0, 4] {
            assert!(selection.consider(index));
            selection.accept(index);
        }
        assert!(!selection.consider(1));
        assert!(!selection.consider(2));
        assert_eq!(selection.remaining, 0);
    }

    #[test]
    fn target_fees_are_rechecked_for_each_snapshot_and_equal_caps_are_eligible() {
        let users = [tx(1, 0, Direction::Inbound), tx(2, 0, Direction::Outbound)];
        let mut expensive_l1 = Selection::new(&users, 1, 2, 1);
        assert!(expensive_l1.consider(1));
        assert!(!expensive_l1.consider(0));
        let mut cheaper_l1 = Selection::new(&users, 1, 1, 2);
        assert!(cheaper_l1.consider(0));
        assert!(!cheaper_l1.consider(1));

        let mut replacement = users.clone();
        replacement[0].max_fee_per_gas = 2;
        let mut replaced = Selection::new(&replacement, 1, 2, 1);
        assert!(replaced.consider(0));
        assert!(!replaced.consider(1));
        assert!(replaced.fee_blocked().is_empty());
        assert!(Selection::new(&users, 0, 0, 0).eligible.is_empty());
    }

    #[test]
    fn blocked_suffix_does_not_consume_limit_one_or_need_per_transaction_simulation() {
        let mut users = (0..10_000)
            .map(|n| tx(1, n, Direction::Outbound))
            .collect::<Vec<_>>();
        users.push(tx(2, 0, Direction::Inbound));
        let mut selection = Selection::new(&users, 1, 0, 0);
        assert!(selection.consider(0));
        selection.block(0);
        for index in 1..10_000 {
            assert!(!selection.consider(index));
        }
        assert!(selection.consider(10_000));
        selection.accept(10_000);
        assert_eq!(selection.remaining, 0);
    }

    #[test]
    fn inbound_priority_and_next_opportunity_preserve_canonical_phase_order() {
        let users = vec![tx(1, 0, Direction::Inbound), tx(2, 0, Direction::Outbound)];
        let mut selection = Selection::new(&users, 1, 0, 0);
        assert!(
            !selection.consider(1),
            "outbound cannot steal the older inbound's position"
        );
        assert!(selection.consider(0));
        selection.block(0);
        assert_eq!(
            selection.resume_chain(),
            Some((users[1].sender, Direction::Outbound))
        );
        // A later slot evaluates the new chain afresh; no cached ready flag.
        let mut next = Selection::new(&[users[1].clone(), users[0].clone()], 1, 0, 0);
        assert!(next.consider(0));
        next.accept(0);
    }

    #[test]
    fn accepted_groups_and_directions_keep_independent_nonce_chains() {
        let users = vec![
            tx(1, 0, Direction::Outbound),
            tx(1, 0, Direction::Inbound),
            tx(2, 0, Direction::Outbound),
        ];
        let mut selection = Selection::new(&users, 2, 0, 0);
        selection.block(0);
        assert!(selection.consider(2));
        selection.accept(2);
        assert!(selection.consider(1));
        selection.accept(1);
        assert!(!selection.consider(0));
        assert_eq!(selection.remaining, 0);
        assert!(!selection.fifo_bypassed());
    }

    #[test]
    fn only_bypassed_eligible_fifo_work_needs_capacity_backoff() {
        let users = vec![
            tx(1, 0, Direction::Inbound),
            tx(2, 0, Direction::Outbound),
            tx(3, 0, Direction::Inbound),
        ];
        let mut selection = Selection::new(&users, 3, 0, 0);
        assert!(!selection.fifo_bypassed());
        assert!(selection.consider(1));
        selection.accept(1);
        assert!(
            selection.fifo_bypassed(),
            "outbound bypassed the oldest inbound"
        );
        assert!(selection.consider(0));
        selection.accept(0);
        assert!(
            !selection.fifo_bypassed(),
            "remaining work is a FIFO suffix"
        );

        let mut blocked = Selection::new(&users, 3, 0, 0);
        blocked.block(0);
        blocked.accept(1);
        assert!(
            !blocked.fifo_bypassed(),
            "source waits do not measure capacity"
        );
    }

    #[test]
    fn discovery_deadline_resumes_at_another_chain_instead_of_the_same_senders_suffix() {
        let mut users = (0..10_000)
            .map(|n| tx(1, n, Direction::Outbound))
            .collect::<Vec<_>>();
        users.push(tx(2, 0, Direction::Outbound));
        let mut selection = Selection::new(&users, 64, 0, 0);
        assert!(selection.consider(0));
        // No condition was classified before the deadline. The next opportunity
        // must not repeatedly examine this chain's 9,999 successors first.
        assert_eq!(
            selection.resume_chain(),
            Some((users[10_000].sender, Direction::Outbound))
        );
    }
}
