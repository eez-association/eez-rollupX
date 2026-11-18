//! Real production capture -> structural admission -> Stateless -> observation replay.
use super::*;
use crate::testkit::history_observations::{Fixture, PROBE};
use alloy_primitives::TxKind;

fn pin(fixture: &Fixture) -> B256 {
    Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS)
        .validate(&fixture.admitted())
        .unwrap()
        .settling_block()
        .block()
        .settlement_evidence()
        .ordinary_prefix_hash()
}

fn admit(blocks: Vec<eez_control_rpc::v2::BlockWitness>) -> Vec<AdmittedBlock> {
    blocks
        .into_iter()
        .map(|block| {
            AdmittedBlock::from_wire(
                block,
                &mut 0,
                crate::window::WindowLimits {
                    blocks: 16,
                    payload_bytes: 16 * 1024 * 1024,
                    witness_items: 100_000,
                },
            )
            .unwrap()
        })
        .collect()
}

fn captured(fixture: &Fixture, claims: &[FifoObservationClaim]) -> Vec<AdmittedBlock> {
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    captured_at(fixture, claims, block.body.transactions.len())
}

fn captured_at(
    fixture: &Fixture,
    claims: &[FifoObservationClaim],
    boundary: usize,
) -> Vec<AdmittedBlock> {
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    let observations: Vec<_> = claims
        .iter()
        .cloned()
        .map(|claim| BlockObservation {
            after_transactions: boundary,
            manager_address: crate::EEZL2_ADDRESS,
            claim,
        })
        .collect();
    let (blocks, observed) = fixture.captured(&observations);
    assert_eq!(observed.len(), claims.len());
    for (actual, expected) in observed.iter().zip(claims) {
        assert_eq!(actual.success, expected.expected_success);
        assert_eq!(actual.output, expected.expected_output);
    }
    let admitted = admit(blocks);
    let history = crate::testkit::checked_history(&admitted).unwrap();
    let validator = Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&admitted).unwrap();
    assert_eq!(window.window_post_block_hash(), block.header.hash_slow());
    let Validator::Stateless(backend) = validator else {
        unreachable!()
    };
    backend
        .verify_observations(&window, claims, history)
        .unwrap();
    admitted
}

#[test]
fn real_capture_preserves_observation_reads_and_nested_rollback() {
    let fixture = Fixture::storage_probe();
    let pin = pin(&fixture);
    let word = |value: u64| Bytes::copy_from_slice(&U256::from(value).to_be_bytes::<32>());
    let write = |value| {
        let mut claim = history_observation_claim(pin, CallMode::Static, word(value));
        claim.call_mode = CallMode::Mutable;
        claim.data = word(value);
        with_mutable_delivery(claim)
    };
    let mut outer = write(1);
    outer.rollback_spans = vec![4];
    let mut inner = write(2);
    inner.rollback_spans = vec![2];
    let claims = vec![
        outer,
        inner,
        history_observation_claim(pin, CallMode::Static, word(2)),
        history_observation_claim(pin, CallMode::Static, word(1)),
        history_observation_claim(pin, CallMode::Static, word(0)),
    ];
    let admitted = captured(&fixture, &claims);
    // PROBE is never called by the canonical block. Omitting its code therefore
    // reaches authenticated observation replay, after ordinary validation passes.
    let mut missing = admitted;
    let code = fixture.genesis.alloc[&PROBE].code.as_ref().unwrap();
    let terminal = missing.last_mut().unwrap();
    let before = terminal.witness.codes.len();
    terminal.witness.codes.retain(|item| item != code);
    assert_eq!(terminal.witness.codes.len() + 1, before);
    let history = crate::testkit::checked_history(&missing).unwrap();
    let validator = Validator::stateless_for_test(fixture.genesis, TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&missing).unwrap();
    let Validator::Stateless(backend) = validator else {
        unreachable!()
    };
    let error = backend
        .verify_observations(&window, &claims, history)
        .unwrap_err();
    assert!(
        matches!(error, ObservationValidationError::Replay(ref text) if text.contains("bytecode")),
        "{error:?}"
    );
}

fn nested_effect(effect: &[u8]) -> Vec<u8> {
    // Empty input calls this same contract with one byte; the inner forbidden
    // operation must fail, and the outer frame propagates that failure.
    let mut code = vec![0x36, 0x15, 0x60, 0, 0x57];
    code.extend_from_slice(effect);
    code.push(0x00);
    code[3] = u8::try_from(code.len()).unwrap();
    code.push(0x5b);
    code.extend_from_slice(&[
        0x60, 1, 0x5f, 0x52, 0x5f, 0x5f, 0x60, 1, 0x60, 31, 0x5f, 0x73,
    ]);
    code.extend_from_slice(PROBE.as_slice());
    code.extend_from_slice(&[0x5a, 0xf1, 0x15, 0x60, 0, 0x57, 0x00]);
    let target = u8::try_from(code.len()).unwrap();
    let jump = code.len() - 3;
    code[jump] = target;
    code.extend_from_slice(&[0x5b, 0x5f, 0x5f, 0xfd]);
    code
}

#[test]
fn real_capture_and_signer_enforce_static_effects_directly_and_through_call() {
    for effect in ["60015f55", "5f5fa0", "5f5f5ff0"] {
        let effect = hex::decode(effect).unwrap();
        for nested in [false, true] {
            let code = if nested {
                nested_effect(&effect)
            } else {
                effect.clone()
            };
            let fixture = Fixture::with_probe_code(false, Some(code));
            let mut claim =
                history_observation_claim(pin(&fixture), CallMode::Static, Bytes::new());
            claim.expected_success = false;
            let admitted = captured(&fixture, std::slice::from_ref(&claim));
            let history = crate::testkit::checked_history(&admitted).unwrap();
            let validator = Validator::stateless_for_test(fixture.genesis, TEST_SYSTEM_ADDRESS);
            let window = validator.validate(&admitted).unwrap();
            let Validator::Stateless(backend) = validator else {
                unreachable!()
            };
            claim.expected_success = true;
            assert_eq!(
                backend.verify_observations(&window, &[claim], history),
                Err(ObservationValidationError::OutcomeMismatch { index: 0 }),
                "effect {effect:?}, nested {nested}"
            );
        }
    }
}

#[test]
fn real_capture_authenticates_absent_account_slot_and_observation_only_blockhash() {
    // The canonical transactions never touch this account or PROBE's slot 99.
    let absent = Address::repeat_byte(0xab);
    let mut code = vec![0x73];
    code.extend_from_slice(absent.as_slice());
    code.extend_from_slice(&hex::decode("315f5260635460205260405ff3").unwrap());
    let fixture = Fixture::with_probe_code(false, Some(code));
    assert!(!fixture.genesis.alloc.contains_key(&absent));
    captured(
        &fixture,
        &[history_observation_claim(
            pin(&fixture),
            CallMode::Static,
            vec![0; 64].into(),
        )],
    );

    let fixture = Fixture::with_probe_code(false, Some(hex::decode("6001405f5260205ff3").unwrap()));
    let expected = Bytes::copy_from_slice(&fixture.blocks[0].hash);
    let admitted = captured(
        &fixture,
        &[history_observation_claim(
            pin(&fixture),
            CallMode::Static,
            expected,
        )],
    );
    assert!(
        admitted.last().unwrap().witness.headers.len() > 1,
        "BLOCKHASH(1) at block six must capture more than the immediate parent"
    );
}

fn initcode(runtime: &[u8]) -> Bytes {
    let size = u8::try_from(runtime.len()).unwrap();
    let mut init = vec![
        0x60, size, 0x60, 12, 0x60, 0, 0x39, 0x60, size, 0x60, 0, 0xf3,
    ];
    init.extend_from_slice(runtime);
    init.into()
}

#[test]
fn real_capture_observes_earlier_create_before_later_canonical_selfdestruct() {
    use crate::testkit::history_observations::CaptureScenario;
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    // Empty input returns 42; nonempty input executes SELFDESTRUCT. Osaka retains
    // code when creation was in an earlier transaction, even in the same block.
    let runtime = hex::decode("36156009575f5fff005b602a5f5260205ff3").unwrap();
    let fixture = Fixture::with_capture_scenario(
        true,
        None,
        CaptureScenario {
            transactions: vec![(TxKind::Create, initcode(&runtime))],
            suffix_call: Some((created, Bytes::from_static(&[1]))),
            ..Default::default()
        },
    );
    let mut claim = history_observation_claim(
        pin(&fixture),
        CallMode::Static,
        U256::from(42).to_be_bytes_vec().into(),
    );
    claim.target_address = created;
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    captured_at(&fixture, &[claim], block.body.transactions.len() - 2);
}

#[test]
fn frozen_engine_observation_vectors() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_sol_types::SolCall as _;
    use eez_protocol::settlement::FifoObservationCallback;
    use eez_protocol::{EEZL2_AUTHORIZED_PROXIES_SLOT, proxy_mapping_key};
    let mut vectors = Vec::new();
    for name in [
        "gas",
        "callback-success-manager-recall",
        "callback-revert-manager-recall",
    ] {
        let callback = Address::with_last_byte(0xc1);
        let source_target = Address::with_last_byte(0xc2);
        let mut scenario = CaptureScenario::default();
        let code = if name == "gas" {
            hex::decode("5a5f5260205ff3").unwrap()
        } else {
            let mut manager =
                crate::testkit::history::genesis().alloc[&crate::EEZL2_ADDRESS].clone();
            let mut registration = [0u8; 32];
            registration[11..31].copy_from_slice(source_target.as_slice());
            registration[31] = 1;
            manager.storage.get_or_insert_default().insert(
                proxy_mapping_key(callback, EEZL2_AUTHORIZED_PROXIES_SLOT),
                B256::from(registration),
            );
            scenario.accounts.push((crate::EEZL2_ADDRESS, manager));
            let mut code = hex::decode("60205f5f5f73").unwrap();
            code.extend_from_slice(callback.as_slice());
            code.extend_from_slice(&hex::decode("5afa6020523660006080376020604036608073").unwrap());
            code.extend_from_slice(crate::EEZL2_ADDRESS.as_slice());
            code.extend_from_slice(&hex::decode("5afa505a60605260805ff3").unwrap());
            // Record GAS after returning from a real manager call.
            code
        };
        let fixture = Fixture::with_capture_scenario(false, Some(code), scenario);
        let mut claim = history_observation_claim(pin(&fixture), CallMode::Static, Bytes::new());
        if name != "gas" {
            claim.data = eez_protocol::abi::computeCrossChainProxyAddressCall {
                originalAddress: claim.source_address,
                originalRollupId: claim.source_rollup_id.0,
            }
            .abi_encode()
            .into();
            claim.callbacks.push(FifoObservationCallback {
                source_address: PROBE,
                target_address: source_target,
                data: Bytes::new(),
                success: name.contains("success"),
                output: U256::from(7).to_be_bytes_vec().into(),
            });
        }
        let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
        let (_, actual) = fixture.captured(&[BlockObservation {
            after_transactions: block.body.transactions.len(),
            manager_address: crate::EEZL2_ADDRESS,
            claim: claim.clone(),
        }]);
        claim.expected_success = actual[0].success;
        claim.expected_output = actual[0].output.clone();
        assert!(claim.expected_success, "{name}");
        captured(&fixture, &[claim.clone()]);
        vectors.push(
            serde_json::json!({"case":name,"success":claim.expected_success,
            "output":hex::encode(&claim.expected_output),
            "blocks":fixture.blocks.iter().map(|b| hex::encode(&b.hash)).collect::<Vec<_>>() }),
        );
    }
    let vector = serde_json::to_value(vectors).unwrap();
    // Generated once by the frozen Reth 2.7 runtime; never derive expected
    // outcomes from the candidate engine or a regenerated shipping fixture.
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("capture-reth27-vectors.json")).unwrap();
    assert_eq!(
        vector, expected,
        "frozen Reth 2.7 observations and block commitments"
    );
    println!("WITNESS_ENGINE_VECTORS={vector}");
}

#[test]
fn real_capture_create_clears_prior_storage_and_nested_rollback_restores_creation() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    let child = PROBE.create(1);
    let runtime = hex::decode("36156009575f355f555b5f545f5260205ff3").unwrap();
    let init = initcode(&runtime);
    let size = u8::try_from(init.len()).unwrap();
    let mut factory = vec![
        0x60, size, 0x60, 13, 0x5f, 0x39, 0x60, size, 0x5f, 0x5f, 0xf0, 0x50, 0x00,
    ];
    factory.extend_from_slice(&init);
    let fixture = Fixture::with_capture_scenario(
        false,
        Some(factory),
        CaptureScenario {
            accounts: vec![(
                child,
                GenesisAccount {
                    storage: Some(
                        [(B256::ZERO, B256::with_last_byte(99))]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
            )],
            ..Default::default()
        },
    );
    let root = pin(&fixture);
    let mut create = history_observation_claim(root, CallMode::Mutable, Bytes::new());
    create.rollback_spans = vec![4];
    let mut read = history_observation_claim(root, CallMode::Static, vec![0; 32].into());
    read.target_address = child;
    let mut write = read.clone();
    write.call_mode = CallMode::Mutable;
    write.data = U256::from(7).to_be_bytes_vec().into();
    write.expected_output = write.data.clone();
    write.rollback_spans = vec![1];
    write = with_mutable_delivery(write);
    let mut after = read.clone();
    after.expected_output = Bytes::new();
    captured(
        &fixture,
        &[
            create.clone(),
            read.clone(),
            write,
            read.clone(),
            after,
            {
                create.rollback_spans = vec![2];
                create
            },
            read,
        ],
    );
}

fn decoded_scopes(root: B256, depth: usize, distinct: bool) -> Vec<FifoObservationClaim> {
    use eez_blob_codec::Message;
    use eez_protocol::settlement::{FifoTransactionRootPlan, fifo_observation_claims};
    let word = |value: usize| Bytes::from(U256::from(value).to_be_bytes_vec());
    let write = |value| {
        vec![
            Message::Call {
                to_chain: 1,
                from_address: Address::with_last_byte(0x50),
                to_address: PROBE,
                value: U256::ZERO,
                gas: 0,
                data: word(value),
            },
            Message::ReturnSuccess {
                return_data: word(value),
            },
        ]
    };
    let read = |value| {
        vec![
            Message::StaticCall {
                to_chain: 1,
                from_address: Address::with_last_byte(0x50),
                to_address: PROBE,
                gas: 0,
                data: Bytes::new(),
            },
            Message::ReturnSuccess {
                return_data: word(value),
            },
        ]
    };
    let mut messages = vec![Message::InitiateCrossChainTransaction {
        chain_id: 0,
        tx_data: Bytes::new(),
    }];
    for value in 1..=depth {
        messages.push(Message::Snapshot);
        if distinct {
            messages.extend(write(value));
        }
    }
    if !distinct {
        messages.extend(write(1));
    }
    for value in (0..depth).rev() {
        messages.push(Message::Revert);
        if distinct {
            messages.extend(read(value));
        }
    }
    if !distinct {
        messages.extend(read(0));
    }
    messages.push(Message::FinishCrossChainTransaction);
    let blobs =
        eez_blob_codec::encode_native_payload(1, Bytes::from_static(&[0]), messages).unwrap();
    let native = eez_blob_codec::decode_native_payload(&blobs, 1).unwrap();
    let semantic = eez_protocol::blob_semantics::decode_native_semantics(&native).unwrap();
    assert_eq!(semantic.len(), 1);
    let composition = semantic[0].materialize().unwrap();
    fifo_observation_claims(
        &composition,
        &FifoTransactionRootPlan {
            mutable_new_roots: vec![root; if distinct { depth } else { 1 }],
            static_roots: vec![root; if distinct { depth } else { 1 }],
            endpoint: root,
        },
    )
    .unwrap()
}

#[test]
fn real_decoder_accepts_64_same_and_distinct_nested_scopes() {
    let fixture = Fixture::storage_probe();
    let root = pin(&fixture);
    for distinct in [false, true] {
        let claims = decoded_scopes(root, 64, distinct);
        assert_eq!(
            claims.iter().map(|c| c.rollback_spans.len()).sum::<usize>(),
            64
        );
        captured(&fixture, &claims);
    }
}

fn resource_scope_case(depth: usize, distinct: bool) {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    let warm = Address::with_last_byte(0xe1);
    // Read 10,000 absent slots before any snapshot opens. All reads fit under
    // the production observation gas cap; the cache is irrelevant to writes.
    let warmer = hex::decode("6127105b6001900380545080600357505f5ff3").unwrap();
    let fixture = Fixture::with_capture_scenario(
        false,
        Some(hex::decode("36156009575f355f555b5f545f5260205ff3").unwrap()),
        CaptureScenario {
            accounts: vec![(
                warm,
                GenesisAccount {
                    nonce: Some(1),
                    code: Some(warmer.into()),
                    ..Default::default()
                },
            )],
            ..Default::default()
        },
    );
    let root = pin(&fixture);
    let mut warming = history_observation_claim(root, CallMode::Static, Bytes::new());
    warming.target_address = warm;
    let mut claims = vec![warming];
    claims.extend(decoded_scopes(root, depth, distinct));
    captured(&fixture, &claims);
}

#[test]
fn resource_one_scope() {
    resource_scope_case(1, false);
}
#[test]
fn resource_64_same_boundary_scopes() {
    resource_scope_case(64, false);
}
#[test]
fn resource_64_distinct_boundary_scopes() {
    resource_scope_case(64, true);
}

#[test]
fn real_capture_keeps_parent_slot_proof_when_later_canonical_execution_deletes_slot() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    let code = hex::decode("36156009575f355f555b5f545f5260205ff3").unwrap();
    let fixture = Fixture::with_capture_scenario(
        true,
        None,
        CaptureScenario {
            accounts: vec![(
                PROBE,
                GenesisAccount {
                    nonce: Some(1),
                    code: Some(code.into()),
                    storage: Some(
                        [(B256::ZERO, B256::with_last_byte(9))]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
            )],
            suffix_call: Some((PROBE, vec![0; 32].into())),
            ..Default::default()
        },
    );
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    let claim = history_observation_claim(
        pin(&fixture),
        CallMode::Static,
        U256::from(9).to_be_bytes_vec().into(),
    );
    captured_at(&fixture, &[claim], block.body.transactions.len() - 2);
}

fn reject_missing_observation_nodes(
    fixture: &Fixture,
    claims: &[FifoObservationClaim],
    control: Vec<eez_control_rpc::v2::BlockWitness>,
) -> usize {
    let admitted = captured(fixture, claims);
    let control = &control.last().unwrap().witness.as_ref().unwrap().state;
    let extra: Vec<_> = admitted
        .last()
        .unwrap()
        .witness
        .state
        .iter()
        .filter(|node| !control.iter().any(|old| old.as_slice() == node.as_ref()))
        .cloned()
        .collect();
    assert!(
        !extra.is_empty(),
        "absence read must need observation-only trie nodes"
    );
    for node in &extra {
        let mut missing = admitted.clone();
        missing
            .last_mut()
            .unwrap()
            .witness
            .state
            .retain(|item| item != node);
        let history = crate::testkit::checked_history(&missing).unwrap();
        let validator = Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS);
        let window = validator
            .validate(&missing)
            .expect("removed node must not be needed by canonical execution");
        let Validator::Stateless(backend) = validator else {
            unreachable!()
        };
        let error = backend
            .verify_observations(&window, claims, history)
            .unwrap_err();
        assert!(
            matches!(error, ObservationValidationError::Replay(_)),
            "{error:?}"
        );
    }
    extra.len()
}

#[test]
fn real_capture_requires_nonempty_storage_absence_proof_nodes() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    let fixture = Fixture::with_capture_scenario(
        false,
        None,
        CaptureScenario {
            accounts: vec![(
                PROBE,
                GenesisAccount {
                    nonce: Some(1),
                    code: Some(hex::decode("5f35545f5260205ff3").unwrap().into()),
                    storage: Some(
                        (1..=3)
                            .map(|slot| {
                                (B256::with_last_byte(slot), B256::with_last_byte(slot + 10))
                            })
                            .collect(),
                    ),
                    ..Default::default()
                },
            )],
            ..Default::default()
        },
    );
    let mut claim = history_observation_claim(
        pin(&fixture),
        CallMode::Static,
        U256::from(11).to_be_bytes_vec().into(),
    );
    claim.data = U256::from(1).to_be_bytes_vec().into();
    captured(&fixture, &[claim.clone()]);
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    let observation = |claim| BlockObservation {
        after_transactions: block.body.transactions.len(),
        manager_address: crate::EEZL2_ADDRESS,
        claim,
    };
    // A present-slot control supplies the common storage branch. Choose a
    // missing slot on another populated branch, so its leaf proves absence.
    let (control, _) = fixture.captured(&[observation(claim.clone())]);
    claim.expected_output = vec![0; 32].into();
    for slot in 4..=99u64 {
        claim.data = U256::from(slot).to_be_bytes_vec().into();
        let (read, _) = fixture.captured(&[observation(claim.clone())]);
        let old = &control.last().unwrap().witness.as_ref().unwrap().state;
        if read
            .last()
            .unwrap()
            .witness
            .as_ref()
            .unwrap()
            .state
            .iter()
            .any(|node| !old.contains(node))
        {
            let count = reject_missing_observation_nodes(&fixture, &[claim], control);
            println!("NONEMPTY_STORAGE_ABSENCE_REQUIRED_NODES={count};slot={slot}");
            return;
        }
    }
    panic!("no nontrivial absent-slot proof was exercised");
}

#[test]
fn real_capture_requires_observation_only_account_absence_proof_nodes() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    let fixture = Fixture::with_capture_scenario(
        false,
        Some(hex::decode("3615600a575f35315f525b60205ff3").unwrap()),
        CaptureScenario {
            accounts: (0xb0..=0xbf)
                .map(|i| {
                    (
                        Address::with_last_byte(i),
                        GenesisAccount {
                            nonce: Some(1),
                            balance: U256::from(5),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        },
    );
    let mut claim = history_observation_claim(pin(&fixture), CallMode::Static, vec![0; 32].into());
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    let observation = |claim| BlockObservation {
        after_transactions: block.body.transactions.len(),
        manager_address: crate::EEZL2_ADDRESS,
        claim,
    };
    // The control loads the same PROBE account/code, but makes no BALANCE read.
    let (control, _) = fixture.captured(&[observation(claim.clone())]);
    for seed in 1..=64u8 {
        let hash = alloy_primitives::keccak256([seed]);
        let address = Address::from_slice(&hash[12..]);
        assert!(!fixture.genesis.alloc.contains_key(&address));
        claim.data = U256::from_be_slice(address.as_slice())
            .to_be_bytes_vec()
            .into();
        let (read, _) = fixture.captured(&[observation(claim.clone())]);
        let old = &control.last().unwrap().witness.as_ref().unwrap().state;
        if read
            .last()
            .unwrap()
            .witness
            .as_ref()
            .unwrap()
            .state
            .iter()
            .any(|node| !old.contains(node))
        {
            let count = reject_missing_observation_nodes(&fixture, &[claim], control);
            println!("ACCOUNT_ABSENCE_REQUIRED_NODES={count};address={address}");
            return;
        }
    }
    panic!("no nontrivial absent-account proof was exercised");
}

#[test]
fn real_capture_reverts_multiple_deliveries_at_one_scope_end() {
    let fixture = Fixture::storage_probe();
    let root = pin(&fixture);
    let write = |value| {
        let mut claim = history_observation_claim(
            root,
            CallMode::Static,
            U256::from(value).to_be_bytes_vec().into(),
        );
        claim.call_mode = CallMode::Mutable;
        claim.data = claim.expected_output.clone();
        with_mutable_delivery(claim)
    };
    let mut first = write(1u64);
    first.rollback_spans = vec![3];
    captured(
        &fixture,
        &[
            first,
            write(2),
            history_observation_claim(
                root,
                CallMode::Static,
                U256::from(2).to_be_bytes_vec().into(),
            ),
            history_observation_claim(root, CallMode::Static, vec![0; 32].into()),
        ],
    );
}

#[cfg(target_os = "linux")]
fn allocated_bytes(work: impl FnOnce()) -> u64 {
    let counter = tikv_jemalloc_ctl::thread::allocatedp::read().unwrap();
    let start = counter.get();
    work();
    counter.get() - start
}

#[test]
#[cfg(target_os = "linux")]
fn allocation_bound_is_enforced_for_admitted_nested_scopes() {
    let calibration = allocated_bytes(|| {
        std::hint::black_box(vec![0u8; 1 << 20]);
    });
    assert!(
        calibration >= 1 << 20,
        "must measure the active Rust allocator"
    );
    let one = allocated_bytes(|| resource_scope_case(1, false));
    let same = allocated_bytes(|| resource_scope_case(64, false));
    let distinct = allocated_bytes(|| resource_scope_case(64, true));
    println!("NESTED_THREAD_ALLOCATIONS={one},{same},{distinct}");
    // The same-boundary case adds only scope metadata; the distinct case also
    // executes 126 extra deliveries/reads. Both envelopes reject the old clones.
    assert!(
        same <= one + (1 << 20),
        "same-boundary cache amplification: {one}->{same}"
    );
    assert!(
        distinct <= one + (128 << 20),
        "distinct-boundary cache amplification: {one}->{distinct}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn sibling_scope_allocation_evidence() {
    use crate::testkit::history_observations::CaptureScenario;
    use alloy_genesis::GenesisAccount;
    use eez_blob_codec::Message;
    use eez_protocol::settlement::{FifoTransactionRootPlan, fifo_observation_claims};
    // Every target writes 64 fresh slots. Each sibling scope rolls all of them
    // back before moving to a different account at the same canonical prefix.
    let code: Bytes = hex::decode("60405b6001900360018155806002575000")
        .unwrap()
        .into();
    let fixture = Fixture::with_capture_scenario(
        false,
        None,
        CaptureScenario {
            accounts: (0xa0..=0xdf)
                .map(|i| {
                    (
                        Address::with_last_byte(i),
                        GenesisAccount {
                            nonce: Some(1),
                            code: Some(code.clone()),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        },
    );
    let root = pin(&fixture);
    let mut measurements = Vec::new();
    for count in [8u8, 64] {
        let mut messages = vec![Message::InitiateCrossChainTransaction {
            chain_id: 0,
            tx_data: Bytes::new(),
        }];
        for index in 0..count {
            messages.extend([
                Message::Snapshot,
                Message::Call {
                    to_chain: 1,
                    from_address: Address::with_last_byte(0x50),
                    to_address: Address::with_last_byte(0xa0 + index),
                    value: U256::ZERO,
                    gas: 0,
                    data: Bytes::new(),
                },
                Message::ReturnSuccess {
                    return_data: Bytes::new(),
                },
                Message::Revert,
            ]);
        }
        messages.push(Message::FinishCrossChainTransaction);
        let blobs =
            eez_blob_codec::encode_native_payload(1, Bytes::from_static(&[0]), messages).unwrap();
        let native = eez_blob_codec::decode_native_payload(&blobs, 1).unwrap();
        let semantics = eez_protocol::blob_semantics::decode_native_semantics(&native).unwrap();
        let claims = fifo_observation_claims(
            &semantics[0].materialize().unwrap(),
            &FifoTransactionRootPlan {
                mutable_new_roots: vec![root; usize::from(count)],
                static_roots: vec![],
                endpoint: root,
            },
        )
        .unwrap();
        // Warm engine-wide lazy data before comparing the two workload sizes.
        if count == 8 {
            captured(&fixture, &claims);
        }
        let bytes = allocated_bytes(|| {
            captured(&fixture, &claims);
        });
        measurements.push(bytes);
    }
    println!("SIBLING_THREAD_ALLOCATIONS={measurements:?}");
}

#[test]
fn frozen_mutable_delivery_gas_vector() {
    // Nonempty calldata stores GAS and stops; empty calldata reads the slot.
    let fixture = Fixture::with_probe_code(
        false,
        Some(hex::decode("36156009575a5f55005b5f545f5260205ff3").unwrap()),
    );
    let root = pin(&fixture);
    let mut write = history_observation_claim(root, CallMode::Static, Bytes::new());
    write.call_mode = CallMode::Mutable;
    write.data = Bytes::from_static(&[1]);
    write.rollback_spans = vec![2];
    write = with_mutable_delivery(write);
    let mut read = history_observation_claim(root, CallMode::Static, Bytes::new());
    let block: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    let observations = [write.clone(), read.clone()]
        .into_iter()
        .map(|claim| BlockObservation {
            after_transactions: block.body.transactions.len(),
            manager_address: crate::EEZL2_ADDRESS,
            claim,
        })
        .collect::<Vec<_>>();
    let (_, outcomes) = fixture.captured(&observations);
    assert!(outcomes.iter().all(|x| x.success));
    assert_eq!(outcomes[1].output.len(), 32);
    read.expected_output = outcomes[1].output.clone();
    let restored = history_observation_claim(root, CallMode::Static, vec![0; 32].into());
    captured(&fixture, &[write, read, restored]);
    let vector = serde_json::json!({"case":"mutable-delivery-gas-retained-read-rollback", "output":hex::encode(&outcomes[1].output),
        "blocks":fixture.blocks.iter().map(|b|hex::encode(&b.hash)).collect::<Vec<_>>()});
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("capture-reth27-mutable-gas.json")).unwrap();
    assert_eq!(
        vector, expected,
        "frozen REVM native-delivery gas and commitments"
    );
    println!("MUTABLE_GAS_VECTOR={vector}");
}
