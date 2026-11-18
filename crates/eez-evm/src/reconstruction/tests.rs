//! Re-finalization must preserve the full detached overlay, not merely its root.
use super::*;
use eez_primitives::{EEZL2_ADDRESS, SYSTEM_ADDRESS};
use evm2::{BaseEvmTypes, PendingState, bytecode::Bytecode, evm::InMemoryDB};
use std::collections::BTreeMap;

fn envelope() -> Recovered<evm2::ethereum::TxEnvelope> {
    Recovered::new_unchecked(
        evm2::ethereum::TxEnvelope::Legacy(Default::default()),
        Address::ZERO,
    )
}
fn carrier(pending: PendingState) -> Carrier<BaseEvmTypes> {
    Carrier::assemble(
        evm2::ExecutionConfig::for_spec_and_version(
            SpecId::OSAKA,
            *evm2::Version::base(SpecId::OSAKA),
        ),
        SpecId::OSAKA,
        Default::default(),
        (),
        &envelope(),
        Default::default(),
        pending,
    )
    .unwrap()
}
fn info(balance: u64, nonce: u64, code: bool) -> AccountInfo {
    let mut info = AccountInfo::empty().with_balance(U256::from(balance));
    info.nonce = nonce;
    if code {
        info.set_code(Bytecode::from_stored(alloy_primitives::bytes!("00")));
    }
    info
}
fn views(pending: &PendingState) -> BTreeMap<Address, String> {
    pending
        .account_changes()
        .map(|a| (a.address, format!("{a:?}")))
        .collect()
}

#[test]
fn loaded_and_unloaded_cleanup_match_prior_update_contract() {
    // Previous update_account_info contract: retain original/flags/storage, touch on
    // change, explicitly wipe loaded storage when a present account becomes absent.
    // Unloaded insertion has no transaction-lifetime flags or explicit storage wipe.
    for address in [EEZL2_ADDRESS, SYSTEM_ADDRESS] {
        for loaded in [false, true] {
            for present in [false, true] {
                for balance in [0, 7] {
                    for (nonce, code) in [(0, false), (3, false), (0, true)] {
                        let original = present.then(|| info(balance, nonce, code));
                        let mut db = InMemoryDB::default();
                        if let Some(value) = &original {
                            db.insert_account_info(&address, value.clone());
                        }
                        let mut pending = PendingState::default();
                        if loaded {
                            pending.insert_account(address, original.clone(), original.clone());
                            pending.insert_storage(
                                address,
                                U256::ONE,
                                U256::from(9),
                                U256::from(9),
                            );
                        }
                        let before = pending.clone();
                        clear_protocol_balances(&mut db, &mut pending).unwrap();
                        let changed = present && balance != 0;
                        if !changed {
                            assert_eq!(pending, before);
                            continue;
                        }
                        assert_eq!(pending.account_changes().count(), 1);
                        let a = pending.account_changes_for(&address).unwrap();
                        assert_eq!(a.original, original.as_ref());
                        let expected = (nonce != 0 || code).then(|| info(0, nonce, code));
                        assert_eq!(a.current, expected.as_ref());
                        assert_eq!(a.touched, loaded);
                        assert!(!a.created && !a.selfdestructed);
                        assert_eq!(a.storage.wiped, loaded && expected.is_none());
                        if loaded {
                            let slot = &a.storage.slots[&U256::ONE].value;
                            assert_eq!(slot.original, U256::from(9));
                            assert_eq!(
                                slot.current,
                                if expected.is_none() {
                                    U256::ZERO
                                } else {
                                    U256::from(9)
                                }
                            );
                        }
                        let once = pending.clone();
                        clear_protocol_balances(&mut db, &mut pending).unwrap();
                        assert_eq!(pending, once, "cleanup must be idempotent");
                    }
                }
            }
        }
    }
}

#[test]
fn finalized_lifecycle_metadata_survives_reconstruction_and_cleanup() {
    let created = Address::repeat_byte(0x71);
    let deleted = Address::repeat_byte(0x72);
    let delegated = Address::repeat_byte(0x73);
    let empty = Address::repeat_byte(0x74);
    let mut pending = PendingState::default();
    for address in [
        created,
        deleted,
        delegated,
        empty,
        Address::ZERO,
        EEZL2_ADDRESS,
    ] {
        let value = if address == EEZL2_ADDRESS {
            info(7, 1, false)
        } else {
            info(0, 0, false)
        };
        pending.insert_account(address, Some(value.clone()), Some(value));
        pending.insert_storage(address, U256::ONE, U256::from(9), U256::from(9));
    }
    let mut first = carrier(pending);
    {
        let mut account = first.0.state_mut().account(&created).unwrap();
        account.mark_created();
        account.set_nonce(1);
        account.touch();
    }
    {
        let mut account = first.0.state_mut().account(&deleted).unwrap();
        account.mark_created();
        account.mark_destructed();
    }
    first
        .0
        .state_mut()
        .account(&delegated)
        .unwrap()
        .set_delegation(created);
    first.0.state_mut().account(&empty).unwrap().touch();
    let finalized = first.detach(&envelope()).unwrap().pending_state;
    assert!(
        finalized
            .account_changes_for(&deleted)
            .unwrap()
            .selfdestructed
    );
    assert!(
        finalized
            .account_changes_for(&deleted)
            .unwrap()
            .storage
            .wiped
    );
    assert!(finalized.account_changes_for(&created).unwrap().created);
    assert!(finalized.account_info(&empty).is_none());
    assert!(finalized.account_info(&Address::ZERO).is_some());
    let again = carrier(finalized.clone())
        .detach(&envelope())
        .unwrap()
        .pending_state;
    assert_eq!(
        again, finalized,
        "all flags, originals, reads and slot values must survive"
    );
    let mut cleaned = finalized.clone();
    clear_protocol_balances(&mut InMemoryDB::default(), &mut cleaned).unwrap();
    let mut before = views(&finalized);
    before.remove(&EEZL2_ADDRESS);
    let mut after = views(&cleaned);
    after.remove(&EEZL2_ADDRESS);
    assert_eq!(
        after, before,
        "cleanup must not touch unrelated addresses or add a synthetic caller"
    );
    assert_eq!(
        cleaned.account_info(&EEZL2_ADDRESS).unwrap().balance,
        U256::ZERO
    );
}

#[test]
fn preloaded_changed_originals_and_created_flags_survive_cleanup() {
    let mut pending = PendingState::default();
    pending.insert_account(
        EEZL2_ADDRESS,
        Some(info(13, 0, false)),
        Some(info(7, 1, false)),
    );
    pending.insert_storage(EEZL2_ADDRESS, U256::ONE, U256::from(9), U256::from(4));
    let mut first = carrier(pending);
    first
        .0
        .state_mut()
        .account(&EEZL2_ADDRESS)
        .unwrap()
        .mark_created();
    let mut pending = first.detach(&envelope()).unwrap().pending_state;
    clear_protocol_balances(&mut InMemoryDB::default(), &mut pending).unwrap();
    let a = pending.account_changes_for(&EEZL2_ADDRESS).unwrap();
    assert!(a.created && a.touched && !a.selfdestructed);
    assert_eq!(a.original, Some(&info(13, 0, false)));
    assert_eq!(a.current, Some(&info(0, 1, false)));
    assert_eq!(a.storage.slots[&U256::ONE].value.current, U256::from(4));
    assert!(!a.storage.wiped);
}

#[test]
fn missing_cleanup_witness_does_not_consume_or_commit_pending_state() {
    let mut pending = PendingState::default();
    pending.insert_account(EEZL2_ADDRESS, None, Some(info(7, 1, false)));
    let before = pending.clone();
    let error = clear_protocol_balances(&mut RejectReads, &mut pending).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unexpected reconstruction database read")
    );
    assert_eq!(
        pending, before,
        "all fallible source reads precede taking the pending state"
    );
}

#[test]
fn reconstruction_rejects_other_execution_forks() {
    let result = Carrier::<BaseEvmTypes>::assemble(
        evm2::ExecutionConfig::for_spec_and_version(
            SpecId::PRAGUE,
            *evm2::Version::base(SpecId::PRAGUE),
        ),
        SpecId::PRAGUE,
        Default::default(),
        (),
        &envelope(),
        Default::default(),
        Default::default(),
    );
    assert!(matches!(result, Err(HandlerError::External(_))));
}

#[test]
fn real_transactions_survive_a_second_osaka_finalization() {
    use alloy_primitives::{Bytes, TxKind, bytes};
    let user = Address::repeat_byte(0x11);
    let target = Address::repeat_byte(0x22);
    let delegate = Address::repeat_byte(0x33);
    for case in 0..5 {
        let mut db = InMemoryDB::default();
        db.insert_account_info(&user, info(1_000_000, 0, false));
        db.insert_account_info(&target, info(0, 0, false));
        let code = Bytecode::from_stored(bytes!("600160005500"));
        if case == 3 {
            db.insert_account_info(
                &target,
                AccountInfo::empty().with_code(Bytecode::new_eip7702(delegate)),
            );
            db.insert_account_info(&delegate, AccountInfo::empty().with_code(code.clone()));
        } else if case == 0 {
            db.insert_account_info(&target, AccountInfo::empty().with_nonce(1).with_code(code));
        }
        db.insert_account_storage(&target, &U256::ZERO, &U256::from(9));
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            Default::default(),
            evm2::ethereum::ethereum_tx_registry(SpecId::OSAKA),
            db,
            evm2::Precompiles::base(SpecId::OSAKA),
        );
        let (to, input) = match case {
            1 => (TxKind::Create, bytes!("60016000555f5fff")),
            2 => (TxKind::Create, bytes!("600160005560015ff3")),
            _ => (TxKind::Call(target), Bytes::new()),
        };
        let tx = Recovered::new_unchecked(
            evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
                to,
                input,
                gas_limit: 1_000_000,
                value: if case == 4 { U256::ONE } else { U256::ZERO },
                ..Default::default()
            }),
            user,
        );
        let raw = evm.transact(&tx).unwrap().detach();
        assert!(raw.result.is_success(), "case {case}: {:?}", raw.result);
        let copy = Carrier::assemble(
            evm.execution_config().clone(),
            evm.config_spec_id(),
            *evm.block(),
            (),
            &tx,
            raw.result.clone(),
            raw.pending_state.clone(),
        )
        .unwrap()
        .detach(&tx)
        .unwrap();
        assert_eq!(copy.result, raw.result, "case {case}: receipt data");
        assert_eq!(
            copy.pending_state, raw.pending_state,
            "case {case}: complete lifecycle/read metadata"
        );
    }
}

#[test]
fn cleanup_keeps_deleted_accounts_absent_and_handles_mixed_loaded_accounts() {
    let mut db = InMemoryDB::default();
    db.insert_account_info(&SYSTEM_ADDRESS, info(19, 1, false));
    db.insert_account_info(&EEZL2_ADDRESS, info(23, 2, false));
    let mut pending = PendingState::default();
    pending.insert_account(SYSTEM_ADDRESS, Some(info(19, 1, false)), None);
    clear_protocol_balances(&mut db, &mut pending).unwrap();
    assert!(pending.account_info(&SYSTEM_ADDRESS).is_none());
    assert_eq!(
        pending.account_info(&EEZL2_ADDRESS),
        Some(&info(0, 2, false))
    );
    assert_eq!(pending.account_changes().count(), 2);
    assert!(pending.account_changes_for(&Address::ZERO).is_none());
    let mut pending = PendingState::default();
    pending.insert_account(
        SYSTEM_ADDRESS,
        Some(info(19, 1, false)),
        Some(info(7, 1, false)),
    );
    clear_protocol_balances(&mut db, &mut pending).unwrap();
    assert_eq!(
        pending.account_info(&SYSTEM_ADDRESS),
        Some(&info(0, 1, false))
    );
    assert_eq!(
        pending.account_info(&EEZL2_ADDRESS),
        Some(&info(0, 2, false))
    );
    assert_eq!(pending.account_changes().count(), 2);
}
