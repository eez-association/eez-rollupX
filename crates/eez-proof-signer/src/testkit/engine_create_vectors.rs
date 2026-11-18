//! Identical fixture construction compiled separately with baseline and candidate engines.
use super::history_observations::{CaptureScenario, Fixture};
use alloy_genesis::GenesisAccount;
use alloy_primitives::{B256, Bytes, TxKind, U256};

#[test]
fn create_prior_storage_block_vectors() {
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    let runtime = hex::decode("36156009575f5fff005b602a5f5260205ff3").unwrap();
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
    for balance in [0u64, 1] {
        let fixture = Fixture::with_capture_scenario(
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
                transactions: vec![(TxKind::Create, init.clone().into())],
                suffix_call: Some((created, Bytes::from_static(&[1]))),
                retain_prior_storage_at: Some(created),
                followup_call: Some((created, Bytes::new())),
                ..Default::default()
            },
        );
        assert_eq!(fixture.blocks.len(), 7);
        let result = serde_json::json!({
            "balance": balance,
            "blocks": fixture.blocks.iter().map(|b| hex::encode(&b.rlp)).collect::<Vec<_>>(),
            "hashes": fixture.blocks.iter().map(|b| hex::encode(&b.hash)).collect::<Vec<_>>(),
        });
        let expected: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("engine-create-reth27-vectors.json")).unwrap();
        assert_eq!(
            &result,
            expected.iter().find(|v| v["balance"] == balance).unwrap()
        );
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
        println!("CREATE_REFERENCE {result}");
    }
}
