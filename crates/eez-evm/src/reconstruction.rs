//! Reconstruct finalized Osaka state through the pinned engine public API.

use alloy_consensus::{Typed2718, transaction::Recovered};
use alloy_primitives::{Address, B256, U256};
use evm2::{
    Evm, EvmFeatures, EvmTypes, ExecutedTx, SpecId, TxResult, TxResultWithState,
    evm::{AccountInfo, DbResult, DynDatabase},
    interpreter::Host,
    registry::{HandlerError, HandlerResult, TxHandler, TxRegistry, TxRequest},
};
use std::sync::Mutex;

fn reject(message: &'static str) -> HandlerError {
    HandlerError::external(std::io::Error::other(message))
}

// Any accidental dependency on carrier-local state must fail, never synthesize absence.
struct RejectReads;
impl DynDatabase for RejectReads {
    fn get_account(&mut self, _: &Address) -> DbResult<Option<AccountInfo>> {
        Err(unexpected_read())
    }
    fn get_code_by_hash(&mut self, _: &B256) -> DbResult<evm2::bytecode::Bytecode> {
        Err(unexpected_read())
    }
    fn get_storage(&mut self, _: &Address, _: &U256) -> DbResult<U256> {
        Err(unexpected_read())
    }
    fn get_block_hash(&mut self, _: &U256) -> DbResult<B256> {
        Err(unexpected_read())
    }
}
fn unexpected_read() -> evm2::DatabaseError {
    evm2::DatabaseError::new(
        std::io::Error::other("unexpected reconstruction database read"),
        true,
    )
}

type PreparedResult<T> = (Recovered<<T as evm2::EvmTypesHost>::Tx>, TxResult<T>);
struct Prepared<T: EvmTypes> {
    value: Mutex<Option<PreparedResult<T>>>,
}
impl<T: EvmTypes<Tx: PartialEq>> TxHandler<T, T::Tx, TxResult<T>> for Prepared<T> {
    fn validate(&self, _: TxRequest<'_, '_, T, T::Tx>) -> HandlerResult<()> {
        self.value
            .lock()
            .map_err(|_| reject("reconstruction poisoned"))?
            .take();
        Err(reject("reconstruction does not admit transactions"))
    }
    fn execute(&self, req: TxRequest<'_, '_, T, T::Tx>) -> HandlerResult<TxResult<T>> {
        // Consume before checking identity. Even a rejected attempt cannot reuse this result.
        let (expected, outcome) = self
            .value
            .lock()
            .map_err(|_| reject("reconstruction poisoned"))?
            .take()
            .ok_or_else(|| reject("reconstruction already consumed"))?;
        if expected.inner() != req.envelope || expected.signer() != req.tx.signer() {
            return Err(reject("reconstruction transaction or signer mismatch"));
        }
        *req.host.state_mut().logs_mut() = outcome.logs.clone();
        Ok(outcome)
    }
}

struct Carrier<T: EvmTypes>(Evm<'static, T>);
impl<T> Carrier<T>
where
    T: EvmTypes<Tx: Typed2718 + Clone + PartialEq + Send + Sync, TxResultExt: Send, EvmExt: Clone>,
{
    fn new(
        original: &mut Evm<'_, T>,
        tx: &Recovered<T::Tx>,
        mut raw: TxResultWithState<T>,
        credit: Option<BaseFeeCredit>,
    ) -> HandlerResult<Self> {
        // Re-finalization is tested only for the accepted Osaka profile. Later lifecycle/BAL
        // rules need independent coverage before this isolated adapter can support them.
        if original.spec_id() != SpecId::OSAKA {
            return Err(reject("reconstruction requires Osaka"));
        }
        let mut correction = None;
        if let Some(credit) = credit {
            let amount = fee_amount(Some(credit), raw.result.tx_gas_used())?;
            if !amount.is_zero() {
                let address = credit.collector;
                let (info, recreate) = match raw.pending_state.account_changes_for(&address) {
                    Some(account) => {
                        let info = account.current.cloned().unwrap_or_default();
                        let recreate = account.selfdestructed;
                        if recreate {
                            if account.current.is_some()
                                || (!account.storage.wiped && account.original.is_some())
                                || account
                                    .storage
                                    .slots
                                    .values()
                                    .any(|slot| !slot.value.current.is_zero())
                            {
                                return Err(reject("unsupported collector deletion"));
                            }
                            let prior = account.original.cloned();
                            raw.pending_state
                                .insert_account(address, prior, Some(info.clone()));
                        }
                        (info, recreate)
                    }
                    None => {
                        let info = original.overlay_db_mut().get_account(&address)?;
                        raw.pending_state
                            .insert_account(address, info.clone(), info.clone());
                        (info.unwrap_or_default(), false)
                    }
                };
                let balance = info
                    .balance
                    .checked_add(amount)
                    .ok_or_else(|| reject("L2 fee collector balance overflow"))?;
                correction = Some((address, balance, recreate));
            }
        }
        let mut carrier = Self::assemble(
            original.execution_config().clone(),
            original.config_spec_id(),
            *original.block(),
            original.ext().clone(),
            tx,
            raw.result,
            raw.pending_state,
        )?;
        if let Some((address, balance, recreate)) = correction {
            if recreate {
                // Fee-only recreation has zero nonce/code. Preserve the deletion's storage wipe
                // and restore the created/discard-old-storage flags through the public state API.
                let mut features = carrier.0.version().features;
                features.remove(EvmFeatures::EIP161);
                carrier
                    .0
                    .state_mut()
                    .create_account(&address, &address, &U256::ZERO, features)?
                    .map_err(|_| reject("collector recreation collision"))?;
            }
            let mut account = carrier.0.state_mut().account(&address)?;
            account.set_balance(balance);
            account.touch();
        }
        Ok(carrier)
    }
    fn assemble(
        config: evm2::ExecutionConfig<T>,
        spec: T::SpecId,
        block: evm2::env::BlockEnv<T>,
        ext: T::EvmExt,
        tx: &Recovered<T::Tx>,
        result: TxResult<T>,
        pending: evm2::PendingState,
    ) -> HandlerResult<Self> {
        if spec.into() != SpecId::OSAKA {
            return Err(reject("reconstruction requires Osaka"));
        }
        let registry = TxRegistry::new().with_handler(
            tx.inner().ty(),
            |tx: &T::Tx| Some(tx),
            Prepared {
                value: Mutex::new(Some((tx.clone(), result))),
            },
        );
        // At pinned evm2 e681662, Osaka finalization reads State and Version, not TxEnv
        // or precompiles. The private handler executes no bytecode or ordinary preparation.
        let mut evm = Evm::new_with_execution_config_and_ext(
            config,
            spec,
            block,
            registry,
            RejectReads,
            evm2::Precompiles::base(SpecId::OSAKA),
            ext,
        );
        evm.state_mut().set_pending_state(pending);
        Ok(Self(evm))
    }
    fn detach(mut self, tx: &Recovered<T::Tx>) -> HandlerResult<TxResultWithState<T>> {
        Ok(self.0.transact(tx)?.detach())
    }
    fn observe<R>(
        mut self,
        tx: &Recovered<T::Tx>,
        observe: impl FnOnce(&ExecutedTx<'_, '_, T>) -> R,
    ) -> HandlerResult<R> {
        let executed = self.0.transact(tx)?;
        let observed = observe(&executed);
        drop(executed);
        Ok(observed)
    }
}

pub(crate) fn fee_amount(credit: Option<BaseFeeCredit>, gas_used: u64) -> HandlerResult<U256> {
    credit.map_or(Ok(U256::ZERO), |credit| {
        credit
            .charged_base_fee_per_gas
            .checked_mul(U256::from(gas_used))
            .ok_or_else(|| reject("L2 base fee overflow"))
    })
}

pub(crate) fn finalize<T>(
    original: &mut Evm<'_, T>,
    tx: &Recovered<T::Tx>,
    raw: TxResultWithState<T>,
    credit: Option<BaseFeeCredit>,
) -> HandlerResult<TxResultWithState<T>>
where
    T: EvmTypes<Tx: Typed2718 + Clone + PartialEq + Send + Sync, TxResultExt: Send, EvmExt: Clone>,
{
    if fee_amount(credit, raw.result.tx_gas_used())?.is_zero() {
        return Ok(raw);
    }
    Carrier::new(original, tx, raw, credit)?.detach(tx)
}
pub(crate) fn observe<T, R>(
    original: &mut Evm<'_, T>,
    tx: &Recovered<T::Tx>,
    raw: TxResultWithState<T>,
    credit: Option<BaseFeeCredit>,
    observe: impl FnOnce(&ExecutedTx<'_, '_, T>) -> R,
) -> HandlerResult<R>
where
    T: EvmTypes<Tx: Typed2718 + Clone + PartialEq + Send + Sync, TxResultExt: Send, EvmExt: Clone>,
{
    debug_assert!(!fee_amount(credit, raw.result.tx_gas_used())?.is_zero());
    Carrier::new(original, tx, raw, credit)?.observe(tx, observe)
}

#[derive(Clone, Copy)]
pub(crate) struct BaseFeeCredit {
    pub collector: Address,
    pub charged_base_fee_per_gas: U256,
}

/// Fixed Osaka cleanup for one finalized transaction (or a fresh terminal delta), never a
/// merged multi-transaction overlay. On error the caller must discard the complete result.
pub(crate) fn clear_protocol_balances(
    db: &mut dyn DynDatabase,
    pending: &mut evm2::PendingState,
) -> DbResult<()> {
    (|| -> HandlerResult<()> {
        let mut loaded = Vec::with_capacity(2);
        let mut unloaded = Vec::with_capacity(2);
        for address in crate::balances::PROTOCOL_BALANCE_ACCOUNTS {
            match pending.account_changes_for(&address) {
                Some(account) => {
                    if account.current.is_some_and(|info| !info.balance.is_zero()) {
                        loaded.push(address);
                    }
                }
                None => {
                    if let Some(original) = db.get_account(&address)?
                        && !original.balance.is_zero()
                    {
                        let mut current = original.clone();
                        current.balance = U256::ZERO;
                        unloaded.push((
                            address,
                            original,
                            (!current.is_empty()).then_some(current),
                        ));
                    }
                }
            }
        }
        if !loaded.is_empty() {
            // This unexposed envelope only returns the attached pending state. It never goes
            // through admission/preparation and its synthetic result is never used as a receipt.
            let tx = Recovered::new_unchecked(
                evm2::ethereum::TxEnvelope::Legacy(Default::default()),
                Address::ZERO,
            );
            let mut carrier = Carrier::<evm2::BaseEvmTypes>::assemble(
                evm2::ExecutionConfig::for_spec_and_version(
                    SpecId::OSAKA,
                    *evm2::Version::base(SpecId::OSAKA),
                ),
                SpecId::OSAKA,
                Default::default(),
                (),
                &tx,
                Default::default(),
                std::mem::take(pending),
            )?;
            for address in loaded {
                let mut account = carrier.0.state_mut().account(&address)?;
                account.set_balance(U256::ZERO);
                account.touch();
            }
            *pending = carrier.detach(&tx)?.pending_state;
        }
        // Unloaded accounts have no transaction-lifetime metadata: keep the previous insertion
        // behavior, including no entries for absent/already-zero accounts, after re-finalization.
        for (address, original, current) in unloaded {
            pending.insert_account(address, Some(original), current);
        }
        Ok(())
    })()
    .map_err(|error| match error {
        HandlerError::Database(error) => error,
        other => evm2::DatabaseError::new(other, true),
    })
}

#[cfg(test)]
mod tests;
