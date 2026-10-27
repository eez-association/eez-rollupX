//! Native history transaction planning and validation shared by every L2 executor.
use alloy_evm::{
    Evm,
    block::{BlockExecutionError, StateDB},
};
use alloy_primitives::{B256, U256, keccak256};
use eez_primitives::{EezTxEnvelope, HISTORY_TX_VERSION, HistoryTransaction};
use eez_protocol::l1_history::{
    ENDPOINT_SLOT, FIRST_CUTOFF_SLOT, FIRST_NUMBER_SLOT, HISTORY_ADDRESS, HISTORY_RUNTIME,
    HistoryExecution, HistorySchedule, HistoryState, INTERVAL_SLOT, MAX_IMPORT_HEADERS,
    MAX_IMPORT_SLOT, UPDATE_DELAY_SLOT, decode_import, hash_storage_key,
};
use revm::{Database, context::Block as _};
use std::sync::Arc;

pub use eez_protocol::l1_history::{L1HistorySource, WitnessHistory};

#[derive(Debug, Clone)]
pub(crate) struct HistoryConfig {
    pub schedule: HistorySchedule,
    pub first_number: u64,
    pub source: Arc<dyn L1HistorySource>,
    pub l2_chain_id: u64,
}

fn error(message: impl Into<String>) -> BlockExecutionError {
    BlockExecutionError::msg(message.into())
}
fn scalar<DB: Database>(db: &mut DB, slot: u64) -> Result<u64, BlockExecutionError> {
    let value = db
        .storage(HISTORY_ADDRESS, U256::from(slot))
        .map_err(|_| error("cannot read L1 history storage"))?;
    u64::try_from(value).map_err(|_| error("L1 history scalar exceeds uint64"))
}

pub(crate) fn state<DB: Database>(
    db: &mut DB,
    config: &HistoryConfig,
) -> Result<HistoryState, BlockExecutionError> {
    let info = db
        .basic(HISTORY_ADDRESS)
        .map_err(|_| error("cannot load L1 history account"))?
        .ok_or_else(|| error("missing L1 history account"))?;
    if info.code_hash != keccak256(HISTORY_RUNTIME) || info.nonce != 1 {
        return Err(error("incorrect L1 history runtime or nonce"));
    }
    let code = db
        .code_by_hash(info.code_hash)
        .map_err(|_| error("missing L1 history runtime witness"))?;
    if code.original_bytes().as_ref() != HISTORY_RUNTIME {
        return Err(error("incorrect L1 history runtime bytes"));
    }
    let first_number = scalar(db, FIRST_NUMBER_SLOT)?;
    let endpoint = scalar(db, ENDPOINT_SLOT)?;
    let first_cutoff = scalar(db, FIRST_CUTOFF_SLOT)?;
    if first_number != config.first_number
        || scalar(db, INTERVAL_SLOT)? != config.schedule.interval()
        || scalar(db, UPDATE_DELAY_SLOT)? != config.schedule.delay()
        || scalar(db, MAX_IMPORT_SLOT)? != MAX_IMPORT_HEADERS as u64
    {
        return Err(error("L1 history storage differs from fixed profile"));
    }
    let endpoint_hash = B256::from(
        db.storage(HISTORY_ADDRESS, hash_storage_key(endpoint))
            .map_err(|_| error("missing L1 history endpoint witness"))?,
    );
    Ok(HistoryState {
        first_number,
        endpoint,
        endpoint_hash,
        first_cutoff,
    })
}

/// Freeze a producer choice from local parent state and ready evidence. The
/// returned transaction is a block input; replay never reruns producer policy.
pub(crate) fn prepare<DB: Database>(
    db: &mut DB,
    config: &HistoryConfig,
    timestamp: u64,
) -> Result<(HistoryExecution, Option<EezTxEnvelope>), BlockExecutionError> {
    let state = state(db, config)?;
    let cutoff = config
        .schedule
        .source_cutoff(timestamp)
        .map_err(BlockExecutionError::other)?;
    let chain = config
        .source
        .headers(cutoff)
        .map_err(BlockExecutionError::other)?;
    let updating = cutoff.checked_add(config.schedule.delay()) == Some(timestamp);
    let selected = if updating {
        Some(
            chain
                .select(cutoff)
                .map_err(BlockExecutionError::other)?
                .header()
                .number
                .min(state.endpoint.saturating_add(MAX_IMPORT_HEADERS as u64)),
        )
        .filter(|number| *number > state.endpoint)
    } else {
        None
    };
    let execution = config
        .schedule
        .execution(timestamp, state, &chain, selected)
        .map_err(BlockExecutionError::other)?;
    let transaction = if let Some(input) = &execution.import {
        Some(
            HistoryTransaction {
                version: HISTORY_TX_VERSION,
                chain_id: config.l2_chain_id,
                hashes: decode_import(input)
                    .map_err(BlockExecutionError::other)?
                    .hashes,
            }
            .into(),
        )
    } else {
        None
    };
    Ok((execution, transaction))
}

/// Bind inherited state and randomness even when an eligible update is omitted.
/// Returns whether the block header agrees with inheritance; the block executor
/// permits a different value only if transaction zero validates a real update.
pub(crate) fn begin<E: Evm<DB: StateDB>>(
    evm: &mut E,
    config: &HistoryConfig,
) -> Result<bool, BlockExecutionError> {
    let timestamp = u64::try_from(evm.block().timestamp())
        .map_err(|_| error("L2 history timestamp exceeds uint64"))?;
    let execution = transition(evm.db_mut(), config, timestamp, None)?;
    let inherited = evm.block().prevrandao() == Some(execution.prev_randao);
    if !inherited && execution.cutoff.checked_add(config.schedule.delay()) != Some(timestamp) {
        return Err(error("L2 PREVRANDAO differs from inherited L1 origin"));
    }
    Ok(inherited)
}

/// Validate a publicly recorded update against authenticated parent storage.
pub(crate) fn transition<DB: Database>(
    db: &mut DB,
    config: &HistoryConfig,
    timestamp: u64,
    tx: Option<&EezTxEnvelope>,
) -> Result<HistoryExecution, BlockExecutionError> {
    let prior = state(db, config)?;
    let tx = tx.filter(|tx| tx.is_history());
    let hashes = match tx {
        Some(EezTxEnvelope::History(tx)) => {
            tx.validate().map_err(BlockExecutionError::other)?;
            if tx.chain_id != config.l2_chain_id {
                return Err(error("history chain ID differs from configured L2"));
            }
            Some(&tx.hashes)
        }
        Some(_) => return Err(error("invalid history transaction type")),
        None => None,
    };
    let selected = hashes
        .map(|hashes| {
            prior
                .endpoint
                .checked_add(hashes.len() as u64)
                .ok_or_else(|| error("history endpoint overflow"))
        })
        .transpose()?;
    let cutoff = config
        .schedule
        .source_cutoff(timestamp)
        .map_err(BlockExecutionError::other)?;
    let chain = config
        .source
        .headers(cutoff)
        .map_err(BlockExecutionError::other)?;
    let execution = config
        .schedule
        .execution(timestamp, prior, &chain, selected)
        .map_err(BlockExecutionError::other)?;
    if let Some(hashes) = hashes {
        let expected = execution
            .import
            .as_ref()
            .ok_or_else(|| error("missing history execution"))?;
        if decode_import(expected)
            .map_err(BlockExecutionError::other)?
            .hashes
            != *hashes
        {
            return Err(error(
                "history transaction differs from authenticated headers",
            ));
        }
    }
    Ok(execution)
}

pub(crate) fn validate<E: Evm<DB: StateDB>>(
    evm: &mut E,
    config: &HistoryConfig,
    tx: &EezTxEnvelope,
) -> Result<alloy_primitives::Bytes, BlockExecutionError> {
    let timestamp = u64::try_from(evm.block().timestamp())
        .map_err(|_| error("L2 history timestamp exceeds uint64"))?;
    let execution = transition(evm.db_mut(), config, timestamp, Some(tx))?;
    if evm.block().prevrandao() != Some(execution.prev_randao) {
        return Err(error("L2 PREVRANDAO differs from selected L1 origin"));
    }
    execution
        .import
        .ok_or_else(|| error("missing history execution calldata"))
}

#[cfg(test)]
pub(crate) mod tests;
