//! Dedicated history updates: the parent state and block position supply all context.
use super::*;

/// Reserved local EIP-2718 type, distinct from cross-chain source transactions.
pub const HISTORY_TX_TYPE: u8 = 0x46;
pub const HISTORY_TX_VERSION: u64 = 1;
pub const MAX_HISTORY_HASHES: usize = 64;

/// Exact wire body: `RLP([version, chainId, hashes])`. There is no source identity,
/// calldata, gas, destination or value supplied by the producer. The executor
/// derives those fields after authenticating the parent and the header chain.
#[derive(Clone, Debug, PartialEq, Eq, alloy_rlp::RlpEncodable)]
pub struct HistoryTransaction {
    pub version: u64,
    pub chain_id: u64,
    pub hashes: Vec<B256>,
}

impl HistoryTransaction {
    pub fn validate(&self) -> Result<(), alloy_rlp::Error> {
        if self.version != HISTORY_TX_VERSION {
            return Err(alloy_rlp::Error::Custom(
                "unsupported history transaction version",
            ));
        }
        if self.hashes.is_empty()
            || self.hashes.len() > MAX_HISTORY_HASHES
            || self.hashes.iter().any(B256::is_zero)
        {
            return Err(alloy_rlp::Error::Custom("invalid history hash list"));
        }
        Ok(())
    }
}

impl Decodable for HistoryTransaction {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let mut input = *buf;
        let outer = alloy_rlp::Header::decode(&mut input)?;
        if !outer.list || outer.payload_length > 2 * 9 + 3 + MAX_HISTORY_HASHES * 33 {
            return Err(alloy_rlp::Error::Custom("invalid history envelope length"));
        }
        if input.len() < outer.payload_length {
            return Err(alloy_rlp::Error::InputTooShort);
        }
        let (mut fields, rest) = input.split_at(outer.payload_length);
        let version = u64::decode(&mut fields)?;
        let chain_id = u64::decode(&mut fields)?;
        let list = alloy_rlp::Header::decode(&mut fields)?;
        if !list.list
            || list.payload_length != fields.len()
            || list.payload_length == 0
            || list.payload_length > MAX_HISTORY_HASHES * 33
            || !list.payload_length.is_multiple_of(33)
        {
            return Err(alloy_rlp::Error::Custom("invalid history hash list length"));
        }
        let mut hashes = Vec::with_capacity(list.payload_length / 33);
        while !fields.is_empty() {
            hashes.push(B256::decode(&mut fields)?);
        }
        let tx = Self {
            version,
            chain_id,
            hashes,
        };
        tx.validate()?;
        *buf = rest;
        Ok(tx)
    }
}

impl Typed2718 for HistoryTransaction {
    fn ty(&self) -> u8 {
        HISTORY_TX_TYPE
    }
}
impl Encodable2718 for HistoryTransaction {
    fn encode_2718_len(&self) -> usize {
        1 + self.length()
    }
    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_u8(HISTORY_TX_TYPE);
        self.encode(out);
    }
}
impl Decodable2718 for HistoryTransaction {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        if ty != HISTORY_TX_TYPE {
            return Err(Eip2718Error::UnexpectedType(ty));
        }
        Ok(Self::decode(buf)?)
    }
    fn fallback_decode(_: &mut &[u8]) -> Result<Self, Eip2718Error> {
        Err(Eip2718Error::UnexpectedType(0))
    }
}
impl Sealable for HistoryTransaction {
    fn hash_slow(&self) -> B256 {
        self.trie_hash()
    }
}
impl SignerRecoverable for HistoryTransaction {
    fn recover_signer(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        Ok(SYSTEM_ADDRESS)
    }
    fn recover_signer_unchecked(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        self.recover_signer()
    }
}

impl Transaction for HistoryTransaction {
    fn chain_id(&self) -> Option<u64> {
        Some(self.chain_id)
    }
    fn nonce(&self) -> u64 {
        0
    }
    // The supported profile has a fixed block limit above this cap and history
    // is first. Execution still derives min(cap, remaining block gas).
    fn gas_limit(&self) -> u64 {
        HISTORY_TX_GAS_CAP
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
        TxKind::Call(HISTORY_ADDRESS)
    }
    fn is_create(&self) -> bool {
        false
    }
    fn value(&self) -> U256 {
        U256::ZERO
    }
    /// Calldata is not a transaction field. Execution derives it from the hashes
    /// and the eligible cutoff; RPC exposes `hashes` instead of invented input.
    fn input(&self) -> &Bytes {
        static EMPTY: Bytes = Bytes::new();
        &EMPTY
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

impl serde::Serialize for HistoryTransaction {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use alloy_primitives::U64;
        use serde::ser::SerializeStruct;
        let mut tx = serializer.serialize_struct("HistoryTransaction", 13)?;
        tx.serialize_field("version", &U64::from(self.version))?;
        tx.serialize_field("chainId", &U64::from(self.chain_id))?;
        tx.serialize_field("hashes", &self.hashes)?;
        tx.serialize_field("nonce", &U64::ZERO)?;
        tx.serialize_field("to", &HISTORY_ADDRESS)?;
        tx.serialize_field("value", &U256::ZERO)?;
        tx.serialize_field("input", &Bytes::new())?;
        tx.serialize_field("gas", &U64::from(HISTORY_TX_GAS_CAP))?;
        tx.serialize_field("gasPrice", &U256::ZERO)?;
        tx.serialize_field("v", &U256::ZERO)?;
        tx.serialize_field("r", &U256::ZERO)?;
        tx.serialize_field("s", &U256::ZERO)?;
        tx.serialize_field("yParity", &U256::ZERO)?;
        tx.end()
    }
}

impl<'de> serde::Deserialize<'de> for HistoryTransaction {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Rpc {
            #[serde(with = "alloy_serde::quantity")]
            version: u64,
            #[serde(with = "alloy_serde::quantity")]
            chain_id: u64,
            #[serde(deserialize_with = "deserialize_hashes")]
            hashes: Vec<B256>,
            nonce: Option<U256>,
            to: Option<Address>,
            value: Option<U256>,
            input: Option<Bytes>,
            gas: Option<U256>,
            gas_price: Option<U256>,
            v: Option<U256>,
            r: Option<U256>,
            s: Option<U256>,
            y_parity: Option<U256>,
            source_hash: Option<B256>,
            access_list: Option<Vec<serde::de::IgnoredAny>>,
            blob_versioned_hashes: Option<Vec<serde::de::IgnoredAny>>,
            authorization_list: Option<Vec<serde::de::IgnoredAny>>,
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            max_fee_per_blob_gas: Option<U256>,
        }
        let rpc = Rpc::deserialize(deserializer)?;
        if [
            rpc.nonce,
            rpc.value,
            rpc.gas_price,
            rpc.v,
            rpc.r,
            rpc.s,
            rpc.y_parity,
        ]
        .into_iter()
        .flatten()
        .any(|v| !v.is_zero())
            || rpc.to.is_some_and(|to| to != HISTORY_ADDRESS)
            || rpc
                .gas
                .is_some_and(|gas| gas != U256::from(HISTORY_TX_GAS_CAP))
            || rpc.input.is_some_and(|input| !input.is_empty())
            || rpc.source_hash.is_some()
            || rpc.max_fee_per_gas.is_some()
            || rpc.max_priority_fee_per_gas.is_some()
            || rpc.max_fee_per_blob_gas.is_some()
            || [
                rpc.access_list,
                rpc.blob_versioned_hashes,
                rpc.authorization_list,
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.is_empty())
        {
            return Err(serde::de::Error::custom(
                "invalid history RPC placeholder or context field",
            ));
        }
        let tx = Self {
            version: rpc.version,
            chain_id: rpc.chain_id,
            hashes: rpc.hashes,
        };
        tx.validate().map_err(serde::de::Error::custom)?;
        Ok(tx)
    }
}

/// Bound JSON list allocation just as the canonical RLP decoder does.
fn deserialize_hashes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<B256>, D::Error> {
    struct Hashes;
    impl<'de> serde::de::Visitor<'de> for Hashes {
        type Value = Vec<B256>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("one to 64 history hashes")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            if sequence
                .size_hint()
                .is_some_and(|size| size > MAX_HISTORY_HASHES)
            {
                return Err(serde::de::Error::custom("history hash list exceeds bound"));
            }
            let mut hashes = Vec::new();
            while let Some(hash) = sequence.next_element()? {
                if hashes.len() == MAX_HISTORY_HASHES {
                    return Err(serde::de::Error::custom("history hash list exceeds bound"));
                }
                hashes.push(hash);
            }
            Ok(hashes)
        }
    }
    deserializer.deserialize_seq(Hashes)
}
