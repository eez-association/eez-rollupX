//! Two embedded L1 snapshots with a threshold change already canonical.
use super::*;
use alloy_consensus::Header;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_storage_api::{StateProviderBox, StateProviderFactory, errors::provider::ProviderResult};

struct Snapshots {
    parent: B256,
    head: B256,
    states: [MockEthProvider; 2],
}

impl crate::local::provider::StateSnapshotProvider for Snapshots {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        StateProviderFactory::latest(&self.states[1])
    }
    fn state_by_block_hash(&self, hash: B256) -> ProviderResult<StateProviderBox> {
        assert!(hash == self.parent || hash == self.head);
        StateProviderFactory::latest(&self.states[usize::from(hash == self.head)])
    }
}

fn registry_state(
    registry: Address,
    threshold: u64,
    quorum: &AttestationQuorum,
) -> MockEthProvider {
    let state = MockEthProvider::new();
    // rollups(id) returns (manager, zero root, zero balance).
    let mut code = vec![0x73]; // PUSH20 manager
    code.extend_from_slice(manager().as_slice());
    code.extend_from_slice(&[0x60, 0, 0x52, 0x60, 96, 0x60, 0, 0xf3]);
    state.add_account(
        registry,
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(code.into()),
    );
    // Return storage[calldataload(4)]: slot zero for threshold(), the verifier
    // address for verificationKey(address). Both use the production ABI reads.
    let mut policy = ExtendedAccount::new(0, U256::ZERO)
        .with_bytecode(alloy_primitives::bytes!("6004355460005260206000f3"))
        .extend_storage([(B256::ZERO, U256::from(threshold))]);
    let verifier = alloy_primitives::hex::decode(
        include_str!("../../composer/estimate/testdata/mainnet-protocol-ecdsa-proof-system.hex")
            .trim(),
    )
    .unwrap();
    for member in &quorum.members {
        policy =
            policy.extend_storage([(member.proof_system.into_word(), member.prover.vkey().into())]);
        state.add_account(
            member.proof_system,
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(verifier.clone().into())
                .extend_storage([(B256::with_last_byte(1), member.attester.into_word().into())]),
        );
    }
    state.add_account(manager(), policy);
    state
}

#[tokio::test]
async fn sizing_uses_selected_parent_even_when_latest_threshold_has_changed() {
    let (mut quorum, _) = quorum(&[(0, SIGN), (0, SIGN), (0, SIGN)]);
    quorum.members[0].proof_system = Address::with_last_byte(0x70);
    let registry = Address::repeat_byte(0x66);
    let parent = Header {
        number: 1,
        gas_limit: 30_000_000,
        ..Default::default()
    };
    let head = Header {
        number: 2,
        parent_hash: parent.hash_slow(),
        ..parent.clone()
    };
    let headers: MockEthProvider = MockEthProvider::new().with_genesis_block();
    headers.add_header(parent.hash_slow(), parent.clone());
    headers.add_header(head.hash_slow(), head.clone());
    for (at_parent, at_head) in [(2, 3), (3, 2)] {
        let chain = crate::local::provider::ChainProvider {
            headers: Arc::new(headers.clone()),
            provider: Arc::new(Snapshots {
                parent: parent.hash_slow(),
                head: head.hash_slow(),
                states: [
                    registry_state(registry, at_parent, &quorum),
                    registry_state(registry, at_head, &quorum),
                ],
            }),
            evm_config: eez_evm::EezEvmConfig::for_l1_simulation(Arc::new(
                reth_chainspec::ChainSpecBuilder::mainnet()
                    .shanghai_activated()
                    .build(),
            )),
            gnosis_evm_config: None,
        };
        let latest = quorum.policy_at(&chain, registry, 7, None).await.unwrap();
        let selected = quorum
            .registration_at_parent(
                &chain,
                registry,
                7,
                &parent,
                Some((head.hash_slow(), latest.clone())),
                budget().expires_at(),
            )
            .await
            .unwrap();
        assert_eq!(latest.threshold as u64, at_head);
        assert_eq!(selected.threshold as u64, at_parent);
        let newer = quorum
            .registration_at_parent(
                &chain,
                registry,
                7,
                &head,
                Some((parent.hash_slow(), selected.clone())),
                budget().expires_at(),
            )
            .await
            .unwrap();
        assert_eq!(newer, latest);
        let cached = quorum
            .registration_at_parent(
                &chain,
                registry,
                7,
                &parent,
                Some((parent.hash_slow(), selected.clone())),
                tokio::time::Instant::now(),
            )
            .await
            .unwrap();
        assert_eq!(cached, selected);
        assert!(
            quorum
                .registration_at_parent(
                    &chain,
                    registry,
                    7,
                    &parent,
                    None,
                    tokio::time::Instant::now()
                )
                .await
                .is_err()
        );
        let systems = quorum.sizing_systems(&selected);
        assert_eq!(systems.len(), selected.threshold);
        if at_parent == 2 {
            assert_eq!(systems, vec![proof_system(2), proof_system(3)]);
        }
    }
}
