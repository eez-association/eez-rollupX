//! Read-only held-pool observations served only by Composer fronts.

use std::sync::Arc;

use alloy_primitives::B256;
use eez_composer::HeldPool;
use jsonrpsee::{RpcModule, types::ErrorObjectOwned};

type RpcState = (Arc<HeldPool>, u64, u64);

pub(crate) fn module(
    pool: Arc<HeldPool>,
    l1_chain_id: u64,
    l2_chain_id: u64,
) -> eyre::Result<RpcModule<RpcState>> {
    let mut module = RpcModule::new((pool, l1_chain_id, l2_chain_id));
    module.register_method("eez_getCrossChainQueueStatus", |_, (pool, _, _), _| {
        pool.queue_status()
    })?;
    module.register_method("eez_getCrossChainTransaction", |params, (pool, l1, l2), _| {
        let hash = params.one::<B256>()?;
        let status = pool.transaction_status(hash).map(|mut status| {
            if status["transactions"].is_null() {
                let inbound = status["direction"] == "inbound";
                let source = serde_json::json!({"chainId":format!("{:#x}",if inbound {l1} else {l2}),"hash":hash});
                status["transactions"] = serde_json::json!({
                    "basis":"source", "complete":false, "attemptGeneration":null,
                    "l1":if inbound {Some(source.clone())} else {None},
                    "l2":if inbound {vec![]} else {vec![source]}, "postBatch":null,
                });
            }
            status
        });
        Ok::<_, ErrorObjectOwned>(status)
    })?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Bytes};
    use eez_composer::{Direction, HeldTx};
    use serde_json::{Value, json};

    #[tokio::test]
    async fn pending_references_use_chain_ids_and_unknown_sources_remain_null() {
        let pool = Arc::new(HeldPool::new(Default::default()));
        let rpc = module(Arc::clone(&pool), 10200, 906969).unwrap();
        for (tag, direction) in [(1, Direction::Inbound), (2, Direction::Outbound)] {
            let hash = B256::repeat_byte(tag);
            pool.push_contiguous(
                HeldTx {
                    raw_tx: Bytes::new(),
                    hash,
                    attempts: 0,
                    excluded_slots: 0,
                    sender: Address::repeat_byte(tag),
                    nonce: 0,
                    direction,
                    max_fee_per_gas: 1,
                    priority_fee_per_gas: 1,
                },
                0,
            )
            .unwrap();
            let request = json!({"jsonrpc":"2.0","id":tag,"method":"eez_getCrossChainTransaction","params":[hash]}).to_string();
            let (response, _) = rpc.raw_json_request(&request, 1).await.unwrap();
            let result: Value = serde_json::from_str(response.get()).unwrap();
            let status = &result["result"];
            assert_eq!(status["hash"], json!(hash));
            assert_eq!(status["lifecycle"], "queued");
            let refs = &status["transactions"];
            assert_eq!(refs["basis"], "source");
            assert_eq!(refs["complete"], false);
            assert!(refs["postBatch"].is_null());
            assert!(refs["attemptGeneration"].is_null());
            if direction == Direction::Inbound {
                assert_eq!(refs["l1"], json!({"chainId":"0x27d8","hash":hash}));
                assert_eq!(refs["l2"], json!([]));
            } else {
                assert!(refs["l1"].is_null());
                assert_eq!(refs["l2"], json!([{"chainId":"0xdd6d9","hash":hash}]));
            }
        }
        let request = json!({"jsonrpc":"2.0","id":9,"method":"eez_getCrossChainTransaction","params":[B256::ZERO]}).to_string();
        let (response, _) = rpc.raw_json_request(&request, 1).await.unwrap();
        let value: Value = serde_json::from_str(response.get()).unwrap();
        assert!(value["result"].is_null());
    }

    #[tokio::test]
    async fn restart_reports_empty_queue_and_unknown_progress_without_fabricated_health() {
        let rpc = module(Arc::new(HeldPool::new(Default::default())), 1, 2).unwrap();
        let (response, _) = rpc
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"eez_getCrossChainQueueStatus","params":[]}"#,
                1,
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(response.get()).unwrap();
        assert_eq!(value["result"]["queued"], 0);
        assert!(value["result"]["lastCanonicalCompletion"].is_null());
        assert!(value["result"]["sourceState"].is_null());
        let (response, _) = rpc.raw_json_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"eez_getCrossChainTransaction","params":["invalid"]}"#, 1
        ).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(response.get()).unwrap();
        assert_eq!(value["error"]["code"], -32602);
    }
}
