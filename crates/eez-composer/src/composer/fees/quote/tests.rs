use super::*;

fn intent() -> Value {
    json!({"type":"0x2","chainId":"0x1","from":Address::repeat_byte(0x11),
        "to":Address::repeat_byte(0x22),"nonce":"0x0","gas":"0x186a0","value":"0x0","data":"0x1234"})
}

#[test]
fn intent_profile_is_explicit_and_bounded() {
    let parsed = L2FeeRequest::parse(&intent()).unwrap();
    assert_eq!(parsed.tx.gas_limit, 100_000);
    for (field, value) in [
        ("gas", json!(100000)),
        ("nonce", json!("0x00")),
        ("chainId", json!("0x0")),
        ("gas", json!("0x4c4b41")),
        ("type", json!("0x4")),
        ("to", Value::Null),
        (
            "data",
            json!(format!("0x{}", "ff".repeat(MAX_CALLDATA + 1))),
        ),
        ("from", json!(eez_primitives::SYSTEM_ADDRESS)),
    ] {
        let mut bad = intent();
        bad[field] = value;
        assert!(L2FeeRequest::parse(&bad).is_err(), "{field}");
    }
    for field in ["gas", "value", "nonce", "from"] {
        let mut bad = intent();
        bad.as_object_mut().unwrap().remove(field);
        assert!(L2FeeRequest::parse(&bad).is_err());
    }
    for field in ["accessList", "authorizationList", "maxFeePerGas"] {
        let mut bad = intent();
        bad[field] = json!([]);
        assert!(L2FeeRequest::parse(&bad).is_err());
    }
}

#[test]
fn signed_admission_recovers_sender_and_preserves_fee_caps() {
    use alloy_network::TxSignerSync;
    let signer = alloy_signer_local::PrivateKeySigner::from_bytes(
        &alloy_primitives::B256::with_last_byte(7),
    )
    .unwrap();
    let mut tx = L2FeeRequest::parse(&intent()).unwrap().tx;
    tx.max_priority_fee_per_gas = 123;
    tx.max_fee_per_gas = 456;
    let signature = signer.sign_transaction_sync(&mut tx).unwrap();
    let envelope = alloy_consensus::TxEnvelope::from(tx.clone().into_signed(signature));
    let request = L2FeeRequest::from_signed(&envelope).unwrap();
    assert_eq!(request.sender, signer.address());
    assert_eq!(request.tx, tx);
    assert!(request.signed_fees);

    tx.access_list = alloy_eips::eip2930::AccessList(vec![alloy_eips::eip2930::AccessListItem {
        address: Address::repeat_byte(0x22),
        storage_keys: vec![],
    }]);
    let signature = signer.sign_transaction_sync(&mut tx).unwrap();
    assert!(matches!(
        L2FeeRequest::from_signed(&tx.into_signed(signature).into()),
        Err(FeeQuoteError::Unsupported(
            "access lists are outside the quote profile"
        ))
    ));
}

#[test]
fn rounded_tip_covers_cost_without_using_the_gas_limit() {
    assert_eq!(required_tip(U256::from(100_001), 21_000).unwrap(), 5);
    assert_eq!(required_tip(U256::ZERO, 21_000).unwrap(), 0);
    assert!(required_tip(U256::MAX, 1).is_err());
    assert!(required_tip(U256::from(1), 0).is_err());
    for gas in [21_000, 99_999, 5_000_000] {
        let amount = U256::from(10_000_000_000_u64);
        let tip = required_tip(amount, gas).unwrap();
        assert!(U256::from(tip) * U256::from(gas) >= amount);
        assert!(U256::from(tip - 1) * U256::from(gas) < amount);
    }
}

#[test]
fn placeholder_bounds_signature_encoding_but_env_has_real_sender() {
    let request = L2FeeRequest::parse(&intent()).unwrap();
    let mut signed = request.tx.clone();
    signed.max_priority_fee_per_gas = 5;
    signed.max_fee_per_gas = 15;
    for signature in [
        Signature::new(U256::from(1), U256::from(2), false),
        Signature::new(U256::MAX, U256::MAX, true),
    ] {
        assert!(
            signed.clone().into_signed(signature).encoded_2718().len()
                <= request.sizing_bytes(5, 15).len()
        );
    }
    let env = request.env(5, 15);
    assert_eq!(env.caller, request.sender);
    assert_eq!(env.gas_priority_fee, Some(5));
    assert_eq!(env.gas_price, 15);
    assert_eq!(env.gas_limit, 100_000);
}

#[test]
fn cancellation_and_frame_exhaustion_fail_closed() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let budget = Budget::new(Arc::clone(&cancelled));
    assert!(budget.check().is_ok());
    budget.exhausted.store(true, Ordering::Relaxed);
    assert!(matches!(
        budget.check(),
        Err(FeeQuoteError::Limit("nested frame budget"))
    ));
    budget.exhausted.store(false, Ordering::Relaxed);
    cancelled.store(true, Ordering::Relaxed);
    assert!(budget.check().is_err());
}

#[test]
fn bounded_session_refuses_further_target_execution() {
    struct Counter(Arc<AtomicUsize>);
    impl TargetExecutionSession for Counter {
        fn execute(
            &mut self,
            _: ExecutionRequest,
            _: &mut CompositionBuilder,
        ) -> ExecutorResult<eez_protocol::ExecutionOutcome> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(eez_protocol::ExecutionOutcome::Resolved {
                return_data: vec![],
                gas_used: 1,
                success: true,
            })
        }
        fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
            Ok(Box::new(()))
        }
        fn rollback(&mut self, _: SessionSnapshot) -> ExecutorResult<()> {
            Ok(())
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let budget = Arc::new(Budget::new(Arc::new(AtomicBool::new(false))));
    let mut session = BoundedSession {
        inner: Box::new(Counter(Arc::clone(&count))),
        budget: Arc::clone(&budget),
    };
    let req = ExecutionRequest {
        call_mode: eez_protocol::CallMode::Mutable,
        target_address: Address::ZERO,
        data: Bytes::new(),
        value: U256::ZERO,
        source_address: Address::ZERO,
        source_rollup_id: RollupId(1),
    };
    let mut builder = CompositionBuilder::new(RollupId(1), HashMap::new());
    for _ in 0..MAX_FRAMES {
        session.execute(req.clone(), &mut builder).unwrap();
    }
    assert!(session.execute(req.clone(), &mut builder).is_err());
    assert!(session.execute(req, &mut builder).is_err());
    assert_eq!(count.load(Ordering::Relaxed), MAX_FRAMES);
    assert!(matches!(
        budget.check(),
        Err(FeeQuoteError::Limit("nested frame budget"))
    ));
}

#[test]
fn history_sizing_matches_encoding_and_refuses_unreconciled_native_blocks() {
    use alloy_network::TxSignerSync;
    use reth_provider::test_utils::MockEthProvider;
    let provider = MockEthProvider::<eez_primitives::EezPrimitives>::new();
    let genesis = Header::default();
    let endpoint = L2Endpoint {
        number: 0,
        hash: genesis.hash_slow(),
    };
    let signer = alloy_signer_local::PrivateKeySigner::from_bytes(
        &alloy_primitives::B256::with_last_byte(7),
    )
    .unwrap();
    let mut tx = L2FeeRequest::parse(&intent()).unwrap().tx;
    let sig = signer.sign_transaction_sync(&mut tx).unwrap();
    let tx = eez_primitives::EezTxEnvelope::from(tx.into_signed(sig));
    let raw = Bytes::from(tx.encoded_2718());
    let beneficiary = Address::repeat_byte(0x44);
    let mut block = eez_primitives::Block::default();
    block.header.number = 1;
    block.header.parent_hash = endpoint.hash;
    block.header.beneficiary = beneficiary;
    block.body.transactions.push(tx);
    let parent = SealedHeader::seal_slow(block.header.clone());
    provider.add_block(parent.hash(), block.clone());
    let budget = Budget::new(Arc::new(AtomicBool::new(false)));
    let measured = history_size(&provider, &parent, endpoint, beneficiary, &budget).unwrap();
    let expected = eez_payload_codec::v0::encode(&[
        BlockRun {
            blocks: 1,
            beneficiary,
            extra_data: &[],
            transactions: &[raw],
        },
        BlockRun {
            blocks: 1,
            beneficiary,
            extra_data: eez_driver::BUILDER_EXTRA_DATA,
            transactions: &[],
        },
    ])
    .unwrap()
    .len();
    assert_eq!(measured, expected);
    assert!(matches!(
        history_size(
            &provider,
            &parent,
            L2Endpoint {
                hash: alloy_primitives::B256::ZERO,
                ..endpoint
            },
            beneficiary,
            &budget
        ),
        Err(FeeQuoteError::Stale)
    ));
    block.body.transactions = vec![
        eez_primitives::SystemTransaction {
            chain_id: 1,
            nonce: 0,
            to: eez_protocol::EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Bytes::new(),
        }
        .into(),
    ];
    provider.add_block(parent.hash(), block);
    assert!(matches!(
        history_size(&provider, &parent, endpoint, beneficiary, &budget),
        Err(FeeQuoteError::Unavailable(
            "unsettled native history requires reconciliation"
        ))
    ));
}
