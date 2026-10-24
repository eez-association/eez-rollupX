//! Native nonce-free execution and ordinary L2 base-fee distribution.
//!
//! Uses the pinned Ethereum instructions, precompiles, validation and gas rules.
//! Native caller accounting and the L2 base-fee destination are specialized;
//! plain and inspected execution run the same handler.

use alloy_evm::{
    Database, EthEvmFactory, Evm, EvmEnv, EvmFactory, eth::EthEvmContext,
    precompiles::PrecompilesMap,
};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use eez_primitives::{EEZL2_ADDRESS, SYSTEM_ADDRESS, SYSTEM_TX_GAS_CAP, SYSTEM_TX_TYPE};
use eez_protocol::l2::{ProfileError, validate_fee_collector};
use revm::{
    Context, Inspector, SystemCallEvm,
    context::{BlockEnv, CfgEnv, Evm as RevmEvm, TxEnv},
    context_interface::{
        Cfg as _, JournalTr, Transaction as _,
        journaled_state::account::JournaledAccountTr,
        result::{EVMError, HaltReason, ResultAndState},
    },
    handler::{
        EthFrame, FrameResult, Handler, instructions::EthInstructions, post_execution,
        pre_execution,
    },
    inspector::{InspectorHandler, NoOpInspector},
    interpreter::{InitialAndFloorGas, interpreter::EthInterpreter},
    primitives::hardfork::SpecId,
};
use std::marker::PhantomData;

type Inner<DB, I> = RevmEvm<
    EthEvmContext<DB>,
    I,
    EthInstructions<EthInterpreter, EthEvmContext<DB>>,
    PrecompilesMap,
    EthFrame,
>;

pub struct EezEvm<DB: Database, I> {
    inner: Inner<DB, I>,
    inspect: bool,
    // EvmFactory cannot return an error. Retain a failed profile and reject
    // execution; missing L2 configuration never selects L1 rules.
    collector: Result<Option<Address>, ProfileError>,
}

impl<DB: Database, I> std::fmt::Debug for EezEvm<DB, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EezEvm")
            .field("collector", &self.collector)
            .finish_non_exhaustive()
    }
}

struct EezHandler<DB, I> {
    collector: Option<Address>,
    marker: PhantomData<(DB, I)>,
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> Handler for EezHandler<DB, I> {
    type Evm = Inner<DB, I>;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;

    fn validate_against_state_and_deduct_caller(
        &self,
        evm: &mut Self::Evm,
        _initial_gas: &mut InitialAndFloorGas,
    ) -> Result<(), Self::Error> {
        if evm.ctx.tx.tx_type != SYSTEM_TX_TYPE {
            return pre_execution::validate_against_state_and_deduct_caller(&mut evm.ctx);
        }
        let ctx = &mut evm.ctx;
        let mut caller = ctx
            .journaled_state
            .load_account_with_code_mut(SYSTEM_ADDRESS)?
            .data;
        pre_execution::validate_account_nonce_and_code(
            &caller.account().info,
            ctx.tx.nonce,
            ctx.cfg.is_eip3607_disabled(),
            true,
        )?;
        let balance =
            pre_execution::calculate_caller_fee(*caller.balance(), &ctx.tx, &ctx.block, &ctx.cfg)?;
        caller.set_balance(balance);
        // A temporary bump changes account emptiness and CALL/SELFDESTRUCT gas
        // during execution. There is no native nonce transition to roll back.
        Ok(())
    }

    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        result: &mut FrameResult,
    ) -> Result<(), Self::Error> {
        post_execution::reward_beneficiary(&mut evm.ctx, result.gas())?;
        let Some(collector) = self.collector else {
            return Ok(());
        };
        if evm.ctx.tx.tx_type == SYSTEM_TX_TYPE || evm.ctx.cfg.is_fee_charge_disabled() {
            return Ok(());
        }
        // Retain the original account in the transaction state. Credit is
        // applied after finalization so SELFDESTRUCT cannot erase the base fee.
        if evm.ctx.block.basefee != 0
            && evm.ctx.tx.effective_gas_price(evm.ctx.block.basefee.into()) != 0
        {
            evm.ctx.journaled_state.load_account_mut(collector)?;
        }
        Ok(())
    }
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> InspectorHandler for EezHandler<DB, I> {
    type IT = EthInterpreter;
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> EezEvm<DB, I> {
    /// Execute a synthetic destination call whose value arrives from outside
    /// this frame. The manager forwards that value through the caller proxy in
    /// canonical execution; a direct-call simulation must preserve its existing
    /// balance and any value returned during execution.
    ///
    /// This entry point requires fee charging to be disabled and rejects native
    /// transactions and creation. The result is provisional; the caller owns
    /// committing effects and restoring the synthetic transaction sender nonce.
    /// Failed calls remove the temporary credit. Errors leave no journal credit.
    pub fn transact_funded_call(
        &mut self,
        tx: TxEnv,
    ) -> Result<ResultAndState, EVMError<DB::Error>> {
        if !self.inner.ctx.cfg.is_fee_charge_disabled()
            || tx.tx_type != 0
            || tx.caller == SYSTEM_ADDRESS
            || !matches!(tx.kind, TxKind::Call(_))
        {
            return Err(EVMError::Custom("invalid funded simulation call".into()));
        }
        self.execute_with_value_credit(tx)
    }

    fn execute_with_value_credit(
        &mut self,
        tx: TxEnv,
    ) -> Result<ResultAndState, EVMError<DB::Error>> {
        self.collector
            .map_err(|error| EVMError::Custom(error.to_string()))?;
        if tx.value.is_zero() {
            return self.execute(tx);
        }
        let caller = tx.caller;
        let journal = &mut self.inner.ctx.journaled_state;
        let funding = journal
            .load_account_with_code_mut(caller)
            .map_err(EVMError::Database)
            .and_then(|mut account| {
                let balance = *account.balance();
                let funded = balance
                    .checked_add(tx.value)
                    .ok_or_else(|| EVMError::Custom("call value credit balance overflow".into()))?;
                account.set_balance(funded);
                Ok(balance)
            });
        let previous_balance = match funding {
            Ok(balance) => balance,
            Err(error) => {
                journal.finalize();
                return Err(error);
            }
        };
        let mut output = self.execute(tx)?;
        if !output.result.is_success() {
            output
                .state
                .get_mut(&caller)
                // Caller loading is mandatory before any executed call or halt.
                .expect("executed funded call has a caller account")
                .info
                .balance = previous_balance;
        }
        Ok(output)
    }

    fn execute(&mut self, tx: TxEnv) -> Result<ResultAndState, EVMError<DB::Error>> {
        let collector = self
            .collector
            .map_err(|error| EVMError::Custom(error.to_string()))?;
        // Canonical transactions pass base-fee admission, so this is basefee.
        // Advisory calls may relax that admission and pay less; they cannot
        // distribute fees the sender did not pay.
        let credit = collector
            .filter(|_| {
                tx.tx_type != SYSTEM_TX_TYPE && !self.inner.ctx.cfg.is_fee_charge_disabled()
            })
            .map(|address| {
                (
                    address,
                    tx.effective_gas_price(self.inner.ctx.block.basefee.into())
                        .min(self.inner.ctx.block.basefee.into()),
                )
            });
        self.inner.ctx.tx = tx;
        let mut handler = EezHandler {
            collector,
            marker: PhantomData,
        };
        let result = if self.inspect {
            handler.inspect_run(&mut self.inner)
        } else {
            handler.run(&mut self.inner)
        };
        // Match ExecuteEvm: finalize even on errors, so failed validation/DB
        // access cannot leak loaded or minted state into the next transaction.
        let mut state = self.inner.ctx.journaled_state.finalize();
        let result = result?;
        if let Some((collector, price)) = credit.filter(|(_, price)| *price != 0) {
            let amount = U256::from(price)
                .checked_mul(U256::from(result.tx_gas_used()))
                .ok_or_else(|| EVMError::Custom("L2 base fee overflow".into()))?;
            let account = state.get_mut(&collector).ok_or_else(|| {
                EVMError::Custom("fee collector absent after successful fee settlement".into())
            })?;
            if account.is_selfdestructed() {
                // Execution deleted this account. Recreate only the fee credit,
                // preserving neither code, nonce nor storage from the deletion.
                // Created makes both CacheDB and the block State clear old storage.
                account.info = Default::default();
                account.storage.clear();
                account.unmark_selfdestruct();
                account.unmark_selfdestructed_locally();
                account.mark_created();
            }
            account.info.balance = account
                .info
                .balance
                .checked_add(amount)
                .ok_or_else(|| EVMError::Custom("L2 fee collector balance overflow".into()))?;
            account.mark_touch();
        }
        Ok(ResultAndState { result, state })
    }
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> Evm for EezEvm<DB, I> {
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        &self.inner.ctx.block
    }
    fn cfg_env(&self) -> &CfgEnv {
        &self.inner.ctx.cfg
    }
    fn chain_id(&self) -> u64 {
        self.inner.ctx.cfg.chain_id
    }

    /// Mint committed native value before validation and transfer. Failed calls
    /// remove only this mint, preserving pooled funds and metered receipts.
    fn transact_raw(&mut self, tx: TxEnv) -> Result<ResultAndState, Self::Error> {
        let collector = self
            .collector
            .map_err(|error| EVMError::Custom(error.to_string()))?;
        if tx.tx_type != SYSTEM_TX_TYPE {
            if tx.caller == SYSTEM_ADDRESS {
                return Err(EVMError::Custom(
                    "reserved system sender requires the native transaction type".into(),
                ));
            }
            return self.execute(tx);
        }
        // Direct TxEnv callers bypass envelope conversion. Block/semantic
        // validation additionally checks position gas and source authority.
        if collector.is_none()
            || tx.caller != SYSTEM_ADDRESS
            || (tx.kind != TxKind::Call(EEZL2_ADDRESS)
                && !(tx.kind == TxKind::Call(eez_primitives::HISTORY_ADDRESS)
                    && tx.value.is_zero()))
            || tx.gas_limit > SYSTEM_TX_GAS_CAP
            || (tx.kind == TxKind::Call(eez_primitives::HISTORY_ADDRESS)
                && tx.gas_limit > eez_primitives::HISTORY_TX_GAS_CAP)
            || tx.nonce != 0
            || tx.gas_price != 0
            || tx.chain_id.is_none()
            || tx.gas_priority_fee.is_some()
            || !tx.access_list.0.is_empty()
            || !tx.blob_hashes.is_empty()
            || tx.max_fee_per_blob_gas != 0
            || !tx.authorization_list.is_empty()
        {
            return Err(EVMError::Custom(
                "invalid native system transaction environment".into(),
            ));
        }
        self.execute_with_value_credit(tx)
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        self.collector
            .map_err(|error| EVMError::Custom(error.to_string()))?;
        if contract == eez_primitives::HISTORY_ADDRESS {
            return Err(EVMError::Custom(
                "history updates require a recorded native transaction".into(),
            ));
        }
        // Ethereum protocol calls, including EIP-2935, retain their own semantics.
        self.inner.system_call_with_caller(caller, contract, data)
    }

    fn finish(self) -> (DB, EvmEnv) {
        let Context {
            block: block_env,
            cfg: cfg_env,
            journaled_state,
            ..
        } = self.inner.ctx;
        (journaled_state.database, EvmEnv { block_env, cfg_env })
    }
    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inspect = enabled;
    }
    fn components(&self) -> (&DB, &I, &PrecompilesMap) {
        (
            &self.inner.ctx.journaled_state.database,
            &self.inner.inspector,
            &self.inner.precompiles,
        )
    }
    fn components_mut(&mut self) -> (&mut DB, &mut I, &mut PrecompilesMap) {
        (
            &mut self.inner.ctx.journaled_state.database,
            &mut self.inner.inspector,
            &mut self.inner.precompiles,
        )
    }
}

/// Selects authenticated L2 fee rules or explicit L1 simulation semantics.
#[derive(Debug, Clone, Copy)]
pub struct EezEvmFactory {
    pub(crate) collector: Result<Option<Address>, ProfileError>,
}

impl EezEvmFactory {
    /// Construct L2 execution from the genesis-selected collector.
    pub fn for_rollup(collector: Address) -> Result<Self, ProfileError> {
        validate_fee_collector(collector)?;
        Ok(Self {
            collector: Ok(Some(collector)),
        })
    }
    /// L1 source simulation retains Ethereum fee distribution.
    pub const fn for_l1_simulation() -> Self {
        Self {
            collector: Ok(None),
        }
    }
}

impl EvmFactory for EezEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = EezEvm<DB, I>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<DBError: revm::database_interface::DBErrorMarker> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> EezEvm<DB, NoOpInspector> {
        EezEvm {
            inner: EthEvmFactory::default().create_evm(db, input).into_inner(),
            inspect: false,
            collector: self.collector,
        }
    }
    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> EezEvm<DB, I> {
        EezEvm {
            inner: EthEvmFactory::default()
                .create_evm_with_inspector(db, input, inspector)
                .into_inner(),
            inspect: true,
            collector: self.collector,
        }
    }
}

#[cfg(test)]
mod tests;
