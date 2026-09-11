//! Resumed-batch settlement (`settlement.start > 0`): L1 skips leading entries
//! a competitor already applied and settles only the new suffix.
//!
//! No other suite deterministically reaches this path, since a composer
//! produces one only when a peer settles a strict prefix of the same chain.
//! This test therefore extends a real batch read back from L1 and recomputes
//! its attestation. Every commitment used by the extension is an actual L2
//! block hash.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_blob_codec::{
    Blob, BlobSidecarEncoding, BlobTransactionParams, prepare_blob_sidecar,
    prepare_blob_transaction_with_sidecar, sign_blob_transaction,
};
use eez_protocol::abi::{
    EtherDeltaSol, ExecutionEntrySol, ProofSystemBatchPerVerificationEntriesSol as EvmBatch,
    RootUpdateSol, postAndVerifyBatchCall,
};
use eez_protocol::signer::EcdsaProofSigner;

use eez_testkit::{
    ANVIL_ADDR, ANVIL_ATTESTER_KEY, ANVIL_KEY_1, Harness, NodeConfig, NodeHandle, wait_for,
    wait_for_latest_height,
};

const TIMEOUT: Duration = Duration::from_mins(5);

/// Decode the composer's most recent `postAndVerifyBatch` calldata.
async fn last_posted_batch(
    l1_rpc: &str,
    eez: Address,
    from_block: u64,
) -> Option<(EvmBatch, u64, B256)> {
    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse().ok()?);
    let tip = provider.get_block_number().await.ok()?;
    for block_number in (from_block..=tip).rev() {
        let block = provider
            .get_block_by_number(block_number.into())
            .full()
            .await
            .ok()??;
        for transaction in block.transactions.txns() {
            let input = transaction.inner.input();
            if transaction.inner.to() == Some(eez)
                && input.starts_with(&postAndVerifyBatchCall::SELECTOR)
            {
                let call = postAndVerifyBatchCall::abi_decode(input).ok()?;
                return Some((call.batch, block_number, *transaction.inner.tx_hash()));
            }
        }
    }
    None
}

/// Re-attest `batch` against the actual blob payload and submit a type-3 tx.
/// Go through the builder fixture so its Beacon API also serves this new tx.
async fn post_batch(
    harness: &Harness,
    original_hash: B256,
    sender_key: &str,
    attester_key: &str,
    mut batch: EvmBatch,
) -> eyre::Result<B256> {
    let original = harness
        .captured_blob_transaction(original_hash)
        .map_err(|e| eyre::eyre!("{e:#}"))?;
    let sidecar = original
        .as_eip4844()
        .expect("the original batch must be type-3")
        .tx()
        .sidecar()
        .as_eip4844()
        .expect("Anvil uses EIP-4844 sidecars");
    let blobs = sidecar
        .blobs
        .iter()
        .enumerate()
        .map(|(i, blob)| Blob::try_from_slice(i, blob.as_slice()))
        .collect::<Result<Vec<_>, _>>()?;
    let sidecar = prepare_blob_sidecar(&blobs, BlobSidecarEncoding::Eip4844)?;
    assert!(
        !batch.blobIndices.is_empty(),
        "must exercise blob-backed resumption"
    );
    assert_eq!(
        original.blob_versioned_hashes().unwrap(),
        sidecar.versioned_hashes(),
        "re-attestation must bind the original blob contents",
    );
    let attester =
        EcdsaProofSigner::from_private_key(attester_key.trim_start_matches("0x").parse::<B256>()?)?;
    let vkey = B256::left_padding_from(attester.address().as_slice());
    let hashes = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &batch,
        vkey,
        sidecar.versioned_hashes(),
    )?;
    batch.proofs = hashes
        .iter()
        .map(|hash| attester.sign_prehash(*hash))
        .collect::<Result<Vec<Bytes>, _>>()?;

    let signer: alloy_signer_local::PrivateKeySigner =
        sender_key.trim_start_matches("0x").parse()?;
    let chain = harness.chain();
    let provider = ProviderBuilder::new().connect_http(chain.rpc_url().parse()?);
    let calldata = postAndVerifyBatchCall { batch }.abi_encode();
    let signed = sign_blob_transaction(
        prepare_blob_transaction_with_sidecar(
            BlobTransactionParams {
                chain_id: provider.get_chain_id().await?,
                nonce: provider
                    .get_transaction_count(signer.address())
                    .pending()
                    .await?,
                gas_limit: 5_000_000,
                max_fee_per_gas: provider.get_gas_price().await? * 2 + 1_000_000_000,
                max_priority_fee_per_gas: 1_000_000_000,
                max_fee_per_blob_gas: provider.get_blob_base_fee().await?.max(1) * 2,
                to: chain.eez_address(),
                value: U256::ZERO,
                access_list: Default::default(),
                input: calldata.into(),
            },
            sidecar,
        )?,
        &signer,
    )?;
    let builder = ProviderBuilder::new().connect_http(harness.builder_rpc_url().parse()?);
    let _: serde_json::Value = builder
        .client()
        .request(
            "eth_sendBundle",
            (serde_json::json!({
                "txs": [signed.pooled_transaction],
                "blockNumber": format!("0x{:x}", provider.get_block_number().await? + 1),
            }),),
        )
        .await?;
    Ok(signed.transaction_hash)
}

/// Immediate entry moving `rollup_id` between two actual block-hash
/// commitments, without calls or value transfer.
fn transition_immediate(rollup_id: u64, from: B256, to: B256) -> ExecutionEntrySol {
    ExecutionEntrySol {
        rootUpdates: vec![RootUpdateSol {
            rollupId: rollup_id,
            etherDelta: EtherDeltaSol::ZERO,
            currentRoot: from,
            newRoot: to,
        }],
        proxyEntryHash: B256::ZERO,
        l2ToL1Calls: Vec::new(),
        expectedL1ToL2Calls: Vec::new(),
        rollingHash: B256::ZERO,
        destinationRollupId: rollup_id,
        success: true,
        returnData: Bytes::new(),
    }
}

/// Reposting a landed batch with extra immediates appended must settle only
/// the suffix: the original entries are stale, so L1 skips them.
///
/// This asserts that the prefix is skipped, the transaction does not unwind
/// with `AllImmediateL2TxsFailed`, and derivation continues settling afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resumed_batch_settles_only_its_new_suffix() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let env = harness.env().await.unwrap();
    let node = NodeHandle::start("resume-composer", &NodeConfig::default(), &env)
        .await
        .unwrap();

    chain.wait_for_batches(3, TIMEOUT).await.unwrap();
    wait_for_latest_height(&node, 1, TIMEOUT).await.unwrap();

    // Freeze before reading the batch as well as the live commitment, so they
    // refer to the same head even when the next composer batch is already queued.
    chain.set_interval_mining(0).await.unwrap();
    let l1_rpc = chain.rpc_url();
    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse().unwrap());
    // A composer submission may already be in flight. Prevent its account
    // from funding another batch in our manually mined block; the crafted tx
    // uses a different account. Keep the composer alive to observe the resume.
    let poster_balance = provider.get_balance(ANVIL_ADDR).await.unwrap();
    harness
        .anvil
        .set_balance(ANVIL_ADDR, U256::ZERO)
        .await
        .unwrap();
    let (original, posted_at, original_hash) = wait_for(TIMEOUT, || async {
        Ok(last_posted_batch(l1_rpc, chain.eez_address(), chain.deploy_block()).await)
    })
    .await
    .expect("composer posted a batch that can be read back");

    let entries_before = original.entries.len();
    let skipped_before = chain.entries_skipped().await.unwrap();
    let executions_before = chain.executions_performed().await.unwrap();
    let live_hash = chain.state_root().await.unwrap();

    // A round trip `live -> genesis -> live` makes both appended entries apply
    // while preserving the final commitment. A single no-op would positionally
    // match at index zero and never exercise the resumed branch.
    let probe_hash = harness.l2_genesis_block_hash().unwrap();
    assert_ne!(
        probe_hash, live_hash,
        "the probe must differ from the live hash"
    );
    let mut resumed = original.clone();
    resumed.entries.push(transition_immediate(
        chain.rollup_id(),
        live_hash,
        probe_hash,
    ));
    resumed.entries.push(transition_immediate(
        chain.rollup_id(),
        probe_hash,
        live_hash,
    ));
    resumed.immediateEntryCount = U256::from(resumed.entries.len());
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut resumed)
        .expect("rolling hashes for the crafted batch");

    let transaction_hash = post_batch(
        &harness,
        original_hash,
        ANVIL_KEY_1,
        ANVIL_ATTESTER_KEY,
        resumed,
    )
    .await
    .expect("crafted resumed batch submitted");
    chain.mine().await.unwrap();

    let receipt = wait_for(TIMEOUT, || async {
        Ok(provider.get_transaction_receipt(transaction_hash).await?)
    })
    .await
    .unwrap_or_else(|error| panic!("resumed batch receipt: {error:#}"));
    assert!(
        receipt.status(),
        "resumed batch reverted instead of applying its appended suffix",
    );

    let skipped_after = chain.entries_skipped().await.unwrap();
    assert_eq!(
        skipped_after - skipped_before,
        entries_before,
        "expected all {entries_before} already-applied entries from L1 block {posted_at} to be skipped",
    );
    let executions_after = chain.executions_performed().await.unwrap();
    assert_eq!(
        executions_after - executions_before,
        2,
        "exactly the two appended immediates should have applied",
    );
    assert_eq!(
        chain.state_root().await.unwrap(),
        live_hash,
        "the round-trip suffix must preserve the live block-hash commitment",
    );

    harness
        .anvil
        .set_balance(ANVIL_ADDR, poster_balance)
        .await
        .unwrap();
    chain
        .set_interval_mining(eez_testkit::Chain::block_time_secs())
        .await
        .unwrap();
    node.assert_no_divergence_failure_logs();
    let batches_now = chain.batches_posted().await.unwrap();
    chain
        .wait_for_batches(batches_now + 2, TIMEOUT)
        .await
        .expect("composer stopped settling after observing a resumed batch");
    node.assert_no_divergence_failure_logs();
    node.assert_no_process_death();
}
