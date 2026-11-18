//! Dynamic child read: SSTORE(1, SLOAD(0)) binds storage visibility into the block root.
use super::history_observations::{CaptureScenario, Fixture};
use alloy_genesis::GenesisAccount;
use alloy_primitives::{B256, Bytes, TxKind, U256};

pub(super) fn fixture(balance: u64) -> Fixture {
    fixture_with_child(balance, Bytes::new(), false)
}

pub(super) fn alternative_fixture(balance: u64) -> Fixture {
    fixture_with_child(balance, Bytes::from_static(&[1]), false)
}

pub(super) fn extended_fixture(balance: u64) -> Fixture {
    fixture_with_child(balance, Bytes::new(), true)
}

fn fixture_with_child(balance: u64, input: Bytes, extra_empty_child: bool) -> Fixture {
    let writes_child_storage = input.is_empty();
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    // Nonempty calldata follows the existing later-transaction SELFDESTRUCT case.
    // Empty calldata reads slot zero, writes that value to slot one, and returns it.
    let runtime = hex::decode("36156009575f5fff005b5f54806001555f5260205ff3").unwrap();
    let mut init = vec![
        0x60,
        runtime.len() as u8,
        0x60,
        12,
        0x60,
        0,
        0x39,
        0x60,
        runtime.len() as u8,
        0x60,
        0,
        0xf3,
    ];
    init.extend(runtime);
    Fixture::with_capture_scenario(
        true,
        None,
        CaptureScenario {
            accounts: vec![(
                created,
                GenesisAccount {
                    balance: U256::from(balance),
                    storage: Some(
                        [(B256::ZERO, B256::with_last_byte(99))]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
            )],
            transactions: vec![(TxKind::Create, init.into())],
            suffix_call: Some((created, Bytes::from_static(&[1]))),
            retain_prior_storage_at: Some(created),
            followup_call: Some((created, input)),
            extra_empty_child,
            expected_child_storage: writes_child_storage.then_some((
                created,
                U256::ONE,
                U256::from(99),
            )),
            ..Default::default()
        },
    )
}

#[test]
fn create_child_sload_block_vectors() {
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    for balance in [0u64, 1] {
        let fixture = fixture(balance);
        assert_eq!(fixture.blocks.len(), 7);
        assert_eq!(
            fixture.capture_states[6].alloc[&created]
                .storage
                .as_ref()
                .unwrap()[&B256::ZERO],
            B256::with_last_byte(99)
        );
        let result = serde_json::json!({
            "balance": balance,
            "blocks": fixture.blocks.iter().map(|b| hex::encode(&b.rlp)).collect::<Vec<_>>(),
            "hashes": fixture.blocks.iter().map(|b| hex::encode(&b.hash)).collect::<Vec<_>>(),
        });
        let validated = crate::validate::Validator::stateless_for_test(
            fixture.genesis.clone(),
            eez_primitives::SYSTEM_ADDRESS,
        )
        .validate(&fixture.admitted())
        .unwrap();
        assert_eq!(
            validated.window_post_block_hash(),
            B256::from_slice(&fixture.blocks.last().unwrap().hash)
        );
        let expected: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("child-sload-reth27-vectors.json")).unwrap();
        assert_eq!(
            &result,
            expected
                .iter()
                .find(|row| row["balance"] == balance)
                .unwrap()
        );
    }
}
