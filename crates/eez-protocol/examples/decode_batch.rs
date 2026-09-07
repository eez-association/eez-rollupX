//! Decode `postAndVerifyBatch` calldata for settlement diagnostics.
use alloy_primitives::{B256, hex};
use alloy_sol_types::SolCall;
use eez_protocol::abi::{ExecutionEntrySol, postAndVerifyBatchCall};

fn dump_entry(label: &str, e: &ExecutionEntrySol) {
    println!("== {label} ==");
    println!("  proxyEntryHash = {}", e.proxyEntryHash);
    println!("  destinationRollupId = {}", e.destinationRollupId);
    println!("  rootUpdates.len() = {}", e.rootUpdates.len());
    for (j, d) in e.rootUpdates.iter().enumerate() {
        println!(
            "    update[{j}] rollupId={} currentRoot={} newRoot={} etherDelta={}",
            d.rollupId, d.currentRoot, d.newRoot, d.etherDelta
        );
    }
    println!("  l2ToL1Calls.len() = {}", e.l2ToL1Calls.len());
    for (j, c) in e.l2ToL1Calls.iter().enumerate() {
        println!(
            "    call[{j}] target={} value={} source={} sourceRollupId={} revertNextNCalls={} isStatic={} gas={} data=0x{}",
            c.targetAddress,
            c.value,
            c.sourceAddress,
            c.sourceRollupId,
            c.revertNextNCalls,
            c.isStatic,
            c.gas,
            hex::encode(&c.data)
        );
    }
    println!(
        "  expectedL1ToL2Calls.len() = {}",
        e.expectedL1ToL2Calls.len()
    );
    println!("  success = {}", e.success);
    println!("  returnData = 0x{}", hex::encode(&e.returnData));
    println!("  rollingHash = {}", e.rollingHash);
    let dir = if e.proxyEntryHash == B256::ZERO && !e.l2ToL1Calls.is_empty() {
        "OUTBOUND immediate"
    } else if e.proxyEntryHash != B256::ZERO && !e.l2ToL1Calls.is_empty() {
        "INBOUND deferred"
    } else if e.proxyEntryHash == B256::ZERO && e.l2ToL1Calls.is_empty() {
        "ANCHOR (settlement-only, signs NO system tx)"
    } else {
        "?? unknown"
    };
    println!("  >>> classified: {dir}");
    println!();
}

fn main() {
    let input = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/batch_calldata_raw.hex".into());
    let raw = if input.starts_with("0x") {
        input
    } else {
        std::fs::read_to_string(&input).unwrap_or_else(|error| {
            panic!("failed to read postBatch calldata from {input}: {error}")
        })
    };
    let raw = raw.trim();
    let bytes = hex::decode(raw).unwrap();
    let decoded = postAndVerifyBatchCall::abi_decode(&bytes).unwrap();
    let b = decoded.batch;

    println!("== BATCH TOP-LEVEL ==");
    println!("entries.len() = {}", b.entries.len());
    println!("immediateEntryCount = {}", b.immediateEntryCount);
    println!(
        "immediateStaticEntryCount = {}",
        b.immediateStaticEntryCount
    );
    println!("staticEntries.len() = {}", b.staticEntries.len());
    println!("callData.len() = {}", b.callData.len());
    println!("blobIndices = {:?}", b.blobIndices);
    println!("proofs.len() = {}", b.proofs.len());
    println!();

    // ── on-chain entries[] (the CLAIMED state-delta chain) ──
    println!("######## ON-CHAIN batch.entries[] (CLAIMED chain) ########");
    for (i, e) in b.entries.iter().enumerate() {
        dump_entry(&format!("ON-CHAIN ENTRY {i}"), e);
    }

    println!("######## CLAIMED CHAIN (on-chain entries[] deltas, in order) ########");
    for (i, e) in b.entries.iter().enumerate() {
        for d in &e.rootUpdates {
            println!("entry[{i}]: {} -> {}", d.currentRoot, d.newRoot);
        }
    }

    println!();
    println!(
        "Native DA is carried only in physical blobs; use the eez-l1 \
         verify_blob_evidence example to authenticate and decode it."
    );
}
