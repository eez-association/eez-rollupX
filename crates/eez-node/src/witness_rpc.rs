//! Debug-namespace RPC serving the prover's augmented execution witness.
//!
//! Reth's `debug_executionWitnessByBlockHash` returns the minimal witness for
//! the parent->final state transition. EEZ lookup pins name intermediate
//! transaction-prefix roots, and recomputing those roots can collapse a trie
//! branch after an intermediate deletion, which needs sibling nodes the
//! minimal witness omits. This method returns the same witness shape built by
//! the proving feed (`ExecutionWitnessMode::Legacy` plus the removal closure),
//! so an out-of-process verifier replays prefixes from the exact input class
//! the signer validates. The extra nodes are content-addressed; a consumer
//! still computes every root itself and rejects unrelated or corrupted nodes.
//!
//! Re-executing a block is as expensive as Reth's own witness methods, so the
//! method is installed only on transports where the operator enabled the
//! `debug` namespace, and concurrent requests are bounded. A request whose
//! client disconnects keeps its permit until the blocking re-execution exits.

use std::sync::Arc;

use alloy_consensus::Header;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use eez_driver::witness::{ExecutionWitnessMode, block_execution_witness};
use eez_primitives::{Block, EezPrimitives};
use jsonrpsee::{
    RpcModule,
    types::{ErrorObject, ErrorObjectOwned},
};
use reth_evm::ConfigureEvm;
use reth_storage_api::{BlockReader, HeaderProvider, StateProviderFactory, TransactionVariant};
use reth_tasks::TaskExecutor;
use tokio::sync::Semaphore;

/// Method name. The `eez_` prefix keeps it distinct from Reth's debug methods;
/// installation still follows the operator's `debug` namespace selection.
pub(crate) const METHOD: &str = "eez_executionWitnessByBlockHash";

/// Concurrent re-executions admitted by this method. CI verifiers use a small
/// worker pool; further requests wait for a permit instead of spawning more
/// blocking trie walks.
const MAX_CONCURRENT_WITNESSES: usize = 2;

pub(crate) struct State<P, E> {
    provider: P,
    evm_config: E,
    executor: TaskExecutor,
    permits: Arc<Semaphore>,
}

/// Build the module. Returns `null` for a block the node does not know; a
/// known block whose parent state is unavailable (for example pruned) is an
/// internal error rather than a partial witness.
pub(crate) fn module<P, E>(
    provider: P,
    evm_config: E,
    executor: TaskExecutor,
) -> eyre::Result<RpcModule<State<P, E>>>
where
    P: BlockReader<Block = Block>
        + StateProviderFactory
        + HeaderProvider<Header = Header>
        + Clone
        + Send
        + Sync
        + 'static,
    E: ConfigureEvm<Primitives = EezPrimitives> + Clone + Send + Sync + 'static,
{
    let mut module = RpcModule::new(State {
        provider,
        evm_config,
        executor,
        permits: Arc::new(Semaphore::new(MAX_CONCURRENT_WITNESSES)),
    });
    module.register_async_method(METHOD, |params, state, _| async move {
        let hash = params.one::<B256>()?;
        let permit = Arc::clone(&state.permits)
            .acquire_owned()
            .await
            .map_err(|_| internal("witness service is shutting down"))?;
        let provider = state.provider.clone();
        let evm_config = state.evm_config.clone();
        state
            .executor
            .spawn_blocking(move || {
                let _permit = permit;
                witness(&provider, &evm_config, hash)
            })
            .await
            .map_err(|_| internal("witness worker failed"))?
    })?;
    Ok(module)
}

fn witness<P, E>(
    provider: &P,
    evm_config: &E,
    hash: B256,
) -> Result<Option<ExecutionWitness>, ErrorObjectOwned>
where
    P: BlockReader<Block = Block> + StateProviderFactory + HeaderProvider<Header = Header>,
    E: ConfigureEvm<Primitives = EezPrimitives>,
{
    let Some(block) = provider
        .recovered_block(BlockHashOrNumber::Hash(hash), TransactionVariant::WithHash)
        .map_err(|error| internal(format!("fetch block {hash}: {error}")))?
    else {
        return Ok(None);
    };
    block_execution_witness(provider, evm_config, &block, ExecutionWitnessMode::Legacy)
        .map(Some)
        .map_err(|error| internal(format!("witness for block {hash}: {error}")))
}

fn internal(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObject::owned(-32000, message.into(), None::<()>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_evm::EezEvmConfig;
    use reth_chainspec::ChainSpecBuilder;
    use reth_provider::test_utils::MockEthProvider;
    use serde_json::Value;

    async fn call<S: Send + Sync + 'static>(module: &RpcModule<S>, param: &str) -> Value {
        let request =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{METHOD}","params":[{param}]}}"#);
        let (response, _) = module.raw_json_request(&request, 1).await.unwrap();
        serde_json::from_str(response.get()).unwrap()
    }

    #[tokio::test]
    async fn serves_the_proving_feed_witness_and_rejects_bad_input() {
        let provider = MockEthProvider::<EezPrimitives>::new();
        let evm_config = EezEvmConfig::new(Arc::new(
            ChainSpecBuilder::mainnet().cancun_activated().build(),
        ));
        let block = Block {
            header: Header {
                number: 1,
                ..Default::default()
            },
            body: Default::default(),
        };
        let hash = block.header.hash_slow();
        provider.add_block(hash, block);
        let rpc = module(provider.clone(), evm_config.clone(), TaskExecutor::test()).unwrap();

        let unknown = call(&rpc, &format!("\"{}\"", B256::repeat_byte(9))).await;
        assert!(unknown["result"].is_null(), "{unknown}");
        let invalid = call(&rpc, "\"not-a-hash\"").await;
        assert_eq!(invalid["error"]["code"], -32602, "{invalid}");

        // The RPC returns exactly the proving feed's Legacy witness, or the
        // same failure as an error; it never substitutes a partial witness.
        let known = call(&rpc, &format!("\"{hash}\"")).await;
        let recovered = provider
            .recovered_block(BlockHashOrNumber::Hash(hash), TransactionVariant::WithHash)
            .unwrap()
            .unwrap();
        match block_execution_witness(
            &provider,
            &evm_config,
            &recovered,
            ExecutionWitnessMode::Legacy,
        ) {
            Ok(expected) => {
                assert_eq!(
                    known["result"],
                    serde_json::to_value(expected).unwrap(),
                    "{known}"
                );
            }
            Err(_) => assert_eq!(known["error"]["code"], -32000, "{known}"),
        }
    }
}
