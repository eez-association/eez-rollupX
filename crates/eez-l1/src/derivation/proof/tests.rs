use super::*;
use alloy_consensus::{SignableTransaction, TxEnvelope, TxLegacy, transaction::Recovered};
use alloy_primitives::{Bytes, LogData, TxKind, U256};
use alloy_provider::ProviderBuilder;
use alloy_rpc_types_eth::{Block, Log, Transaction};
use alloy_sol_types::SolEvent;
use alloy_transport::mock::Asserter;
use eez_protocol::abi::{
    BatchPosted, EtherDeltaSol, ExecutionEntrySol, L2ExecutionPerformed,
    ProofSystemBatchPerVerificationEntriesSol, RollupIdWithProofSystemsSol, RootUpdateSol,
    postAndVerifyBatchCall,
};

const REGISTRY: Address = Address::repeat_byte(0x11);

#[tokio::test]
async fn genesis_context_needs_no_registration_history_scan() {
    let (schedule, reference) = super::super::tests::fixture();
    let replies = Asserter::new();
    super::super::tests::startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let head = super::super::tests::block(14, 1016, 11);
    let parent = Header {
        timestamp: 1000,
        mix_hash: reference.header.mix_hash,
        ..Default::default()
    };
    replies.push_success(&head);
    registry(&replies, parent.hash_slow());
    replies.push_success(&head);
    let checked = source
        .proof_context(
            REGISTRY,
            1,
            target(&head),
            ParentProvenance {
                parent: parent.clone(),
                accepted_l1_number: 0,
                live_slot_endpoints: vec![],
            },
        )
        .await
        .expect("configured genesis and pinned registry state need no lifetime log scan");
    assert_eq!(checked.parent(), &parent);
    assert_eq!(checked.registry_root(), parent.hash_slow());
    assert_eq!(checked.descendant_seed(), reference.header.mix_hash);
    assert!(replies.read_q().is_empty());
}

fn log(data: LogData, containing: &Block, index: u64, tx_hash: B256) -> Log {
    Log {
        inner: alloy_primitives::Log {
            address: REGISTRY,
            data,
        },
        block_hash: Some(containing.header.hash),
        block_number: Some(containing.header.number),
        transaction_hash: Some(tx_hash),
        transaction_index: Some(index),
        log_index: Some(index * 2),
        ..Default::default()
    }
}

fn parent() -> Header {
    Header {
        number: 4,
        timestamp: 1004,
        mix_hash: B256::with_last_byte(7),
        gas_limit: eez_protocol::l2::GAS_LIMIT,
        ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
        withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
        parent_beacon_block_root: Some(B256::ZERO),
        requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        base_fee_per_gas: Some(1_000_000_000),
        ..Default::default()
    }
}

/// Serve actual ABI batch transactions and settlement logs through the shared
/// attribution scanner. A hash claim in this test never bypasses that boundary.
fn endpoints(replies: &Asserter, containing: &Block, roots: &[B256]) {
    let mut batches = Vec::new();
    let mut outcomes = Vec::new();
    let mut transactions = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        let index = index as u64;
        let batch = ProofSystemBatchPerVerificationEntriesSol {
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 1,
                proofSystemIndexes: vec![],
            }],
            entries: vec![ExecutionEntrySol {
                rootUpdates: vec![RootUpdateSol {
                    rollupId: 1,
                    currentRoot: B256::with_last_byte(0xaa),
                    newRoot: *root,
                    etherDelta: EtherDeltaSol::ZERO,
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let signed = TxLegacy {
            chain_id: Some(31337),
            nonce: index,
            gas_limit: 1_000_000,
            gas_price: 1,
            to: TxKind::Call(REGISTRY),
            input: postAndVerifyBatchCall { batch }.abi_encode().into(),
            ..Default::default()
        }
        .into_signed(alloy_primitives::Signature::test_signature());
        let hash = *signed.hash();
        transactions.push(Transaction {
            inner: Recovered::new_unchecked(TxEnvelope::Legacy(signed), Address::ZERO),
            block_hash: Some(containing.header.hash),
            block_number: Some(containing.header.number),
            transaction_index: Some(index),
            block_timestamp: Some(containing.header.timestamp),
            effective_gas_price: Some(1),
        });
        batches.push(log(
            BatchPosted {
                sharedPublicInput: B256::ZERO,
                rollupIds: vec![1],
            }
            .encode_log_data(),
            containing,
            index,
            hash,
        ));
        let mut outcome = log(
            L2ExecutionPerformed {
                rollupId: 1,
                newRoot: *root,
                etherBalance: U256::ZERO,
            }
            .encode_log_data(),
            containing,
            index,
            hash,
        );
        outcome.log_index = Some(index * 2 + 1);
        outcomes.push(outcome);
    }
    replies.push_success(&batches);
    replies.push_success(&outcomes);
    for transaction in transactions {
        replies.push_success(&transaction);
    }
}

fn registry(replies: &Asserter, root: B256) {
    use alloy_sol_types::SolValue;
    replies.push_success(&Bytes::from(
        (Address::repeat_byte(0x22), root, U256::ZERO).abi_encode(),
    ));
}

fn target(block: &Block) -> L1PostingContext {
    L1PostingContext {
        chain_id: 31337,
        parent_number: block.header.number,
        parent_hash: block.header.hash,
        target_timestamp: block.header.timestamp + 4,
    }
}

#[tokio::test]
async fn genesis_proof_requires_consistent_canonical_target() {
    let (schedule, reference) = super::super::tests::fixture();
    let replies = Asserter::new();
    super::super::tests::startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let head = super::super::tests::block(14, 1016, 11);
    let parent = Header {
        timestamp: 1000,
        mix_hash: reference.header.mix_hash,
        ..Default::default()
    };
    for stale_after_read in [false, true] {
        replies.push_success(&head);
        registry(&replies, parent.hash_slow());
        replies.push_success(&if stale_after_read {
            super::super::tests::block(14, 1016, 12)
        } else {
            head.clone()
        });
        let result = source
            .proof_context(
                REGISTRY,
                1,
                target(&head),
                ParentProvenance {
                    parent: parent.clone(),
                    accepted_l1_number: 0,
                    live_slot_endpoints: vec![],
                },
            )
            .await;
        if stale_after_read {
            assert!(matches!(result, Err(ProofContextError::StaleTarget)));
        } else {
            let checked = result.unwrap();
            assert_eq!(checked.descendant_seed(), reference.header.mix_hash);
            assert_eq!(checked.registry_root(), parent.hash_slow());
            assert_eq!(checked.parent(), &parent);
            // The authenticated target is slot 1020; genesis is at 1000 with
            // K=4 and one-second L2 positions. Failed/static actions count too.
            assert!(checked.check_candidate(20, true).is_ok());
            assert!(checked.check_candidate(16, false).is_ok());
            assert_eq!(
                checked.check_candidate(16, true).unwrap_err().0,
                "catch-up candidates cannot carry cross-chain actions"
            );
            for blocks in [0, 1, 19, 21] {
                assert_eq!(
                    checked.check_candidate(blocks, false).unwrap_err().0,
                    "candidate must cover complete Sync-to-Sync intervals"
                );
            }
            assert_eq!(
                checked.check_candidate(24, false).unwrap_err().0,
                "anchor position or containing settlement slot"
            );
        }
    }
}

#[tokio::test]
async fn catchup_parent_keeps_prior_live_seed_and_requires_complete_ordered_endpoints() {
    let (schedule, reference) = super::super::tests::fixture();
    let replies = Asserter::new();
    super::super::tests::startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let head = super::super::tests::block(14, 1016, 11);
    let catchup = super::super::tests::block(12, 1008, 9);
    let live = super::super::tests::block(11, 1004, 8);
    let parent = parent();
    let mut earlier = parent.clone();
    earlier.state_root = B256::with_last_byte(0x34);
    for case in 0..6 {
        replies.push_success(&head);
        replies.push_success(&catchup);
        endpoints(&replies, &catchup, &[parent.hash_slow()]);
        replies.push_success(&catchup); // binary search middle
        replies.push_success(&reference);
        replies.push_success(&live);
        endpoints(&replies, &live, &[earlier.hash_slow(), parent.hash_slow()]);
        let mut supplied = vec![earlier.clone(), parent.clone()];
        match case {
            1 => {
                supplied.pop();
            }
            2 => supplied.push(parent.clone()),
            3 => supplied.reverse(),
            4 => supplied[0].state_root = B256::ZERO,
            5 => supplied[0].gas_limit += 1,
            _ => {
                registry(&replies, parent.hash_slow());
                replies.push_success(&head);
            }
        }
        let result = source
            .proof_context(
                REGISTRY,
                1,
                target(&head),
                ParentProvenance {
                    parent: parent.clone(),
                    accepted_l1_number: 12,
                    live_slot_endpoints: supplied,
                },
            )
            .await;
        match case {
            0 => assert_eq!(result.unwrap().descendant_seed(), live.header.mix_hash),
            1 | 2 => assert!(matches!(
                result,
                Err(ProofContextError::Invalid(
                    "incomplete live-slot endpoint set"
                ))
            )),
            3 | 4 => assert!(matches!(
                result,
                Err(ProofContextError::Invalid("live-slot endpoint hash"))
            )),
            5 => assert!(matches!(
                result,
                Err(ProofContextError::Source(DerivationError::Profile(_)))
            )),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn missed_l1_slot_cannot_refresh_seed_or_accept_fabricated_endpoints() {
    let (schedule, reference) = super::super::tests::fixture();
    let replies = Asserter::new();
    super::super::tests::startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let head = super::super::tests::block(12, 1012, 10);
    let catchup = super::super::tests::block(11, 1008, 9); // no block at timestamp 1004
    let parent = parent();
    for forged in [false, true] {
        replies.push_success(&head);
        replies.push_success(&catchup);
        endpoints(&replies, &catchup, &[parent.hash_slow()]);
        replies.push_success(&catchup);
        replies.push_success(&reference);
        if !forged {
            registry(&replies, parent.hash_slow());
            replies.push_success(&head);
        }
        let result = source
            .proof_context(
                REGISTRY,
                1,
                target(&head),
                ParentProvenance {
                    parent: parent.clone(),
                    accepted_l1_number: 11,
                    live_slot_endpoints: if forged { vec![parent.clone()] } else { vec![] },
                },
            )
            .await;
        if forged {
            assert!(matches!(
                result,
                Err(ProofContextError::Invalid("unexpected live-slot endpoints"))
            ));
        } else {
            assert_eq!(result.unwrap().descendant_seed(), parent.mix_hash);
        }
    }
}

#[tokio::test]
async fn false_parent_wrong_genesis_and_source_failures_remain_rejected() {
    let (schedule, reference) = super::super::tests::fixture();
    for case in 0..4 {
        let replies = Asserter::new();
        super::super::tests::startup_replies(&replies, &reference);
        let source = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies.clone()),
            schedule,
        )
        .await
        .unwrap();
        let head = super::super::tests::block(12, 1008, 9);
        replies.push_success(&head);
        let mut supplied = ParentProvenance {
            parent: parent(),
            accepted_l1_number: 12,
            live_slot_endpoints: vec![],
        };
        match case {
            // A root hash alone (including one set by the owner) never supplies
            // the settlement occurrence needed for a non-genesis parent.
            0 => {
                replies.push_success(&head);
                endpoints(&replies, &head, &[]);
            }
            1 => {
                supplied.parent = Header {
                    timestamp: 1000,
                    mix_hash: reference.header.mix_hash,
                    state_root: B256::repeat_byte(0x55),
                    ..Default::default()
                };
                supplied.accepted_l1_number = 0;
            }
            2 => replies.push_failure_msg("private endpoint must not leak"),
            3 => {
                supplied.parent = Header {
                    timestamp: 1000,
                    mix_hash: reference.header.mix_hash,
                    ..Default::default()
                };
                supplied.accepted_l1_number = 0;
                replies.push_success(&Bytes::from(vec![0; 96]));
            }
            _ => unreachable!(),
        }
        let result = source
            .proof_context(REGISTRY, 1, target(&head), supplied)
            .await;
        let expected = match case {
            0 => "parent is not an accepted endpoint",
            1 => {
                assert!(matches!(
                    result,
                    Err(ProofContextError::Source(DerivationError::Profile(
                        eez_protocol::l2::ProfileError(
                            "header does not occupy its genesis-derived position"
                        )
                    )))
                ));
                continue;
            }
            2 => {
                assert!(matches!(
                    result,
                    Err(ProofContextError::Source(DerivationError::Unavailable(
                        "canonical header RPC"
                    )))
                ));
                continue;
            }
            3 => "rollup is not registered",
            _ => unreachable!(),
        };
        assert!(
            matches!(result, Err(ProofContextError::Invalid(message)) if message == expected),
            "case {case}"
        );
    }
}
