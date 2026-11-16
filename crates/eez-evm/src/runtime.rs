//! RollupX fee and system-call boundary over the unmodified engine.
use crate::{
    EezEvmTx, EezEvmTypes,
    reconstruction::{self as carrier, BaseFeeCredit},
};
use alloy_consensus::{Transaction as _, transaction::Recovered};
use alloy_primitives::{Address, U256};
use evm2::Evm as Raw;
use evm2::block::map_handler_error;
use evm2::registry::{HandlerError, HandlerResult};
use reth_evm::{BlockExecutionError, Evm as RethEvm};

/// RollupX execution boundary: ordinary L2 fees are credited after engine finalization.
/// Provisional results are owned; only the caller can commit them.
#[derive(Debug)]
#[repr(transparent)]
pub struct EezEvm<'a>(Raw<'a, EezEvmTypes>);
const _: () = {
    assert!(
        std::mem::size_of::<EezEvm<'static>>() == std::mem::size_of::<Raw<'static, EezEvmTypes>>()
    );
    assert!(
        std::mem::align_of::<EezEvm<'static>>()
            == std::mem::align_of::<Raw<'static, EezEvmTypes>>()
    );
};

impl<'a> EezEvm<'a> {
    pub(crate) fn new(raw: Raw<'a, EezEvmTypes>) -> Self {
        Self(raw)
    }
    pub(crate) fn from_ref<'r>(raw: &'r Raw<'a, EezEvmTypes>) -> &'r Self {
        // SAFETY: repr(transparent), exactly the same sole field and both unchanged lifetimes.
        // This view imposes no additional memory-validity invariant and owns no extra resources.
        unsafe { &*std::ptr::from_ref(raw).cast::<Self>() }
    }
    pub(crate) fn from_mut<'r>(raw: &'r mut Raw<'a, EezEvmTypes>) -> &'r mut Self {
        // SAFETY: transparent single-field layout preserves alignment/validity. The exclusive
        // input borrow is reborrowed for exactly 'r; neither stored-object lifetime nor ownership changes.
        unsafe { &mut *std::ptr::from_mut(raw).cast::<Self>() }
    }
    pub(crate) fn credit(&self, tx: &Recovered<EezEvmTx>) -> HandlerResult<Option<BaseFeeCredit>> {
        let collector = self.0.ext().collector.map_err(HandlerError::external)?;
        let Some(collector) = collector else {
            return Ok(None);
        };
        if !matches!(tx.inner(), EezEvmTx::Ethereum(_))
            || !self.0.feature(evm2::EvmFeatures::FEE_CHARGE)
        {
            return Ok(None);
        }
        let basefee = self.0.block().basefee;
        let price = match tx.inner().gas_price() {
            Some(price) => U256::from(price),
            None => evm2::ethereum::effective_gas_price(
                U256::from(tx.inner().max_fee_per_gas()),
                U256::from(tx.inner().max_priority_fee_per_gas().unwrap_or_default()),
                basefee,
            ),
        };
        Ok(Some(BaseFeeCredit {
            collector,
            charged_base_fee_per_gas: basefee.min(price),
        }))
    }
    pub(crate) fn into_inner(self) -> Raw<'a, EezEvmTypes> {
        self.0
    }

    pub fn transact(
        &mut self,
        tx: &Recovered<EezEvmTx>,
    ) -> HandlerResult<evm2::TxResultWithState<EezEvmTypes>> {
        let credit = self.credit(tx)?;
        let raw = self.0.transact(tx)?.detach();
        carrier::finalize(&mut self.0, tx, raw, credit)
    }
    pub fn transact_with_inspector(
        &mut self,
        tx: &Recovered<EezEvmTx>,
        inspector: &mut dyn evm2::Inspector<EezEvmTypes>,
    ) -> HandlerResult<evm2::TxResultWithState<EezEvmTypes>> {
        let credit = self.credit(tx)?;
        let raw = self.0.transact_with_inspector(tx, inspector)?.detach();
        carrier::finalize(&mut self.0, tx, raw, credit)
    }
    pub fn system_call(
        &mut self,
        tx: evm2::evm::SystemTx,
    ) -> HandlerResult<evm2::ExecutedTx<'_, 'a, EezEvmTypes>> {
        self.0.ext().collector.map_err(HandlerError::external)?;
        if tx.system_contract_address == eez_primitives::HISTORY_ADDRESS {
            return Err(HandlerError::external(eez_protocol::l2::ProfileError(
                "history updates require a recorded native transaction",
            )));
        }
        self.0.system_call(tx)
    }
    pub fn block(&self) -> &evm2::env::BlockEnv<EezEvmTypes> {
        self.0.block()
    }
    pub fn spec_id(&self) -> evm2::SpecId {
        self.0.spec_id()
    }
    pub fn state(&self) -> &evm2::evm::State<'a> {
        self.0.state()
    }
    pub fn state_mut(&mut self) -> &mut evm2::evm::State<'a> {
        self.0.state_mut()
    }
    pub fn overlay_db(&self) -> &evm2::evm::CacheDB<Box<dyn evm2::evm::DynDatabase + 'a>> {
        self.0.overlay_db()
    }
    pub fn overlay_db_mut(
        &mut self,
    ) -> &mut evm2::evm::CacheDB<Box<dyn evm2::evm::DynDatabase + 'a>> {
        self.0.overlay_db_mut()
    }
    pub fn inspector(&self) -> Option<&(dyn evm2::Inspector<EezEvmTypes> + 'a)> {
        self.0.inspector()
    }
    pub fn set_inspector<I: evm2::Inspector<EezEvmTypes> + 'a>(&mut self, inspector: I) {
        self.0.set_inspector(inspector);
    }
    pub fn clear_inspector(&mut self) -> Option<Box<dyn evm2::Inspector<EezEvmTypes> + 'a>> {
        self.0.clear_inspector()
    }
}
impl RethEvm for EezEvm<'_> {
    type EvmTypes = EezEvmTypes;
    type Transaction = EezEvmTx;
    fn transact(
        &mut self,
        tx: &Recovered<EezEvmTx>,
    ) -> Result<evm2::TxResultWithState<EezEvmTypes>, BlockExecutionError> {
        let credit = self.credit(tx).map_err(map_handler_error)?;
        let raw = RethEvm::transact(&mut self.0, tx)?;
        carrier::finalize(&mut self.0, tx, raw, credit).map_err(map_handler_error)
    }
    fn transact_commit(
        &mut self,
        tx: &Recovered<EezEvmTx>,
    ) -> Result<evm2::TxResult<EezEvmTypes>, BlockExecutionError> {
        let output = EezEvm::transact(self, tx).map_err(map_handler_error)?;
        self.commit_state(&output.pending_state);
        Ok(output.result)
    }
    fn transact_result(
        &mut self,
        tx: &Recovered<EezEvmTx>,
    ) -> Result<evm2::TxResult<EezEvmTypes>, BlockExecutionError> {
        Ok(EezEvm::transact(self, tx)
            .map_err(map_handler_error)?
            .result)
    }
    fn transact_with_inspector<I: evm2::Inspector<EezEvmTypes> + 'static>(
        &mut self,
        tx: &Recovered<EezEvmTx>,
        inspector: I,
    ) -> Result<(I, evm2::TxResultWithState<EezEvmTypes>), BlockExecutionError> {
        let credit = self.credit(tx).map_err(map_handler_error)?;
        let (inspector, raw) = RethEvm::transact_with_inspector(&mut self.0, tx, inspector)?;
        Ok((
            inspector,
            carrier::finalize(&mut self.0, tx, raw, credit).map_err(map_handler_error)?,
        ))
    }
    fn set_inspector<I: evm2::Inspector<EezEvmTypes> + 'static>(&mut self, inspector: I) {
        RethEvm::set_inspector(&mut self.0, inspector);
    }
    unsafe fn with_inspector<I: evm2::Inspector<EezEvmTypes> + 'static, R>(
        &mut self,
        f: impl FnOnce(&mut I) -> R,
    ) -> Option<R> {
        // SAFETY: the caller supplies Reth's identical no-lifetime-parameters contract for I;
        // forwarding preserves the inspector type, borrow and callback unchanged.
        unsafe { RethEvm::with_inspector(&mut self.0, f) }
    }
    fn take_inspector(&mut self) -> Option<Box<dyn evm2::Inspector<EezEvmTypes> + '_>> {
        RethEvm::take_inspector(&mut self.0)
    }
    fn state_db_mut(&mut self) -> &mut dyn evm2::evm::DynDatabase {
        RethEvm::state_db_mut(&mut self.0)
    }
    fn commit_state(&mut self, state: &evm2::PendingState) {
        RethEvm::commit_state(&mut self.0, state);
    }
    fn take_state_cache(&mut self) -> evm2::evm::Cache {
        RethEvm::take_state_cache(&mut self.0)
    }
    fn precompile_ids(&self) -> Vec<(Address, evm2::precompiles::PrecompileId)> {
        RethEvm::precompile_ids(&self.0)
    }
    fn has_precompile(&self, address: &Address) -> bool {
        RethEvm::has_precompile(&self.0, address)
    }
    fn account_info(
        &mut self,
        address: &Address,
    ) -> Result<Option<evm2::evm::AccountInfo>, BlockExecutionError> {
        RethEvm::account_info(&mut self.0, address)
    }
    fn move_precompiles(
        &mut self,
        moves: impl IntoIterator<Item = (Address, Address)>,
    ) -> Result<(), evm2::precompiles::MovePrecompileError> {
        RethEvm::move_precompiles(&mut self.0, moves)
    }
    fn transact_and_discard<R>(
        &mut self,
        tx: &Recovered<EezEvmTx>,
        observe: impl FnOnce(&evm2::ExecutedTx<'_, '_, EezEvmTypes>) -> R,
    ) -> Result<R, BlockExecutionError> {
        let credit = self.credit(tx).map_err(map_handler_error)?;
        let executed = self.0.transact(tx).map_err(map_handler_error)?;
        if carrier::fee_amount(credit, executed.result().tx_gas_used())
            .map_err(map_handler_error)?
            .is_zero()
        {
            let result = observe(&executed);
            drop(executed);
            return Ok(result);
        }
        let raw = executed.detach();
        carrier::observe(&mut self.0, tx, raw, credit, observe).map_err(map_handler_error)
    }
}
