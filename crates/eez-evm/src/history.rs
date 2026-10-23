//! Mandatory history transition, shared by block production and replay.

use alloy_evm::{
    Evm,
    block::{BlockExecutionError, StateDB},
};
use alloy_primitives::{B256, U256, keccak256};
use eez_protocol::l1_history::{
    ENDPOINT_SLOT, FIRST_NUMBER_SLOT, HISTORY_ADDRESS, HISTORY_RUNTIME, HistorySchedule,
    HistoryState, INTERVAL_SLOT, MAX_IMPORT_HEADERS, MAX_IMPORT_SLOT, NEXT_CUTOFF_SLOT,
    UPDATE_DELAY_SLOT, hash_storage_key,
};
use revm::{Database as _, DatabaseCommit as _, context::Block as _};
use std::sync::Arc;

pub use eez_protocol::l1_history::{L1HistorySource, WitnessHistory};

#[derive(Debug, Clone)]
pub(crate) struct HistoryConfig {
    pub schedule: HistorySchedule,
    pub first_number: u64,
    pub source: Arc<dyn L1HistorySource>,
}

fn error(message: impl Into<String>) -> BlockExecutionError {
    BlockExecutionError::msg(message.into())
}

fn scalar<DB: StateDB>(db: &mut DB, slot: u64) -> Result<u64, BlockExecutionError> {
    let value = db
        .storage(HISTORY_ADDRESS, U256::from(slot))
        .map_err(|_| error("cannot read L1 history storage"))?;
    u64::try_from(value).map_err(|_| error("L1 history scalar exceeds uint64"))
}

pub(crate) fn apply<E: Evm<DB: StateDB>>(
    evm: &mut E,
    config: &HistoryConfig,
) -> Result<(), BlockExecutionError> {
    let timestamp = u64::try_from(evm.block().timestamp())
        .map_err(|_| error("L2 history timestamp exceeds uint64"))?;
    let cutoff = config
        .schedule
        .source_cutoff(timestamp)
        .map_err(BlockExecutionError::other)?;
    let info = evm
        .db_mut()
        .basic(HISTORY_ADDRESS)
        .map_err(|_| error("cannot load L1 history account"))?
        .ok_or_else(|| error("missing L1 history account"))?;
    if info.code_hash != keccak256(HISTORY_RUNTIME) || info.nonce != 1 {
        return Err(error("incorrect L1 history runtime or nonce"));
    }
    let code = evm
        .db_mut()
        .code_by_hash(info.code_hash)
        .map_err(|_| error("missing L1 history runtime witness"))?;
    if code.original_bytes().as_ref() != HISTORY_RUNTIME {
        return Err(error("incorrect L1 history runtime bytes"));
    }
    let db = evm.db_mut();
    let first_number = scalar(db, FIRST_NUMBER_SLOT)?;
    let endpoint = scalar(db, ENDPOINT_SLOT)?;
    let next_cutoff = scalar(db, NEXT_CUTOFF_SLOT)?;
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
    let chain = config
        .source
        .headers(cutoff)
        .map_err(BlockExecutionError::other)?;
    let transition = config
        .schedule
        .execution(
            timestamp,
            HistoryState {
                first_number,
                endpoint,
                endpoint_hash,
                next_cutoff,
            },
            &chain,
        )
        .map_err(BlockExecutionError::other)?;
    if evm.block().prevrandao() != Some(transition.prev_randao) {
        return Err(error("L2 PREVRANDAO differs from selected L1 origin"));
    }
    let Some(calldata) = transition.import else {
        return Ok(());
    };
    let system = evm
        .db_mut()
        .basic(eez_primitives::SYSTEM_ADDRESS)
        .map_err(|_| error("cannot load L1 history system caller"))?;
    let mut result = evm
        .transact_system_call(eez_primitives::SYSTEM_ADDRESS, HISTORY_ADDRESS, calldata)
        .map_err(|_| error("L1 history system execution failed"))?;
    if !result.result.is_success()
        || result
            .result
            .output()
            .is_none_or(|output| !output.is_empty())
        || !result.result.logs().is_empty()
    {
        return Err(error(
            "L1 history import reverted, exhausted budget or returned unexpected output",
        ));
    }
    // System calls cannot transfer value, create accounts or mutate unrelated
    // state. Preserve the caller exactly, including its native-transaction nonce.
    if let Some(caller) = result.state.remove(&eez_primitives::SYSTEM_ADDRESS) {
        let previous = system.unwrap_or_default();
        if caller.info.balance != previous.balance
            || caller.info.nonce != previous.nonce
            || caller.info.code_hash != previous.code_hash
            || caller.is_selfdestructed()
            || caller.changed_storage_slots().next().is_some()
        {
            return Err(error("L1 history call changed system account"));
        }
    }
    if result.state.iter().any(|(address, account)| {
        *address != HISTORY_ADDRESS
            && (account.is_touched() || account.changed_storage_slots().next().is_some())
    }) {
        return Err(error("L1 history call changed an unrelated account"));
    }
    let history = result
        .state
        .get(&HISTORY_ADDRESS)
        .ok_or_else(|| error("L1 history call omitted state transition"))?;
    if history.info.balance != info.balance
        || history.info.nonce != info.nonce
        || history.info.code_hash != info.code_hash
        || history.is_selfdestructed()
    {
        return Err(error("L1 history call changed account identity or balance"));
    }
    evm.db_mut().commit(result.state);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
