//! Candidate RollupX transaction handlers; not yet wired into shipping Reth.
//! Wire envelopes and profile validation come from the frozen parent crates.
use alloy_consensus::{Transaction, TxLegacy, Typed2718, transaction::Recovered};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Address, TxKind, U256};
use alloy_primitives::{B256, Bytes};
use eez_primitives::{
    EEZL2_ADDRESS, EezTxEnvelope, HISTORY_ADDRESS, HISTORY_TX_GAS_CAP, HISTORY_TX_TYPE,
    SYSTEM_ADDRESS, SYSTEM_TX_GAS_CAP, SYSTEM_TX_TYPE,
};
use eez_protocol::l2::{ProfileError, validate_fee_collector};
use evm2::{
    BaseEvmConfigSelector, Evm, EvmFeatures, EvmTypesHost, SpecId, TxResult,
    env::TxEnvExt,
    ethereum::{self as eth, TxEnvelope, eip1559, eip2930, eip4844, eip7702, legacy},
    handler::{GasSettlement, TxHandlerHooks},
    interpreter::GasTracker,
    registry::{HandlerError, HandlerResult, TxRegistry, TxRequest, handler},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EezEvmTx {
    Ethereum(TxEnvelope),
    /// Derived execution fields only; source identity and history authority remain block checks.
    Native {
        ty: u8,
        call: TxLegacy,
        access_list: AccessList,
    },
    /// Request-local destination simulation with external value and fee charging disabled.
    Funded(TxLegacy),
    /// Preserve request metadata while retaining the baseline legacy simulation rules:
    /// REVM ignores access lists for type-0 funded calls; they do not change gas or warming.
    FundedAccessList {
        call: TxLegacy,
        access_list: AccessList,
    },
}
impl Typed2718 for EezEvmTx {
    fn ty(&self) -> u8 {
        match self {
            Self::Ethereum(tx) => tx.ty(),
            Self::Native { ty, .. } => *ty,
            Self::Funded(_) | Self::FundedAccessList { .. } => 0,
        }
    }
}
impl EezEvmTx {
    fn ethereum(&self) -> Option<&TxEnvelope> {
        if let Self::Ethereum(tx) = self {
            Some(tx)
        } else {
            None
        }
    }
    fn legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Ethereum(tx) => tx.as_legacy(),
            Self::Funded(tx) | Self::FundedAccessList { call: tx, .. } => Some(tx),
            _ => None,
        }
    }
    fn native(&self) -> Option<&TxLegacy> {
        if let Self::Native { call, .. } = self {
            Some(call)
        } else {
            None
        }
    }
}
impl From<EezTxEnvelope> for EezEvmTx {
    fn from(tx: EezTxEnvelope) -> Self {
        match tx {
            EezTxEnvelope::Ethereum(tx) => Self::Ethereum(tx.into()),
            EezTxEnvelope::System(tx) => Self::Native {
                ty: SYSTEM_TX_TYPE,
                access_list: Default::default(),
                call: TxLegacy {
                    chain_id: Some(tx.chain_id),
                    gas_limit: tx.gas_limit,
                    to: TxKind::Call(tx.to),
                    value: tx.value,
                    input: tx.input.clone(),
                    ..Default::default()
                },
            },
            EezTxEnvelope::History(tx) => Self::Native {
                ty: HISTORY_TX_TYPE,
                access_list: Default::default(),
                call: TxLegacy {
                    chain_id: Some(tx.chain_id),
                    gas_limit: HISTORY_TX_GAS_CAP,
                    to: TxKind::Call(HISTORY_ADDRESS),
                    // The block adapter must install independently validated history calldata/budget.
                    ..Default::default()
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EezEvmExt {
    pub(crate) collector: Result<Option<Address>, ProfileError>,
}
impl Default for EezEvmExt {
    fn default() -> Self {
        Self {
            collector: Err(ProfileError("execution profile is not configured")),
        }
    }
}
impl EezEvmExt {
    pub fn for_rollup(collector: Address) -> Result<Self, ProfileError> {
        validate_fee_collector(collector)?;
        Ok(Self {
            collector: Ok(Some(collector)),
        })
    }
    pub const fn for_l1_simulation() -> Self {
        Self {
            collector: Ok(None),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EezEvmTypes;
impl EvmTypesHost for EezEvmTypes {
    type ConfigSelector = BaseEvmConfigSelector;
    type SpecId = SpecId;
    type Tx = EezEvmTx;
    type EvmExt = EezEvmExt;
    type MessageExt = ();
    type MessageResultExt = ();
    type TxEnvExt = ();
    type TxResultExt = ();
    type BlockEnvExt = ();
    type Host<'a> = Evm<'a, Self>;
}

/// Match REVM's uncharged-call balance semantics without minting a synthetic gas balance.
/// Native nonce/code and per-call validation switches remain the caller's responsibility.
pub fn disable_fee_charging(version: &mut evm2::Version) {
    version
        .features
        .remove(EvmFeatures::FEE_CHARGE | EvmFeatures::BALANCE_CHECK | EvmFeatures::BALANCE_TOP_UP);
}

#[derive(Debug)]
struct ExecutionError(&'static str);
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for ExecutionError {}
fn reject(message: &'static str) -> HandlerError {
    HandlerError::external(ExecutionError(message))
}

struct Hooks;
impl TxHandlerHooks<EezEvmTypes> for Hooks {
    fn before_execution(
        host: &mut Evm<'_, EezEvmTypes>,
        tx: &EezEvmTx,
        caller: Address,
        upfront: U256,
    ) -> HandlerResult<()> {
        host.ext().collector.map_err(HandlerError::external)?;
        if caller == SYSTEM_ADDRESS {
            return Err(reject("reserved system sender requires native type"));
        }
        if host.feature(EvmFeatures::FEE_CHARGE)
            && !host.feature(EvmFeatures::BALANCE_CHECK)
            && host.feature(EvmFeatures::BALANCE_TOP_UP)
            && let Some(tx) = tx.ethereum()
        {
            // REVM floors the balance after the effective fee, not the maximum fee cap.
            // Restore that simulation rule from the transaction's original balance: evm2's
            // sender validation has already topped it up against the maximum fee cap.
            let mut account = host.state_mut().account(&caller)?;
            let original = account.original().map_or(U256::ZERO, |info| info.balance);
            account.set_balance(original.saturating_sub(upfront).max(tx.value()));
            return Ok(());
        }
        eth::charge_upfront(host, caller, upfront)
    }
}

pub fn registry(spec: SpecId) -> TxRegistry<EezEvmTypes, TxResult<EezEvmTypes>> {
    let mut registry = TxRegistry::new()
        .with_handler(0, EezEvmTx::legacy, handler(prepare_legacy, execute_legacy))
        .with_handler(
            SYSTEM_TX_TYPE,
            EezEvmTx::native,
            handler(prepare_native, execute_native),
        )
        .with_handler(
            HISTORY_TX_TYPE,
            EezEvmTx::native,
            handler(prepare_native, execute_native),
        );
    macro_rules! register {
        ($fork:ident, $ty:literal, $access:ident, $module:ident) => {
            if spec.enables(SpecId::$fork) {
                registry.register(
                    $ty,
                    |tx: &EezEvmTx| tx.ethereum().and_then(TxEnvelope::$access),
                    handler(
                        $module::prepare_with_hooks::<EezEvmTypes, Hooks>,
                        $module::execute_prepared::<EezEvmTypes, Hooks>,
                    ),
                );
            }
        };
    }
    register!(BERLIN, 1, as_eip2930, eip2930);
    register!(LONDON, 2, as_eip1559, eip1559);
    register!(CANCUN, 3, as_eip4844, eip4844);
    register!(PRAGUE, 4, as_eip7702, eip7702);
    registry
}

struct PreparedLegacy {
    inner: eth::PreparedTx,
    funded_balance: Option<U256>,
}
fn prepare_legacy(
    req: &mut TxRequest<'_, '_, EezEvmTypes, TxLegacy>,
) -> HandlerResult<PreparedLegacy> {
    let funded_balance = if matches!(
        req.envelope,
        EezEvmTx::Funded(_) | EezEvmTx::FundedAccessList { .. }
    ) {
        let caller = req.tx.signer();
        if req.host.feature(EvmFeatures::FEE_CHARGE)
            || caller == SYSTEM_ADDRESS
            || !matches!(req.tx.inner().to, TxKind::Call(_))
        {
            return Err(reject("invalid funded simulation call"));
        }
        Some(credit_value(req.host, caller, req.tx.inner().value)?)
    } else {
        None
    };
    Ok(PreparedLegacy {
        inner: legacy::prepare_with_hooks::<EezEvmTypes, Hooks>(req)?,
        funded_balance,
    })
}
fn execute_legacy(
    req: TxRequest<'_, '_, EezEvmTypes, TxLegacy>,
    prepared: PreparedLegacy,
) -> HandlerResult<TxResult<EezEvmTypes>> {
    let caller = req.tx.signer();
    let value = req.tx.inner().value;
    // The execution borrows the host, so keep it available for rollback of external credit.
    let host = req.host;
    let result = legacy::execute_prepared::<EezEvmTypes, Hooks>(
        TxRequest {
            host: &mut *host,
            tx: req.tx,
            envelope: req.envelope,
            _non_exhaustive: (),
        },
        prepared.inner,
    )?;
    if let Some(balance) = prepared.funded_balance
        && !result.is_success()
        && !value.is_zero()
    {
        host.state_mut().account(&caller)?.set_balance(balance);
    }
    Ok(result)
}
fn credit_value(
    host: &mut Evm<'_, EezEvmTypes>,
    caller: Address,
    value: U256,
) -> HandlerResult<U256> {
    let mut account = host.state_mut().account(&caller)?;
    let previous = account.balance();
    if !value.is_zero() {
        let balance = previous
            .checked_add(value)
            .ok_or_else(|| reject("call value credit balance overflow"))?;
        account.set_balance(balance);
    }
    Ok(previous)
}

struct PreparedNative {
    intrinsic: u64,
    floor: u64,
    previous_balance: U256,
}
fn prepare_native(
    req: &mut TxRequest<'_, '_, EezEvmTypes, TxLegacy>,
) -> HandlerResult<PreparedNative> {
    let caller = req.tx.signer();
    let tx = req.tx.inner();
    let ty = req.envelope.ty();
    let collector = req.host.ext().collector.map_err(HandlerError::external)?;
    if req
        .envelope
        .access_list()
        .is_some_and(|list| !list.0.is_empty())
        || collector.is_none()
        || caller != SYSTEM_ADDRESS
        || tx.chain_id.is_none()
        || tx.nonce != 0
        || tx.gas_price != 0
        || tx.gas_limit > SYSTEM_TX_GAS_CAP
        || (ty == SYSTEM_TX_TYPE && tx.to != TxKind::Call(EEZL2_ADDRESS))
        || (ty == HISTORY_TX_TYPE
            && (tx.to != TxKind::Call(HISTORY_ADDRESS)
                || !tx.value.is_zero()
                || tx.gas_limit > HISTORY_TX_GAS_CAP))
    {
        return Err(reject("invalid native system transaction environment"));
    }
    let version = req.host.version();
    eth::validate_chain_id(version, tx.chain_id, true)?;
    eth::validate_tx_gas_limit_cap(version, tx.gas_limit)?;
    eth::validate_block_gas_limit(version, tx.gas_limit, req.host.block().gas_limit)?;
    let (intrinsic, floor) =
        eth::intrinsic_and_floor_gas(version, caller, tx.to, &tx.input, 0, 0, tx.value);
    eth::validate_intrinsic_gas(tx.gas_limit, intrinsic, 0)?;
    eth::validate_floor_gas(tx.gas_limit, floor)?;
    eth::validate_execution_gas_limit_cap(version, tx.gas_limit, intrinsic, floor)?;
    let previous_balance = credit_value(req.host, caller, tx.value)?;
    let check_code = req.host.feature(EvmFeatures::EIP3607);
    {
        let mut account = req.host.state_mut().account(&caller)?;
        if check_code && !account.is_empty_code_hash() {
            let code = account.load_code()?;
            if !code.is_empty() && !code.is_eip7702() {
                return Err(HandlerError::RejectCallerWithCode);
            }
        }
        // Match the native caller touch without an ordinary nonce check or bump.
        account.touch();
        account.warm();
    }
    eth::warm_base_accounts(req.host, tx.to);
    Ok(PreparedNative {
        intrinsic,
        floor,
        previous_balance,
    })
}
fn execute_native(
    req: TxRequest<'_, '_, EezEvmTypes, TxLegacy>,
    prepared: PreparedNative,
) -> HandlerResult<TxResult<EezEvmTypes>> {
    let caller = req.tx.signer();
    let tx = req.tx.inner();
    let (gas_limit, reservoir) =
        eth::initial_gas_and_reservoir(req.host.version(), tx.gas_limit, prepared.intrinsic, 0);
    *req.host.tx_env_mut() = TxEnvExt {
        origin: caller,
        gas_price: U256::ZERO,
        chain_id: U256::from(req.host.version().chain_id),
        ..Default::default()
    };
    let mut gas = GasTracker::new_with_execution_gas_and_reservoir(gas_limit, reservoir);
    // Native transactions can only call: the create-only nonce argument is irrelevant.
    let frame =
        eth::prepare_initial_frame(req.host, caller, 0, tx.to, &tx.input, tx.value, &mut gas)?;
    let result = eth::execute_initial_frame(req.host, frame, &mut gas, gas_limit, reservoir)?;
    if !result.is_success() && !tx.value.is_zero() {
        req.host
            .state_mut()
            .account(&caller)?
            .set_balance(prepared.previous_balance);
    }
    eth::default_settle_gas(
        req.host,
        GasSettlement {
            caller,
            gas_price: U256::ZERO,
            gas_limit: tx.gas_limit,
            floor_gas: prepared.floor,
            initial_state_gas: 0,
            result,
        },
    )
}

#[cfg(test)]
pub(crate) mod tests;

#[derive(Debug, Clone, Copy)]
pub struct EezEvmFactory {
    pub(crate) collector: Result<Option<Address>, ProfileError>,
}
impl EezEvmFactory {
    pub fn for_rollup(collector: Address) -> Result<Self, ProfileError> {
        validate_fee_collector(collector)?;
        Ok(Self {
            collector: Ok(Some(collector)),
        })
    }
    pub const fn for_l1_simulation() -> Self {
        Self {
            collector: Ok(None),
        }
    }
}
impl reth_evm_ethereum::EvmFactory for EezEvmFactory {
    type Types = EezEvmTypes;
    type SpecId = SpecId;
    fn spec_id(&self, spec: SpecId) -> SpecId {
        spec
    }
    fn tx_registry(&self, spec: SpecId) -> TxRegistry<Self::Types, TxResult<Self::Types>> {
        registry(spec)
    }
    fn configure_evm(&self, evm: &mut Evm<'_, Self::Types>) {
        *evm.ext_mut() = EezEvmExt {
            collector: self.collector,
        };
    }
}
impl reth_evm::FromRecoveredTx<EezTxEnvelope> for EezEvmTx {
    fn from_recovered_tx(tx: Recovered<EezTxEnvelope>) -> Self {
        tx.into_inner().into()
    }
}
impl reth_evm::FromTxWithEncoded<EezTxEnvelope> for EezEvmTx {}

impl EezEvmTx {
    fn fields(&self) -> &dyn Transaction {
        match self {
            Self::Ethereum(tx) => tx,
            Self::Native { call, .. }
            | Self::Funded(call)
            | Self::FundedAccessList { call, .. } => call,
        }
    }
}
impl Transaction for EezEvmTx {
    fn chain_id(&self) -> Option<u64> {
        self.fields().chain_id()
    }
    fn nonce(&self) -> u64 {
        self.fields().nonce()
    }
    fn gas_limit(&self) -> u64 {
        self.fields().gas_limit()
    }
    fn gas_price(&self) -> Option<u128> {
        self.fields().gas_price()
    }
    fn max_fee_per_gas(&self) -> u128 {
        self.fields().max_fee_per_gas()
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.fields().max_priority_fee_per_gas()
    }
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.fields().max_fee_per_blob_gas()
    }
    fn priority_fee_or_price(&self) -> u128 {
        self.fields().priority_fee_or_price()
    }
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.fields().effective_gas_price(base_fee)
    }
    fn is_dynamic_fee(&self) -> bool {
        self.fields().is_dynamic_fee()
    }
    fn kind(&self) -> TxKind {
        self.fields().kind()
    }
    fn is_create(&self) -> bool {
        self.fields().is_create()
    }
    fn value(&self) -> U256 {
        self.fields().value()
    }
    fn input(&self) -> &Bytes {
        self.fields().input()
    }
    fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::Native { access_list, .. } | Self::FundedAccessList { access_list, .. } => {
                Some(access_list)
            }
            _ => self.fields().access_list(),
        }
    }
    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.fields().blob_versioned_hashes()
    }
    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.fields().authorization_list()
    }
}
impl reth_evm::TransactionEnvMut for EezEvmTx {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        match self {
            Self::Ethereum(tx) => tx.set_gas_limit(gas_limit),
            Self::Native { call, .. }
            | Self::Funded(call)
            | Self::FundedAccessList { call, .. } => call.gas_limit = gas_limit,
        }
    }
    fn set_access_list(&mut self, access_list: AccessList) {
        match self {
            Self::Ethereum(tx) => tx.set_access_list(access_list),
            // Retain forbidden input so native execution rejects it, rather than ignoring it.
            Self::Native {
                access_list: current,
                ..
            }
            | Self::FundedAccessList {
                access_list: current,
                ..
            } => *current = access_list,
            Self::Funded(call) => {
                *self = Self::FundedAccessList {
                    call: std::mem::take(call),
                    access_list,
                };
            }
        }
    }
}
