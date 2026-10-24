//! CI-only prefix reconstruction from the test node's execution witness and
//! chain configuration, deployed genesis and checked L1 headers. Stateless replays pre-execution
//! changes and every transaction; receipts/body are bound to the canonical terminal header.
//! Chain identity/canonicality still rely on the surrounding RPC-backed check.

use std::{error::Error, sync::Arc};

use reth_chainspec::EthChainSpec as _;
use reth_primitives_traits::Block as _;

use alloy_consensus::{
    ReceiptWithBloom, Transaction as _, TxReceipt as _, Typed2718 as _,
    proofs::{calculate_receipt_root, calculate_transaction_root},
};
use alloy_eips::{
    eip2718::{Decodable2718 as _, Encodable2718 as _},
    eip7685::EMPTY_REQUESTS_HASH,
};
use alloy_primitives::{B256, Bloom};
use eez_primitives::{Block, Receipt};
use eez_protocol::l1_history::{
    HeaderChain, MAX_HEADER_BYTES, MAX_IMPORT_HEADERS, WitnessHistory, decode_import,
};

type ReceiptEnvelope = ReceiptWithBloom<Receipt>;
use serde_json::{Value, json};

use super::{required, required_b256, required_hex_bytes};

pub(super) fn verify(input: &Value) -> Result<Value, Box<dyn Error>> {
    let (block, receipts) = decode(input)?;
    let witness = serde_json::from_value(required(input, "/witness")?.clone())?;
    let spec = replay_spec(input)?;
    let history = replay_history(input, &spec, &block)?;
    let evm = eez_evm::EezEvmConfig::new(Arc::clone(&spec))
        .with_l1_history(Arc::new(WitnessHistory(history)))?;
    let indices: Vec<_> = (0..block.body.transactions.len()).collect();
    let replay = stateless_reth::stateless_validation_recovered_with_state_checkpoints(
        block.clone().try_into_recovered()?,
        witness,
        Arc::clone(&spec),
        evm,
        &indices,
    )?;
    if replay.validation.execution_output.receipts
        != receipts
            .iter()
            .map(|r| r.receipt.clone())
            .collect::<Vec<_>>()
    {
        return Err("RPC receipts differ from independent witness replay".into());
    }
    let roots: Vec<_> = replay
        .checkpoints
        .transaction_state_checkpoints
        .iter()
        .map(|c| c.state_root)
        .collect();
    reconstruct(
        &block,
        &receipts,
        &roots,
        replay.checkpoints.pre_transaction_state_root,
    )
}

fn replay_history(
    input: &Value,
    spec: &reth_chainspec::ChainSpec,
    block: &Block,
) -> Result<Arc<HeaderChain>, Box<dyn Error>> {
    use eez_l1::chain_config::{
        ChainDocumentKind, load_l1_chain_spec, offline_l1_rules, parse_chain_document_with_digest,
    };

    // Use the runtime's fixed public presets or the embedded Ethereum genesis,
    // never a path supplied in evidence. Bind the resulting rules, L2 schedule
    // and fee collector to the manager selected at the original post.
    let document = required(input, "/l1_chain_config")?;
    let l1_spec = if let Some(name) = document.as_str() {
        if !matches!(name, "mainnet" | "sepolia" | "hoodi" | "chiado") {
            return Err("unsupported prefix L1 preset".into());
        }
        load_l1_chain_spec(std::path::Path::new(name)).map_err(|error| error.to_string())?
    } else {
        let document = parse_chain_document_with_digest(&serde_json::to_vec(document)?)
            .map_err(|error| format!("invalid L1 genesis: {error}"))?;
        if document.kind() != ChainDocumentKind::Genesis {
            return Err("prefix replay requires the complete deployed L1 genesis".into());
        }
        let genesis = document.into_genesis();
        if matches!(genesis.config.chain_id, 100 | 10200) {
            return Err("Gnosis rules require a supported public preset".into());
        }
        Arc::new(reth_chainspec::ChainSpec::from_genesis(genesis))
    };
    let rules = offline_l1_rules(&l1_spec).map_err(|error| error.to_string())?;
    let profile = rules
        .profile(eez_evm::profile::schedule(spec)?, spec.chain().id())
        .map_err(|error| error.to_string())?;
    if profile.commitment() != required_b256(input, "/protocol_config_hash")? {
        return Err("prefix replay profile differs from the deployed manager commitment".into());
    }

    // One inherited endpoint plus the hashes in the scheduled import suffice.
    // RPC storage reads only locate these bytes; execution below authenticates
    // the endpoint against the parent trie, import calldata and PREVRANDAO.
    let mut count = 1;
    let mut imports = block.body.transactions.iter().filter(|tx| tx.is_history());
    if let Some(tx) = imports.next() {
        count += decode_import(tx.input())?.hashes.len();
    }
    if imports.next().is_some() {
        return Err("multiple history transactions in prefix block".into());
    }
    let headers = required(input, "/l1_headers_rlp")?
        .as_array()
        .ok_or("l1_headers_rlp is not an array")?;
    if headers.is_empty() || headers.len() > MAX_IMPORT_HEADERS + 1 || headers.len() != count {
        return Err(
            "prefix L1 evidence must contain exactly the inherited endpoint and imports".into(),
        );
    }
    let raw_headers = headers
        .iter()
        .map(|raw| {
            let encoded = raw.as_str().ok_or("L1 header is not a hex string")?;
            if encoded.len() < 4 || encoded.len() > 2 + 2 * MAX_HEADER_BYTES {
                return Err("L1 header byte limit".into());
            }
            super::decode_hex_string(encoded, "L1 header")
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let mut chain = HeaderChain::default();
    chain.append(&raw_headers, &rules.headers, MAX_IMPORT_HEADERS + 1)?;
    Ok(Arc::new(chain))
}

fn replay_spec(input: &Value) -> Result<Arc<reth_chainspec::ChainSpec>, Box<dyn Error>> {
    let genesis: alloy_genesis::Genesis =
        serde_json::from_value(required(input, "/genesis")?.clone())?;
    let config: alloy_genesis::ChainConfig =
        serde_json::from_value(required(input, "/chain_config")?.clone())?;
    // The execution schedule depends on the actual genesis header, including
    // its timestamp and L1 reference. Chain configuration alone is insufficient.
    let spec = eez_evm::profile::chain_spec(genesis)?;
    let mut expected_config = spec.genesis.config.clone();
    // Reth 2.7's debug_chainConfig materializes default blob parameters. This
    // profile activates all three forks at timestamp zero; explicit genesis
    // entries take precedence, just as in Reth's RPC serialization.
    let params = spec
        .blob_params_at_timestamp(0)
        .ok_or("validated L2 profile has no genesis blob parameters")?;
    for fork in ["cancun", "prague", "osaka"] {
        expected_config
            .blob_schedule
            .entry(fork.into())
            .or_insert(params);
    }
    // BlobParams has execution-only fields omitted by the RPC wire format.
    if serde_json::to_value(expected_config)? != serde_json::to_value(config)? {
        return Err("deployed genesis configuration differs from RPC chain configuration".into());
    }
    if spec.genesis_header.hash() != required_b256(input, "/genesis_hash")? {
        return Err("deployed genesis differs from the canonical RPC genesis hash".into());
    }
    Ok(Arc::new(spec))
}

fn decode(input: &Value) -> Result<(Block, Vec<ReceiptEnvelope>), Box<dyn Error>> {
    let raw = required_hex_bytes(input, "/block_rlp")?;
    let block = alloy_rlp::decode_exact::<Block>(&raw)?;
    let expected_hash = required_b256(input, "/block_hash")?;
    if block.header.hash_slow() != expected_hash || alloy_rlp::encode(&block) != raw.as_ref() {
        return Err("prefix block does not match the authenticated block hash".into());
    }
    let raw_receipts = required(input, "/receipts_rlp")?
        .as_array()
        .ok_or("receipts_rlp is not an array")?;
    let mut receipts = Vec::with_capacity(raw_receipts.len());
    for raw in raw_receipts {
        let encoded = super::decode_hex_string(
            raw.as_str().ok_or("receipt is not a hex string")?,
            "receipt",
        )?;
        let receipt = ReceiptEnvelope::decode_2718(&mut encoded.as_ref())?;
        if receipt.encoded_2718() != encoded.as_ref() {
            return Err("receipt encoding is not canonical".into());
        }
        if receipt.logs().iter().collect::<Bloom>() != receipt.logs_bloom {
            return Err("receipt bloom does not match its logs".into());
        }
        receipts.push(receipt);
    }
    Ok((block, receipts))
}

fn reconstruct(
    block: &Block,
    receipts: &[ReceiptEnvelope],
    roots: &[B256],
    pre_transaction_root: B256,
) -> Result<Value, Box<dyn Error>> {
    let txs = &block.body.transactions;
    if roots.len() != txs.len() || receipts.len() != txs.len() {
        return Err(
            "prefix evidence must contain one state root and receipt per transaction".into(),
        );
    }
    if !block.body.ommers.is_empty()
        || block
            .body
            .withdrawals
            .as_ref()
            .is_some_and(|items| !items.is_empty())
        || block
            .header
            .requests_hash
            .is_some_and(|hash| hash != EMPTY_REQUESTS_HASH)
    {
        return Err(
            "prefix reconstruction does not support ommers, withdrawals or execution requests"
                .into(),
        );
    }
    if calculate_transaction_root(txs) != block.header.transactions_root
        || calculate_receipt_root(receipts) != block.header.receipts_root
        || roots.last().copied().unwrap_or(pre_transaction_root) != block.header.state_root
    {
        return Err(
            "prefix evidence disagrees with terminal transaction/receipt/state roots".into(),
        );
    }
    let mut prefixes = Vec::with_capacity(txs.len() + 1);
    let mut header = block.header.clone();
    header.state_root = pre_transaction_root;
    header.transactions_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
    header.receipts_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
    header.logs_bloom = Bloom::ZERO;
    header.gas_used = 0;
    if header.blob_gas_used.is_some() {
        header.blob_gas_used = Some(0);
    }
    let hash = header.hash_slow();
    if txs.is_empty() && hash != block.header.hash_slow() {
        return Err("empty prefix differs from the canonical empty block".into());
    }
    prefixes.push(json!({"transaction_index": null, "transaction_count": 0, "hash": hash, "state_root": pre_transaction_root}));
    let mut bloom = Bloom::ZERO;
    let mut gas = 0;
    let mut blob_gas = 0_u64;
    for (index, ((tx, receipt), root)) in txs.iter().zip(receipts).zip(roots).enumerate() {
        if receipt.cumulative_gas_used() < gas || receipt.ty() != tx.ty() {
            return Err("prefix receipts have invalid cumulative gas or transaction types".into());
        }
        gas = receipt.cumulative_gas_used();
        bloom |= receipt.bloom();
        blob_gas = blob_gas
            .checked_add(tx.blob_gas_used().unwrap_or(0))
            .ok_or("blob gas overflow")?;
        let mut header = block.header.clone();
        header.state_root = *root;
        header.transactions_root = calculate_transaction_root(&txs[..=index]);
        header.receipts_root = calculate_receipt_root(&receipts[..=index]);
        header.logs_bloom = bloom;
        header.gas_used = gas;
        if header.blob_gas_used.is_some() {
            header.blob_gas_used = Some(blob_gas);
        }
        let hash = header.hash_slow();
        if index + 1 == txs.len() && hash != block.header.hash_slow() {
            return Err(
                "final reconstructed prefix is not the authenticated terminal block".into(),
            );
        }
        prefixes.push(json!({"transaction_index": index, "transaction_count": index + 1, "hash": hash, "state_root": root}));
    }
    Ok(
        json!({"verified": true, "block_hash": block.header.hash_slow(),
        "block_number": block.header.number, "state_root_source": "stateless_execution_witness_replay",
        "prefixes": prefixes}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxLegacy;
    use alloy_primitives::{Signature, U256};
    use alloy_sol_types::SolCall as _;
    use eez_primitives::EezTxType;
    use eez_protocol::l1_history::{HISTORY_ADDRESS, HistorySchedule, bootstrap_storage};
    use reth_ethereum_primitives::{Transaction, TransactionSigned};

    const REFERENCE_TIMESTAMP: u64 = 1_687_223_762;

    fn captured_input(import: bool) -> Value {
        let mut input: Value =
            serde_json::from_slice(include_bytes!("fixtures/prefix-history.json")).unwrap();
        if import {
            let update: Value =
                serde_json::from_slice(include_bytes!("fixtures/prefix-history-import.json"))
                    .unwrap();
            input
                .as_object_mut()
                .unwrap()
                .extend(update.as_object().unwrap().clone());
        }
        input
    }

    fn write_block(input: &mut Value, block: &Block) {
        input["block_hash"] = json!(block.header.hash_slow());
        input["block_rlp"] = json!(alloy_primitives::Bytes::from(alloy_rlp::encode(block)));
    }

    fn mutate_header(
        input: &mut Value,
        index: usize,
        mutate: impl FnOnce(&mut alloy_consensus::Header),
    ) {
        let bytes: alloy_primitives::Bytes =
            serde_json::from_value(input["l1_headers_rlp"][index].clone()).unwrap();
        let mut header = alloy_rlp::decode_exact(&bytes).unwrap();
        mutate(&mut header);
        input["l1_headers_rlp"][index] =
            json!(alloy_primitives::Bytes::from(alloy_rlp::encode(header)));
    }

    #[test]
    fn captured_prefixes_replay_inherited_and_imported_l1_history() {
        for (import, number, transactions) in [(false, 9468, 3), (true, 9465, 1)] {
            let input = captured_input(import);
            let output = verify(&input).unwrap();
            let prefixes = output["prefixes"].as_array().unwrap();
            assert_eq!(output["block_number"], number);
            assert_eq!(
                output["state_root_source"],
                "stateless_execution_witness_replay"
            );
            assert_eq!(prefixes.len(), transactions + 1);
            assert_eq!(prefixes.last().unwrap()["hash"], input["block_hash"]);
            assert_eq!(prefixes.last().unwrap()["transaction_count"], transactions);
            if !import {
                // Expected pin comes from the original canonical L1 post's
                // reverted lookup row, independently of this replay helper.
                let expected = "0xe889943a26b2ec20ef054994650839c1b6f12b2b51b762d9360f4c7cc0c87873";
                assert_eq!(
                    prefixes
                        .iter()
                        .filter(|prefix| prefix["hash"] == expected)
                        .count(),
                    1
                );
                assert_ne!(output["block_hash"], expected);
            }
        }
    }

    #[test]
    fn prefix_history_rejects_missing_extra_and_malformed_evidence() {
        for import in [false, true] {
            for (mutation, expected) in [
                ("missing", "/l1_headers_rlp"),
                ("empty", "exactly the inherited endpoint and imports"),
                ("extra", "exactly the inherited endpoint and imports"),
                ("too_many", "exactly the inherited endpoint and imports"),
                ("too_large", "L1 header byte limit"),
                ("trailing_rlp", "invalid L1 header RLP"),
                (
                    "fork_layout",
                    "L1 header does not match configured fork layout",
                ),
            ] {
                let mut input = captured_input(import);
                match mutation {
                    "missing" => {
                        input.as_object_mut().unwrap().remove("l1_headers_rlp");
                    }
                    "empty" => input["l1_headers_rlp"] = json!([]),
                    "extra" => {
                        let extra = input["l1_headers_rlp"][0].clone();
                        input["l1_headers_rlp"].as_array_mut().unwrap().push(extra);
                    }
                    "too_many" => {
                        input["l1_headers_rlp"] = json!(vec!["0xc0"; MAX_IMPORT_HEADERS + 2])
                    }
                    "too_large" => {
                        input["l1_headers_rlp"][0] =
                            json!(format!("0x{}", "00".repeat(MAX_HEADER_BYTES + 1)))
                    }
                    "trailing_rlp" => {
                        input["l1_headers_rlp"][0] = json!(format!(
                            "{}00",
                            input["l1_headers_rlp"][0].as_str().unwrap()
                        ))
                    }
                    "fork_layout" => {
                        mutate_header(&mut input, 0, |header| header.requests_hash = None)
                    }
                    _ => unreachable!(),
                }
                let error = verify(&input).unwrap_err().to_string();
                assert!(error.contains(expected), "{import}/{mutation}: {error}");
            }
        }
    }

    #[test]
    fn prefix_history_rejects_uncommitted_l1_rules_and_file_selectors() {
        for (pointer, value, expected) in [
            (
                "/protocol_config_hash",
                json!(B256::ZERO),
                "deployed manager commitment",
            ),
            (
                "/l1_chain_config/config/pragueTime",
                json!(1),
                "deployed manager commitment",
            ),
            (
                "/l1_chain_config",
                json!({"chainId": 7331}),
                "complete deployed L1 genesis",
            ),
            (
                "/l1_chain_config",
                json!("/tmp/untrusted-genesis.json"),
                "unsupported prefix L1 preset",
            ),
            (
                "/l1_chain_config",
                json!("chiado"),
                "L1 reference/profile chain mismatch",
            ),
        ] {
            let mut input = captured_input(false);
            *input.pointer_mut(pointer).unwrap() = value;
            let error = verify(&input).unwrap_err().to_string();
            assert!(error.contains(expected), "{pointer}: {error}");
        }
    }

    #[test]
    fn prefix_replay_binds_checked_headers_to_parent_storage_and_prevrandao() {
        let mut input = captured_input(false);
        mutate_header(&mut input, 0, |header| header.mix_hash = B256::ZERO);
        let (block, _) = decode(&input).unwrap();
        let spec = replay_spec(&input).unwrap();
        // The mutated header remains canonical and fork-correct. Rejection
        // must occur only after loading the authenticated parent history trie.
        assert!(replay_history(&input, &spec, &block).is_ok());
        assert!(
            verify(&input)
                .unwrap_err()
                .to_string()
                .contains("parent L1 history state mismatch")
        );

        for import in [false, true] {
            let mut input = captured_input(import);
            let (mut block, _) = decode(&input).unwrap();
            block.header.mix_hash = B256::ZERO;
            write_block(&mut input, &block);
            assert!(
                verify(&input)
                    .unwrap_err()
                    .to_string()
                    .contains("L2 PREVRANDAO differs from selected L1 origin")
            );
        }
    }

    #[test]
    fn prefix_replay_binds_imports_to_exact_headers_and_native_identity() {
        let mut input = captured_input(true);
        mutate_header(&mut input, 1, |header| header.mix_hash = B256::ZERO);
        let (block, _) = decode(&input).unwrap();
        let spec = replay_spec(&input).unwrap();
        assert!(replay_history(&input, &spec, &block).is_ok());
        assert!(
            verify(&input)
                .unwrap_err()
                .to_string()
                .contains("history transaction differs from authenticated headers")
        );

        let mut input = captured_input(true);
        let (mut block, _) = decode(&input).unwrap();
        let eez_primitives::EezTxEnvelope::System(tx) = &block.body.transactions[0] else {
            panic!("captured block must start with a native history transaction");
        };
        let mut tx = (**tx).clone();
        tx.source_hash = B256::ZERO;
        block.body.transactions[0] = tx.into();
        block.header.transactions_root = calculate_transaction_root(&block.body.transactions);
        write_block(&mut input, &block);
        assert!(
            verify(&input)
                .unwrap_err()
                .to_string()
                .contains("history source identity differs from its genesis or L2 position")
        );
    }

    #[test]
    fn prefix_history_cardinality_follows_the_recorded_import_including_repeats() {
        let mut input = captured_input(true);
        let (mut block, _) = decode(&input).unwrap();
        let spec = replay_spec(&input).unwrap();
        let eez_primitives::EezTxEnvelope::System(tx) = &block.body.transactions[0] else {
            panic!("captured native import");
        };
        let mut tx = (**tx).clone();
        let mut call = decode_import(&tx.input).unwrap();
        call.hashes.clear();
        tx.input = call.abi_encode().into();
        block.body.transactions[0] = tx.clone().into();
        assert!(
            replay_history(&input, &spec, &block)
                .unwrap_err()
                .to_string()
                .contains("exactly the inherited endpoint and imports")
        );
        input["l1_headers_rlp"].as_array_mut().unwrap().truncate(1);
        assert_eq!(
            replay_history(&input, &spec, &block)
                .unwrap()
                .headers()
                .len(),
            1
        );
        block.body.transactions.push(tx.into());
        assert!(
            replay_history(&input, &spec, &block)
                .unwrap_err()
                .to_string()
                .contains("multiple history transactions")
        );

        let mut input = captured_input(true);
        let (block, _) = decode(&input).unwrap();
        input["l1_headers_rlp"].as_array_mut().unwrap().swap(0, 1);
        assert!(
            replay_history(&input, &spec, &block)
                .unwrap_err()
                .to_string()
                .contains("nonconsecutive L1 ancestry")
        );
    }

    #[test]
    fn prefix_history_accepts_the_import_bound_and_rejects_one_more() {
        let mut input = captured_input(true);
        let (mut block, _) = decode(&input).unwrap();
        let spec = replay_spec(&input).unwrap();
        let bytes: alloy_primitives::Bytes =
            serde_json::from_value(input["l1_headers_rlp"][0].clone()).unwrap();
        let mut header: alloy_consensus::Header = alloy_rlp::decode_exact(&bytes).unwrap();
        let mut headers = vec![json!(bytes)];
        let mut hashes = Vec::new();
        for _ in 0..MAX_IMPORT_HEADERS {
            header.parent_hash = header.hash_slow();
            header.number += 1;
            header.timestamp += 12;
            headers.push(json!(alloy_primitives::Bytes::from(alloy_rlp::encode(
                &header
            ))));
            hashes.push(header.hash_slow());
        }
        let eez_primitives::EezTxEnvelope::System(tx) = &block.body.transactions[0] else {
            panic!("captured native import");
        };
        let mut tx = (**tx).clone();
        let mut call = decode_import(&tx.input).unwrap();
        call.hashes = hashes;
        tx.input = call.abi_encode().into();
        block.body.transactions[0] = tx.clone().into();
        input["l1_headers_rlp"] = json!(headers);
        // This tests bounded evidence decoding, not authorization of the
        // synthetic future selection. Actual replay is covered above.
        assert_eq!(
            replay_history(&input, &spec, &block)
                .unwrap()
                .headers()
                .len(),
            65
        );
        call.hashes.push(B256::ZERO);
        tx.input = call.abi_encode().into();
        block.body.transactions[0] = tx.into();
        assert!(
            replay_history(&input, &spec, &block)
                .unwrap_err()
                .to_string()
                .contains("invalid history transaction calldata")
        );
    }

    fn profile_input() -> Value {
        let mut genesis: alloy_genesis::Genesis =
            serde_json::from_slice(include_bytes!("../../../../genesis.json")).unwrap();
        // Keep the nonzero private-L1 reference at the bootstrap cutoff, one
        // complete interval before genesis, and install the matching storage.
        genesis.timestamp = REFERENCE_TIMESTAMP + 4;
        let rollup_x = genesis.config.extra_fields.get_mut("rollupX").unwrap();
        rollup_x["l1Reference"]["chainId"] = json!(1337);
        rollup_x["l1Reference"]["timestamp"] = json!(REFERENCE_TIMESTAMP);
        rollup_x["blockIntervalSeconds"] = json!(1);
        rollup_x["syncIntervalBlocks"] = json!(4);
        rollup_x["historyUpdateOffset"] = json!(2);
        let config: eez_protocol::l2::DerivationConfig =
            serde_json::from_value(rollup_x.clone()).unwrap();
        let history = HistorySchedule::new(
            genesis.timestamp,
            config.block_interval_seconds,
            config.sync_interval_blocks,
            config.history_update_offset,
        )
        .unwrap();
        genesis.alloc.get_mut(&HISTORY_ADDRESS).unwrap().storage = Some(
            bootstrap_storage(
                config.l1_reference.number,
                config.l1_reference.hash,
                history,
            )
            .unwrap(),
        );
        let spec = eez_evm::profile::chain_spec(genesis.clone()).unwrap();
        let mut config = genesis.config.clone();
        for fork in ["cancun", "prague", "osaka"] {
            config
                .blob_schedule
                .insert(fork.into(), alloy_eips::eip7840::BlobParams::prague());
        }
        json!({"chain_config": config, "genesis": genesis,
            "genesis_hash": spec.genesis_header.hash()})
    }

    #[test]
    fn witness_replay_preserves_the_nonzero_genesis_schedule() {
        let input = profile_input();
        let spec = replay_spec(&input).unwrap();
        let mut header = spec.genesis_header.clone().into_header();
        assert_eq!(header.timestamp, REFERENCE_TIMESTAMP + 4);
        assert_eq!(
            spec.genesis,
            serde_json::from_value::<alloy_genesis::Genesis>(input["genesis"].clone()).unwrap()
        );
        let schedule = eez_evm::profile::schedule(spec.as_ref()).unwrap();
        assert_eq!(schedule.l1_slot_seconds(), 4);
        let history = schedule.history().unwrap();
        assert_eq!(history.bootstrap_cutoff(), REFERENCE_TIMESTAMP);
        assert_eq!(history.first_cutoff(), REFERENCE_TIMESTAMP + 4);
        assert_eq!(history.interval(), 4);
        assert_eq!(history.delay(), 3);
        let (number, timestamp) = schedule.next(&header).unwrap();
        assert_eq!((number, timestamp), (1, REFERENCE_TIMESTAMP + 5));
        header.number = number;
        header.timestamp = timestamp;
        schedule.validate_position(&header).unwrap();
    }

    #[test]
    fn replay_rejects_mismatched_or_invalid_genesis_without_repair() {
        for (pointer, value, error) in [
            (
                "/genesis_hash",
                json!(B256::ZERO),
                "canonical RPC genesis hash",
            ),
            (
                "/chain_config/chainId",
                json!(999),
                "RPC chain configuration",
            ),
            (
                "/genesis/timestamp",
                json!("0x0"),
                "genesis precedes its L1 reference",
            ),
            (
                "/genesis/timestamp",
                json!(format!("{REFERENCE_TIMESTAMP:#x}")),
                "genesis reference is after bootstrap cutoff",
            ),
            ("/genesis/gasLimit", json!("0x1"), "gas limit"),
            (
                "/chain_config/blobSchedule/cancun/max",
                json!(8),
                "RPC chain configuration",
            ),
        ] {
            let mut input = profile_input();
            *input.pointer_mut(pointer).unwrap() = value;
            assert!(
                replay_spec(&input).unwrap_err().to_string().contains(error),
                "{pointer}"
            );
        }
    }

    #[test]
    fn replay_rejects_inconsistent_history_bootstrap_without_repair() {
        use eez_protocol::l1_history::{INTERVAL_SLOT, NEXT_CUTOFF_SLOT, UPDATE_DELAY_SLOT};

        for (slot, value) in [
            (NEXT_CUTOFF_SLOT, REFERENCE_TIMESTAMP + 8),
            (INTERVAL_SLOT, 12),
            (UPDATE_DELAY_SLOT, 6),
        ] {
            let mut input = profile_input();
            let mut genesis: alloy_genesis::Genesis =
                serde_json::from_value(input["genesis"].clone()).unwrap();
            genesis
                .alloc
                .get_mut(&HISTORY_ADDRESS)
                .unwrap()
                .storage
                .as_mut()
                .unwrap()
                .insert(U256::from(slot).into(), U256::from(value).into());
            input["genesis"] = serde_json::to_value(genesis).unwrap();
            assert!(
                replay_spec(&input)
                    .unwrap_err()
                    .to_string()
                    .contains("L1 history genesis code, account or bootstrap storage"),
                "slot {slot}"
            );
        }
    }

    const PRE_TRANSACTION_ROOT: B256 = B256::repeat_byte(0x33);

    fn verify_recorded_roots(input: &Value) -> Result<Value, Box<dyn Error>> {
        let (block, receipts) = decode(input)?;
        let roots: Vec<B256> = serde_json::from_value(required(input, "/state_roots")?.clone())?;
        reconstruct(&block, &receipts, &roots, PRE_TRANSACTION_ROOT)
    }

    fn fixture() -> (Block, Vec<ReceiptEnvelope>, Vec<B256>) {
        let mut block = Block::default();
        block.header.number = 42;
        block.body.transactions = (0..2)
            .map(|nonce| {
                TransactionSigned::new_unhashed(
                    Transaction::Legacy(TxLegacy {
                        nonce,
                        ..Default::default()
                    }),
                    Signature::new(U256::ONE, U256::ONE, false),
                )
                .into()
            })
            .collect();
        let receipts = (1..=2)
            .map(|index| ReceiptWithBloom {
                receipt: Receipt {
                    tx_type: EezTxType::Ethereum(alloy_consensus::TxType::Legacy),
                    success: true,
                    cumulative_gas_used: index * 21_000,
                    logs: vec![],
                },
                logs_bloom: Bloom::ZERO,
            })
            .collect::<Vec<_>>();
        let roots = vec![B256::repeat_byte(0x11), B256::repeat_byte(0x22)];
        block.header.state_root = roots[1];
        block.header.transactions_root = calculate_transaction_root(&block.body.transactions);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        block.header.gas_used = 42_000;
        (block, receipts, roots)
    }

    fn input(block: &Block, receipts: &[ReceiptEnvelope], roots: &[B256]) -> Value {
        json!({"block_hash":block.header.hash_slow(),
            "block_rlp":format!("0x{}", alloy_primitives::hex::encode(alloy_rlp::encode(block))),
            "receipts_rlp":receipts.iter().map(|r| format!("0x{}", alloy_primitives::hex::encode(r.encoded_2718()))).collect::<Vec<_>>(),
            "state_roots":roots})
    }

    #[test]
    fn supplied_state_roots_do_not_replace_execution_witness_validation() {
        let (block, receipts, roots) = fixture();
        let evidence = input(&block, &receipts, &roots);
        assert!(
            verify(&evidence)
                .unwrap_err()
                .to_string()
                .contains("/witness")
        );
    }

    #[test]
    fn reconstructs_intermediate_and_terminal_headers() {
        let (block, receipts, roots) = fixture();
        let output = verify_recorded_roots(&input(&block, &receipts, &roots)).unwrap();
        assert_eq!(output["prefixes"].as_array().unwrap().len(), 3);
        assert_ne!(output["prefixes"][1]["hash"], output["block_hash"]);
        assert_eq!(output["prefixes"][2]["hash"], output["block_hash"]);
        let mut prefix = block.header.clone();
        prefix.state_root = roots[0];
        prefix.transactions_root = calculate_transaction_root(&block.body.transactions[..1]);
        prefix.receipts_root = calculate_receipt_root(&receipts[..1]);
        prefix.gas_used = 21_000;
        assert_eq!(output["prefixes"][1]["hash"], json!(prefix.hash_slow()));
    }

    #[test]
    fn pre_transaction_prefix_uses_terminal_height_and_pre_execution_state() {
        let (block, receipts, roots) = fixture();
        let before = PRE_TRANSACTION_ROOT;
        let output = reconstruct(&block, &receipts, &roots, before).unwrap();
        assert_eq!(output["prefixes"][0]["transaction_count"], 0);
        assert_eq!(output["prefixes"][0]["state_root"], json!(before));
        assert_ne!(
            output["prefixes"][0]["hash"],
            json!(block.header.parent_hash)
        );
        let mut header = block.header.clone();
        header.state_root = before;
        header.transactions_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
        header.receipts_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
        header.gas_used = 0;
        assert_eq!(output["prefixes"][0]["hash"], json!(header.hash_slow()));
        assert_eq!(output["prefixes"][2]["hash"], output["block_hash"]);
    }

    #[test]
    fn empty_block_has_one_prefix_bound_to_its_pre_transaction_root() {
        let (mut block, _, _) = fixture();
        block.body.transactions.clear();
        block.header.state_root = PRE_TRANSACTION_ROOT;
        block.header.transactions_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
        block.header.receipts_root = alloy_consensus::constants::EMPTY_ROOT_HASH;
        block.header.gas_used = 0;
        let output = reconstruct(&block, &[], &[], PRE_TRANSACTION_ROOT).unwrap();
        assert_eq!(output["prefixes"].as_array().unwrap().len(), 1);
        assert_eq!(output["prefixes"][0]["hash"], output["block_hash"]);
        assert_eq!(output["prefixes"][0]["transaction_count"], 0);
        assert!(reconstruct(&block, &[], &[], B256::ZERO).is_err());
    }

    #[test]
    fn reconstructs_native_transaction_and_receipt_prefixes() {
        let (mut block, mut receipts, roots) = fixture();
        block.body.transactions[0] = eez_primitives::SystemTransaction {
            chain_id: 1,
            version: eez_primitives::SYSTEM_TX_VERSION,
            source_hash: alloy_primitives::B256::with_last_byte(4),
            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
            to: eez_primitives::EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Default::default(),
        }
        .into();
        receipts[0].receipt.tx_type = EezTxType::System;
        block.header.transactions_root = calculate_transaction_root(&block.body.transactions);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert_eq!(receipts[0].encoded_2718()[0], 0x45);
        let output = verify_recorded_roots(&input(&block, &receipts, &roots)).unwrap();
        assert_eq!(output["prefixes"][2]["hash"], output["block_hash"]);
        receipts[0].receipt.tx_type = EezTxType::Ethereum(alloy_consensus::TxType::Legacy);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());
    }

    #[test]
    fn rejects_incomplete_or_inconsistent_replay_evidence() {
        let (block, receipts, roots) = fixture();
        assert!(reconstruct(&block, &receipts[..1], &roots, PRE_TRANSACTION_ROOT).is_err());
        assert!(reconstruct(&block, &receipts, &roots[..1], PRE_TRANSACTION_ROOT).is_err());
        assert!(
            reconstruct(
                &block,
                &receipts,
                &[roots[0], B256::ZERO],
                PRE_TRANSACTION_ROOT
            )
            .is_err()
        );
        let mut reordered = receipts.clone();
        reordered.reverse();
        assert!(reconstruct(&block, &reordered, &roots, PRE_TRANSACTION_ROOT).is_err());
        for field in [
            "transactions_root",
            "receipts_root",
            "gas_used",
            "logs_bloom",
            "requests_hash",
        ] {
            let mut mutated = block.clone();
            match field {
                "transactions_root" => mutated.header.transactions_root = B256::ZERO,
                "receipts_root" => mutated.header.receipts_root = B256::ZERO,
                "gas_used" => mutated.header.gas_used += 1,
                "logs_bloom" => mutated.header.logs_bloom = Bloom::repeat_byte(1),
                "requests_hash" => mutated.header.requests_hash = Some(B256::ZERO),
                _ => unreachable!(),
            }
            assert!(
                verify_recorded_roots(&input(&mutated, &receipts, &roots)).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn rejects_wrong_block_and_noncanonical_receipt_bytes() {
        let (block, receipts, roots) = fixture();
        let mut evidence = input(&block, &receipts, &roots);
        evidence["block_hash"] = json!(B256::ZERO);
        assert!(verify_recorded_roots(&evidence).is_err());
        let mut evidence = input(&block, &receipts, &roots);
        evidence["receipts_rlp"][0] = json!(format!(
            "{}00",
            evidence["receipts_rlp"][0].as_str().unwrap()
        ));
        assert!(verify_recorded_roots(&evidence).is_err());
    }

    #[test]
    fn rejects_invalid_receipts_even_when_their_root_matches() {
        let (mut block, mut receipts, roots) = fixture();
        receipts[0].receipt.tx_type = EezTxType::Ethereum(alloy_consensus::TxType::Eip1559);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());

        let (mut block, mut receipts, roots) = fixture();
        let receipt = &mut receipts[0];
        receipt.receipt.cumulative_gas_used = 50_000; // next receipt only has 42,000
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());

        let (mut block, mut receipts, roots) = fixture();
        let receipt = &mut receipts[0];
        receipt.logs_bloom = Bloom::repeat_byte(1); // no logs
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());
    }

    #[test]
    fn rejects_unsupported_post_transaction_state_changes() {
        let (mut block, receipts, roots) = fixture();
        block.body.ommers.push(block.header.clone());
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());
        let (mut block, receipts, roots) = fixture();
        block.body.withdrawals = Some(vec![alloy_eips::eip4895::Withdrawal::default()].into());
        assert!(verify_recorded_roots(&input(&block, &receipts, &roots)).is_err());
    }
}
