//! EEZ's unsigned system transactions and otherwise unchanged Ethereum primitives.

use alloy_consensus::{
    Transaction, TransactionEnvelope, Typed2718,
    transaction::{SignerRecoverable, TxHashRef},
};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Encodable2718};
use alloy_primitives::{Address, B256, Bytes, Sealable, Sealed, TxKind, U256, address};
use alloy_rlp::{Decodable, Encodable};
use reth_primitives_traits::InMemorySize;

/// Reserved EIP-2718 type for EEZ's unsigned system transactions.
pub const SYSTEM_TX_TYPE: u8 = 0x45;
/// The single native envelope version admitted by this fresh-network profile.
pub const SYSTEM_TX_VERSION: u64 = 1;
/// Upper bound of a native transaction's execution budget: Osaka's EIP-7825
/// per-transaction cap. Each native transaction encodes exactly
/// [`system_tx_gas_limit`] of the block's remaining gas,
/// so this cap is not reserved in the block; only metered use is charged.
pub const SYSTEM_TX_GAS_CAP: u64 = revm::primitives::eip7825::TX_GAS_LIMIT_CAP;
/// Metered budget cap of the scheduled native L1 history transaction.
pub const HISTORY_TX_GAS_CAP: u64 = 2_000_000;

/// Execution budget of a native transaction at its position in an L2 block:
/// the remaining block gas, bounded by [`SYSTEM_TX_GAS_CAP`].
///
/// `cumulative_gas_used` is the receipts' cumulative gas of the preceding
/// transactions, the figure Ethereum's block-gas rule charges. Every execution
/// that must agree with consensus (block executor, Composer draft, follower and
/// signer replay) derives the budget here. A budget below the transaction's
/// intrinsic gas makes the transaction, and therefore the block, invalid.
#[must_use]
pub const fn system_tx_gas_limit(block_gas_limit: u64, cumulative_gas_used: u64) -> u64 {
    let remaining = block_gas_limit.saturating_sub(cumulative_gas_used);
    if remaining < SYSTEM_TX_GAS_CAP {
        remaining
    } else {
        SYSTEM_TX_GAS_CAP
    }
}
/// Implicit native sender, also used by Ethereum's separate block-level system calls.
/// The native type and execution path, not the address alone, grant native authority.
pub const SYSTEM_ADDRESS: Address = address!("fffffffffffffffffffffffffffffffffffffffe");
/// L1 history predeploy. Only its protocol-authorized import may use the native sender.
pub const HISTORY_ADDRESS: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077");
/// The L2 execution manager: an upgradeable `EEZProxy` genesis predeploy whose
/// address, storage and balance stay fixed across implementation upgrades.
pub const EEZL2_ADDRESS: Address = address!("4B2760F84B83d6b3829E30A7573B0172B6d6402C");
/// The genesis implementation behind [`EEZL2_ADDRESS`]. Consumers use the proxy;
/// an upgrade replaces the implementation without changing the manager address.
pub const EEZL2_GENESIS_IMPLEMENTATION_ADDRESS: Address =
    address!("4C3D971b1079E58843D0E1af155A2e54b9E54dbA");

/// Canonical wire body: version, chain ID, source hash, gas, destination, value, calldata.
/// Signatures and an arbitrary sender are deliberately absent. Authorization is
/// established by derivation; this type must never enter a public transaction pool.
#[derive(Clone, Debug, PartialEq, Eq, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
pub struct SystemTransaction {
    pub version: u64,
    /// The wire integer is uint256; this runtime supports configured u64 chain IDs.
    pub chain_id: u64,
    pub source_hash: B256,
    pub gas_limit: u64,
    pub to: Address,
    pub value: U256,
    pub input: Bytes,
}

impl SystemTransaction {
    /// Check envelope shape without authorizing its occurrence or execution.
    /// Chain/profile binding, exact position gas and source identity are checked
    /// by the executor and settlement reconstruction.
    pub fn validate(&self) -> Result<(), alloy_rlp::Error> {
        if self.version != SYSTEM_TX_VERSION {
            return Err(alloy_rlp::Error::Custom(
                "unsupported native transaction version",
            ));
        }
        let cap = match self.to {
            EEZL2_ADDRESS => SYSTEM_TX_GAS_CAP,
            HISTORY_ADDRESS if self.value.is_zero() => HISTORY_TX_GAS_CAP,
            _ => {
                return Err(alloy_rlp::Error::Custom(
                    "invalid system transaction destination or value",
                ));
            }
        };
        if self.gas_limit > cap {
            return Err(alloy_rlp::Error::Custom(
                "native transaction exceeds its gas cap",
            ));
        }
        Ok(())
    }
}

impl serde::Serialize for SystemTransaction {
    /// Expose the actual native fields plus fixed Ethereum RPC placeholders.
    /// Nonce/signature/fee placeholders do not participate in the native hash.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use alloy_primitives::U64;
        use serde::ser::SerializeStruct;

        let mut tx = serializer.serialize_struct("SystemTransaction", 16)?;
        tx.serialize_field("version", &U64::from(self.version))?;
        tx.serialize_field("chainId", &U64::from(self.chain_id))?;
        tx.serialize_field("sourceHash", &self.source_hash)?;
        tx.serialize_field("nonce", &U64::ZERO)?;
        tx.serialize_field("to", &self.to)?;
        tx.serialize_field("value", &self.value)?;
        tx.serialize_field("input", &self.input)?;
        tx.serialize_field("gas", &U64::from(self.gas_limit()))?;
        tx.serialize_field("gasPrice", &U256::ZERO)?;
        tx.serialize_field("v", &U256::ZERO)?;
        tx.serialize_field("r", &U256::ZERO)?;
        tx.serialize_field("s", &U256::ZERO)?;
        tx.serialize_field("yParity", &U256::ZERO)?;
        tx.serialize_field("accessList", &[] as &[u8])?;
        tx.serialize_field("blobVersionedHashes", &[] as &[u8])?;
        tx.serialize_field("authorizationList", &[] as &[u8])?;
        tx.end()
    }
}

impl<'de> serde::Deserialize<'de> for SystemTransaction {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Rpc {
            #[serde(with = "alloy_serde::quantity")]
            version: u64,
            #[serde(with = "alloy_serde::quantity")]
            chain_id: u64,
            source_hash: B256,
            #[serde(with = "alloy_serde::quantity")]
            gas: u64,
            to: Address,
            value: U256,
            input: Bytes,
            nonce: Option<U256>,
            gas_price: Option<U256>,
            v: Option<U256>,
            r: Option<U256>,
            s: Option<U256>,
            y_parity: Option<U256>,
            access_list: Option<Vec<serde::de::IgnoredAny>>,
            blob_versioned_hashes: Option<Vec<serde::de::IgnoredAny>>,
            authorization_list: Option<Vec<serde::de::IgnoredAny>>,
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            max_fee_per_blob_gas: Option<U256>,
        }
        let rpc = Rpc::deserialize(deserializer)?;
        if [rpc.nonce, rpc.gas_price, rpc.v, rpc.r, rpc.s, rpc.y_parity]
            .into_iter()
            .flatten()
            .any(|value| !value.is_zero())
            || [
                rpc.access_list,
                rpc.blob_versioned_hashes,
                rpc.authorization_list,
            ]
            .into_iter()
            .flatten()
            .any(|values| !values.is_empty())
            || rpc.max_fee_per_gas.is_some()
            || rpc.max_priority_fee_per_gas.is_some()
            || rpc.max_fee_per_blob_gas.is_some()
        {
            return Err(serde::de::Error::custom(
                "invalid native RPC placeholder or fee field",
            ));
        }
        let tx = Self {
            version: rpc.version,
            chain_id: rpc.chain_id,
            source_hash: rpc.source_hash,
            gas_limit: rpc.gas,
            to: rpc.to,
            value: rpc.value,
            input: rpc.input,
        };
        tx.validate().map_err(serde::de::Error::custom)?;
        Ok(tx)
    }
}

impl Typed2718 for SystemTransaction {
    fn ty(&self) -> u8 {
        SYSTEM_TX_TYPE
    }
}

impl Encodable2718 for SystemTransaction {
    fn encode_2718_len(&self) -> usize {
        1 + self.length()
    }

    /// Omit signatures so every node can reconstruct the same bytes and hash.
    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_u8(SYSTEM_TX_TYPE);
        self.encode(out);
    }
}

impl Decodable2718 for SystemTransaction {
    /// Restrict native destinations; execution independently authorizes each operation.
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        if ty != SYSTEM_TX_TYPE {
            return Err(Eip2718Error::UnexpectedType(ty));
        }
        let tx = Self::decode(buf)?;
        tx.validate()?;
        Ok(tx)
    }

    /// Native transactions require the reserved type prefix; no legacy form exists.
    fn fallback_decode(_: &mut &[u8]) -> Result<Self, Eip2718Error> {
        Err(Eip2718Error::UnexpectedType(0))
    }
}

impl Sealable for SystemTransaction {
    fn hash_slow(&self) -> B256 {
        self.trie_hash()
    }
}

impl SignerRecoverable for SystemTransaction {
    /// The protocol supplies the sender; derivation and proof checks authorize the call.
    fn recover_signer(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        Ok(SYSTEM_ADDRESS)
    }

    fn recover_signer_unchecked(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        self.recover_signer()
    }
}

/// Expose the encoded gas budget and zero fees through Ethereum's transaction
/// interface. Nonce is a placeholder, never a system-account sequencing rule.
impl Transaction for SystemTransaction {
    fn chain_id(&self) -> Option<u64> {
        Some(self.chain_id)
    }

    fn nonce(&self) -> u64 {
        0
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_price(&self) -> Option<u128> {
        Some(0)
    }

    fn max_fee_per_gas(&self) -> u128 {
        0
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        0
    }

    fn effective_gas_price(&self, _: Option<u64>) -> u128 {
        0
    }

    fn is_dynamic_fee(&self) -> bool {
        false
    }

    fn kind(&self) -> TxKind {
        TxKind::Call(self.to)
    }

    fn is_create(&self) -> bool {
        false
    }

    fn value(&self) -> U256 {
        self.value
    }

    fn input(&self) -> &Bytes {
        &self.input
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        None
    }
}

#[derive(Clone, Debug, TransactionEnvelope)]
#[envelope(tx_type_name = EezTxType)]
pub enum EezTxEnvelope {
    #[envelope(flatten)]
    Ethereum(reth_ethereum_primitives::TransactionSigned),
    #[envelope(ty = 69)]
    System(Sealed<SystemTransaction>),
}

impl EezTxEnvelope {
    /// History imports are protocol transactions, but are not cross-chain actions.
    pub fn is_history(&self) -> bool {
        matches!(self, Self::System(tx) if tx.to == HISTORY_ADDRESS)
    }

    /// Only EEZL2 transactions delimit the reconstructed cross-chain suffix.
    pub fn is_cross_chain_system(&self) -> bool {
        matches!(self, Self::System(tx) if tx.to == EEZL2_ADDRESS)
    }
}

impl From<reth_ethereum_primitives::TransactionSigned> for EezTxEnvelope {
    fn from(tx: reth_ethereum_primitives::TransactionSigned) -> Self {
        Self::Ethereum(tx)
    }
}

impl From<SystemTransaction> for EezTxEnvelope {
    fn from(tx: SystemTransaction) -> Self {
        Self::System(Sealed::new(tx))
    }
}

impl SignerRecoverable for EezTxEnvelope {
    fn recover_signer(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Ethereum(tx) => tx.recover_signer(),
            Self::System(tx) => tx.recover_signer(),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Ethereum(tx) => tx.recover_signer_unchecked(),
            Self::System(tx) => tx.recover_signer_unchecked(),
        }
    }
}

impl TxHashRef for EezTxEnvelope {
    fn tx_hash(&self) -> &B256 {
        match self {
            Self::Ethereum(tx) => tx.tx_hash(),
            Self::System(tx) => tx.hash_ref(),
        }
    }
}

impl InMemorySize for EezTxEnvelope {
    fn size(&self) -> usize {
        match self {
            Self::Ethereum(tx) => tx.size(),
            Self::System(tx) => std::mem::size_of::<Self>() + tx.input.len(),
        }
    }
}

// Persist canonical EIP-2718 bytes so storage preserves native types and hashes.
impl reth_codecs::Compact for EezTxEnvelope {
    fn to_compact<B: alloy_rlp::BufMut + AsMut<[u8]>>(&self, buf: &mut B) -> usize {
        self.encode_2718(buf);
        self.encode_2718_len()
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (data, rest) = buf.split_at(len);
        (
            Self::decode_2718_exact(data).expect("invalid stored EEZ transaction"),
            rest,
        )
    }
}

pub type Block = alloy_consensus::Block<EezTxEnvelope>;
pub type BlockBody = alloy_consensus::BlockBody<EezTxEnvelope>;
pub type Receipt = alloy_consensus::EthereumReceipt<EezTxType>;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EezPrimitives;

impl reth_primitives_traits::NodePrimitives for EezPrimitives {
    type Block = Block;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = BlockBody;
    type SignedTx = EezTxEnvelope;
    type Receipt = Receipt;
}

impl InMemorySize for EezTxType {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

// Receipt storage needs the full type byte to retain the reserved native type.
impl reth_codecs::Compact for EezTxType {
    fn to_compact<B: alloy_rlp::BufMut + AsMut<[u8]>>(&self, buf: &mut B) -> usize {
        buf.put_u8((*self).into());
        1
    }

    fn from_compact(buf: &[u8], _: usize) -> (Self, &[u8]) {
        (
            Self::try_from(buf[0]).expect("invalid stored EEZ type"),
            &buf[1..],
        )
    }
}

impl alloy_evm::FromRecoveredTx<EezTxEnvelope> for revm::context::TxEnv {
    /// Preserve the exact native budget and protocol sender. Custom transaction
    /// validation bypasses Ethereum fee rules; the EEZ executor additionally
    /// authenticates the envelope and suppresses native account nonce mutation.
    fn from_recovered_tx(tx: &EezTxEnvelope, sender: Address) -> Self {
        match tx {
            EezTxEnvelope::Ethereum(tx) => Self::from_recovered_tx(tx, sender),
            EezTxEnvelope::System(tx) => Self {
                // Recovery normally supplies SYSTEM_ADDRESS. Preserve the supplied
                // sender so the EVM rejects inconsistent recovered input instead
                // of silently granting it native authority.
                caller: sender,
                gas_limit: tx.gas_limit(),
                gas_price: 0,
                kind: TxKind::Call(tx.to),
                value: tx.value,
                data: tx.input.clone(),
                nonce: 0,
                chain_id: Some(tx.chain_id),
                tx_type: SYSTEM_TX_TYPE,
                ..Default::default()
            },
        }
    }
}

impl alloy_evm::FromTxWithEncoded<EezTxEnvelope> for revm::context::TxEnv {
    fn from_encoded_tx(tx: &EezTxEnvelope, sender: Address, _: Bytes) -> Self {
        <Self as alloy_evm::FromRecoveredTx<EezTxEnvelope>>::from_recovered_tx(tx, sender)
    }
}

pub mod engine;

/// Public transaction gossip retains Ethereum's pooled envelope. Conversion from
/// a block is fallible so reorg reinjection cannot put native system txs in a pool.
impl TryFrom<EezTxEnvelope>
    for alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844WithSidecar>
{
    type Error = alloy_consensus::error::ValueError<EezTxEnvelope>;

    fn try_from(tx: EezTxEnvelope) -> Result<Self, Self::Error> {
        match tx {
            EezTxEnvelope::Ethereum(tx) => {
                Self::try_from(tx).map_err(|e| e.map(EezTxEnvelope::Ethereum))
            }
            tx => Err(alloy_consensus::error::ValueError::new(
                tx,
                "native system transactions cannot enter the pool",
            )),
        }
    }
}

impl From<alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844WithSidecar>>
    for EezTxEnvelope
{
    fn from(
        tx: alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844WithSidecar>,
    ) -> Self {
        Self::Ethereum(tx.into())
    }
}

impl reth_rpc_traits::SignableTxRequest<EezTxEnvelope> for alloy_rpc_types_eth::TransactionRequest {
    async fn try_build_and_sign(
        self,
        signer: impl alloy_network::TxSigner<alloy_primitives::Signature> + Send,
    ) -> Result<EezTxEnvelope, reth_rpc_traits::SignTxRequestError> {
        <Self as reth_rpc_traits::SignableTxRequest<reth_ethereum_primitives::TransactionSigned>>::try_build_and_sign(self, signer).await.map(EezTxEnvelope::Ethereum)
    }
}

impl reth_rpc_traits::TryIntoSimTx<EezTxEnvelope> for alloy_rpc_types_eth::TransactionRequest {
    fn try_into_sim_tx(self) -> Result<EezTxEnvelope, alloy_consensus::error::ValueError<Self>> {
        self.build_typed_simulate_transaction()
            .map(EezTxEnvelope::Ethereum)
    }
}

// Use the same canonical wire format as Compact for database compression.
impl reth_codecs::Compress for EezTxEnvelope {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: alloy_rlp::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        self.encode_2718(buf);
    }
}

impl reth_codecs::Decompress for EezTxEnvelope {
    fn decompress(value: &[u8]) -> Result<Self, reth_codecs::DecompressError> {
        Self::decode_2718_exact(value).map_err(reth_codecs::DecompressError::new)
    }
}

#[cfg(test)]
mod tests;

impl<T> From<alloy_consensus::Signed<T>> for EezTxEnvelope
where
    reth_ethereum_primitives::TransactionSigned: From<alloy_consensus::Signed<T>>,
{
    fn from(tx: alloy_consensus::Signed<T>) -> Self {
        Self::Ethereum(tx.into())
    }
}

impl Default for EezTxType {
    fn default() -> Self {
        Self::Ethereum(alloy_consensus::TxType::Legacy)
    }
}
