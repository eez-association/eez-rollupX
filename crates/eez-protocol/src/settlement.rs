//! Settlement transaction-framing helpers shared by the composer and proof
//! signer.

use alloy_primitives::{Address, B256};

/// A sync-block transaction is a system transaction iff its recovered signer
/// matches the deployment's system address and it targets `EEZL2`.
/// The proof signer and composer must use the same deployment values so their
/// pair-end positions, and therefore per-effect settlement block hashes, agree.
#[must_use]
pub fn is_system_tx(
    signer: Address,
    to: Option<Address>,
    expected_system_address: Address,
    eezl2_address: Address,
) -> bool {
    signer == expected_system_address && to == Some(eezl2_address)
}

/// Pair-end tx positions in a sync block — one per settled cross-chain effect,
/// in tx order. A position ends a pair iff it is a USER tx (the `user` half of
/// an outbound `[load | user]` pair) OR a SYSTEM tx followed by a system tx / the
/// block end (a standalone inbound system tx). Single source of truth shared by
/// the composer (per-effect `RootUpdate.newRoot` stitch) and proof signer
/// (effect/checkpoint binding), so the settled block hashes line up entry-for-entry.
#[must_use]
pub fn pair_end_positions(is_system: &[bool]) -> Vec<usize> {
    (0..is_system.len())
        .filter(|&i| !is_system[i] || is_system.get(i + 1).copied().unwrap_or(true))
        .collect()
}

/// Assign execution checkpoints to settlement entries without relying on a
/// failed L1 entry to persist its root update.
///
/// `effects` contains each settlement entry's L2 checkpoint and claimed L1
/// execution outcome in entry order. A leading run of failed entries is
/// carried by the successful anchor. Every later successful entry carries its
/// own checkpoint and those of the immediately following failed entries.
/// Failed entries receive a no-op commitment (`currentRoot == newRoot` after chain
/// stitching), because `EEZ.sol` rolls their root updates back together with
/// the intentionally reverted call.
///
/// The returned tuple is `(anchor_new_root, effect_new_roots)`.
#[must_use]
pub fn settlement_root_carriers(
    pre_settling_root: B256,
    effects: impl IntoIterator<Item = (B256, bool)>,
) -> (B256, Vec<B256>) {
    let effects = effects.into_iter().collect::<Vec<_>>();
    let mut anchor_new_root = pre_settling_root;
    let mut effect_new_roots = vec![pre_settling_root; effects.len()];
    let mut active_success = None;

    // Assign every checkpoint to the most recent successful entry, or to the
    // anchor while the sequence still consists only of failed entries.
    for (effect_index, (checkpoint, success)) in effects.iter().copied().enumerate() {
        if success {
            active_success = Some(effect_index);
        }
        if let Some(carrier_index) = active_success {
            effect_new_roots[carrier_index] = checkpoint;
        } else {
            anchor_new_root = checkpoint;
        }
    }

    // Failed entries must claim the block hash already persisted by their carrier.
    // This also makes their root update a no-op when the chain is stitched.
    let mut persisted_root = anchor_new_root;
    for (effect_index, (_, success)) in effects.iter().copied().enumerate() {
        if success {
            persisted_root = effect_new_roots[effect_index];
        } else {
            effect_new_roots[effect_index] = persisted_root;
        }
    }

    (anchor_new_root, effect_new_roots)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use alloy_primitives::B256;

    use super::{is_system_tx, pair_end_positions, settlement_root_carriers};

    #[test]
    fn system_transaction_identity_uses_deployment_addresses() {
        let system_address = address!("1111111111111111111111111111111111111111");
        let eezl2_address = address!("2222222222222222222222222222222222222222");

        assert!(is_system_tx(
            system_address,
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            address!("3333333333333333333333333333333333333333"),
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            system_address,
            Some(address!("4444444444444444444444444444444444444444")),
            system_address,
            eezl2_address,
        ));
    }

    /// A pair ends at every user (non-system) tx, at a system tx followed by a
    /// system tx, and at the last tx regardless. A system tx followed by a user
    /// tx is NOT a pair end — it pairs with that user tx.
    #[test]
    fn pair_end_positions_cases() {
        assert_eq!(pair_end_positions(&[]), Vec::<usize>::new());
        // all user txs → each ends its own pair
        assert_eq!(pair_end_positions(&[false, false, false]), vec![0, 1, 2]);
        // system → user: only the user (index 1) ends the pair
        assert_eq!(pair_end_positions(&[true, false]), vec![1]);
        // system → system: the first system is a standalone pair end; the last
        // tx is always a pair end
        assert_eq!(pair_end_positions(&[true, true]), vec![0, 1]);
        // user then trailing system (last) → both are pair ends
        assert_eq!(pair_end_positions(&[false, true]), vec![0, 1]);
        // two outbound [load|user] pairs → ends at each user
        assert_eq!(pair_end_positions(&[true, false, true, false]), vec![1, 3]);
        // a lone system tx (also the last) → pair end
        assert_eq!(pair_end_positions(&[true]), vec![0]);
    }

    #[test]
    fn root_carriers_absorb_failed_effect_checkpoints() {
        let pre = B256::repeat_byte(0x10);
        let roots = (1..=5).map(B256::with_last_byte).collect::<Vec<_>>();

        let (anchor, entries) = settlement_root_carriers(
            pre,
            roots.iter().copied().zip([false, false, true, false, true]),
        );

        assert_eq!(anchor, roots[1]);
        assert_eq!(entries, [roots[1], roots[1], roots[3], roots[3], roots[4]]);
    }

    #[test]
    fn root_carriers_preserve_the_direct_success_path() {
        let pre = B256::repeat_byte(0x10);
        let first = B256::repeat_byte(0x11);
        let second = B256::repeat_byte(0x12);

        let (anchor, entries) = settlement_root_carriers(pre, [(first, true), (second, true)]);

        assert_eq!(anchor, pre);
        assert_eq!(entries, [first, second]);
    }
}
