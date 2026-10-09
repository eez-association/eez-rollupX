//! Read-only held-pool observations served only by Composer fronts.

use std::sync::Arc;

use alloy_primitives::B256;
use eez_composer::HeldPool;
use jsonrpsee::{RpcModule, types::ErrorObjectOwned};

pub(crate) fn module(pool: Arc<HeldPool>) -> eyre::Result<RpcModule<Arc<HeldPool>>> {
    let mut module = RpcModule::new(pool);
    module.register_method("eez_getCrossChainQueueStatus", |_, pool, _| {
        pool.queue_status()
    })?;
    module.register_method("eez_getCrossChainTransaction", |params, pool, _| {
        let hash = params.one::<B256>()?;
        Ok::<_, ErrorObjectOwned>(pool.transaction_status(hash))
    })?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restart_reports_empty_queue_and_unknown_progress_without_fabricated_health() {
        let rpc = module(Arc::new(HeldPool::new(Default::default()))).unwrap();
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
