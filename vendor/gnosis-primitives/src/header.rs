use std::mem;

use alloy_consensus::{Block, BlockBody, EMPTY_OMMER_ROOT_HASH, Header, Sealed};
use alloy_eips::{
    BlockNumHash, calc_next_block_base_fee, eip1898::BlockWithParent, eip7840::BlobParams,
};
use alloy_primitives::{
    Address, B64, B256, BlockHash, BlockNumber, Bloom, Bytes, FixedBytes, Sealable, U256, keccak256,
};
use alloy_rlp::{BufMut, Decodable, Encodable, length_of_length};
use alloy_trie::EMPTY_ROOT_HASH;
use reth_chainspec::BaseFeeParams;
use reth_cli_commands::common::HeaderMut;
use reth_codecs::{Compact, DecompressError};
use reth_db::table::{Compress, Decompress};
use reth_primitives_traits::InMemorySize;
use reth_tracing::tracing::debug;
use serde::{Deserialize, Serialize};

pub fn default_mix_hash() -> Option<B256> {
    Some(B256::ZERO)
}

pub fn default_nonce() -> Option<B64> {
    Some(B64::ZERO)
}

/// The header type of this node
///
/// This type extends the regular ethereum header with an extension.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    // derive_more::AsRef,
    // derive_more::Deref,
    // derive_more::DerefMut,
    Default,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct GnosisHeader {
    /// The Keccak 256-bit hash of the parent
    /// block’s header, in its entirety; formally Hp.
    pub parent_hash: B256,
    /// The Keccak 256-bit hash of the ommers list portion of this block; formally Ho.
    #[serde(rename = "sha3Uncles", alias = "ommersHash")]
    pub ommers_hash: B256,
    /// The 160-bit address to which all fees collected from the successful mining of this block
    /// be transferred; formally Hc.
    #[serde(rename = "miner", alias = "beneficiary")]
    pub beneficiary: Address,
    /// The Keccak 256-bit hash of the root node of the state trie, after all transactions are
    /// executed and finalisations applied; formally Hr.
    pub state_root: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with each
    /// transaction in the transactions list portion of the block; formally Ht.
    pub transactions_root: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with the receipts
    /// of each transaction in the transactions list portion of the block; formally He.
    pub receipts_root: B256,
    /// The Bloom filter composed from indexable information (logger address and log topics)
    /// contained in each log entry from the receipt of each transaction in the transactions list;
    /// formally Hb.
    pub logs_bloom: Bloom,
    /// A scalar value corresponding to the difficulty level of this block. This can be calculated
    /// from the previous block’s difficulty level and the timestamp; formally Hd.
    pub difficulty: U256,
    /// A scalar value equal to the number of ancestor blocks. The genesis block has a number of
    /// zero; formally Hi.
    #[serde(with = "alloy_serde::quantity")]
    pub number: BlockNumber,
    /// A scalar value equal to the current limit of gas expenditure per block; formally Hl.
    #[serde(with = "alloy_serde::quantity")]
    pub gas_limit: u64,
    /// A scalar value equal to the total gas used in transactions in this block; formally Hg.
    #[serde(with = "alloy_serde::quantity")]
    pub gas_used: u64,
    /// A scalar value equal to the reasonable output of Unix’s time() at this block’s inception;
    /// formally Hs.
    #[serde(with = "alloy_serde::quantity")]
    pub timestamp: u64,
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
    /// A 256-bit hash which, combined with the
    /// nonce, proves that a sufficient amount of computation has been carried out on this block;
    /// formally Hm.
    #[serde(default = "default_mix_hash", skip_serializing_if = "Option::is_none")]
    pub mix_hash: Option<B256>,
    /// A 64-bit value which, combined with the mixhash, proves that a sufficient amount of
    /// computation has been carried out on this block; formally Hn.
    #[serde(default = "default_nonce", skip_serializing_if = "Option::is_none")]
    pub nonce: Option<B64>,
    /// Gnosis-specific fields for Aura Consensus
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aura_step: Option<U256>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aura_seal: Option<FixedBytes<65>>,
    /// A scalar representing EIP1559 base fee which can move up or down each block according
    /// to a formula which is a function of gas used in parent block and gas target
    /// (block gas limit divided by elasticity multiplier) of parent block.
    /// The algorithm results in the base fee per gas increasing when blocks are
    /// above the gas target, and decreasing when blocks are below the gas target. The base fee per
    /// gas is burned.
    #[serde(
        default,
        with = "alloy_serde::quantity::opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_fee_per_gas: Option<u64>,
    /// The Keccak 256-bit hash of the withdrawals list portion of this block.
    /// <https://eips.ethereum.org/EIPS/eip-4895>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals_root: Option<B256>,
    /// The total amount of blob gas consumed by the transactions within the block, added in
    /// EIP-4844.
    #[serde(
        default,
        with = "alloy_serde::quantity::opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub blob_gas_used: Option<u64>,
    /// A running total of blob gas consumed in excess of the target, prior to the block. Blocks
    /// with above-target blob gas consumption increase this value, blocks with below-target blob
    /// gas consumption decrease it (bounded at 0). This was added in EIP-4844.
    #[serde(
        default,
        with = "alloy_serde::quantity::opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub excess_blob_gas: Option<u64>,
    /// The hash of the parent beacon block's root is included in execution blocks, as proposed by
    /// EIP-4788.
    ///
    /// This enables trust-minimized access to consensus state, supporting staking pools, bridges,
    /// and more.
    ///
    /// The beacon roots contract handles root storage, enhancing Ethereum's functionalities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_beacon_block_root: Option<B256>,
    /// The Keccak 256-bit hash of the an RLP encoded list with each
    /// [EIP-7685] request in the block body.
    ///
    /// [EIP-7685]: https://eips.ethereum.org/EIPS/eip-7685
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_hash: Option<B256>,
    /// The Keccak 256-bit hash of the block's access list.
    ///
    /// When no state changes are present, this field is the hash of an empty RLP list:
    /// `keccak256(rlp.encode([]))` =
    /// `0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347`
    ///
    /// [EIP-7928]: https://eips.ethereum.org/EIPS/eip-7928
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_access_list_hash: Option<B256>,
    /// The slot number corresponding to this block, calculated in the consensus layer.
    ///
    /// [EIP-7843]: https://eips.ethereum.org/EIPS/eip-7843
    #[serde(
        default,
        with = "alloy_serde::quantity::opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub slot_number: Option<u64>,
}

impl GnosisHeader {
    /// Create a [`Block`] from the body and its header.
    pub fn into_block<T>(self, body: BlockBody<T>) -> Block<T> {
        body.into_block(self.into())
    }

    /// Heavy function that will calculate hash of data and will *not* save the change to metadata.
    ///
    /// Use [`Header::seal_slow`] and unlock if you need the hash to be persistent.
    pub fn hash_slow(&self) -> B256 {
        let mut out = Vec::<u8>::new();
        self.encode(&mut out);
        keccak256(&out)
    }

    /// Check if the ommers hash equals to empty hash list.
    pub fn ommers_hash_is_empty(&self) -> bool {
        self.ommers_hash == EMPTY_OMMER_ROOT_HASH
    }

    /// Check if the transaction root equals to empty root.
    pub fn transaction_root_is_empty(&self) -> bool {
        *self.transactions_root == *EMPTY_ROOT_HASH
    }

    /// Returns the blob fee for _this_ block according to the EIP-4844 spec.
    ///
    /// Returns `None` if `excess_blob_gas` is None
    pub fn blob_fee(&self, blob_params: BlobParams) -> Option<u128> {
        Some(blob_params.calc_blob_fee(self.excess_blob_gas?))
    }

    /// Returns the blob fee for the next block according to the EIP-4844 spec.
    ///
    /// Returns `None` if `excess_blob_gas` is None.
    ///
    /// See also [Self::next_block_excess_blob_gas]
    pub fn next_block_blob_fee(&self, blob_params: BlobParams) -> Option<u128> {
        Some(blob_params.calc_blob_fee(self.next_block_excess_blob_gas(blob_params)?))
    }

    /// Calculate base fee for next block according to the EIP-1559 spec.
    ///
    /// Returns a `None` if no base fee is set, no EIP-1559 support
    pub fn next_block_base_fee(&self, base_fee_params: BaseFeeParams) -> Option<u64> {
        Some(calc_next_block_base_fee(
            self.gas_used,
            self.gas_limit,
            self.base_fee_per_gas?,
            base_fee_params,
        ))
    }

    /// Calculate excess blob gas for the next block according to the EIP-4844
    /// spec.
    ///
    /// Returns a `None` if no excess blob gas is set, no EIP-4844 support
    pub fn next_block_excess_blob_gas(&self, blob_params: BlobParams) -> Option<u64> {
        Some(blob_params.next_block_excess_blob_gas_osaka(
            self.excess_blob_gas?,
            self.blob_gas_used?,
            self.base_fee_per_gas?,
        ))
    }

    /// Calculate a heuristic for the in-memory size of the [Header].
    #[inline]
    pub fn size_of(&self) -> usize {
        mem::size_of::<B256>() + // parent hash
        mem::size_of::<B256>() + // ommers hash
        mem::size_of::<Address>() + // beneficiary
        mem::size_of::<B256>() + // state root
        mem::size_of::<B256>() + // transactions root
        mem::size_of::<B256>() + // receipts root
        mem::size_of::<Option<B256>>() + // withdrawals root
        mem::size_of::<Bloom>() + // logs bloom
        mem::size_of::<U256>() + // difficulty
        mem::size_of::<BlockNumber>() + // number
        mem::size_of::<u128>() + // gas limit
        mem::size_of::<u128>() + // gas used
        mem::size_of::<u64>() + // timestamp
        // mem::size_of::<B256>() + // mix hash
        // mem::size_of::<u64>() + // nonce
        mem::size_of::<Option<u128>>() + // base fee per gas
        mem::size_of::<Option<u128>>() + // blob gas used
        mem::size_of::<Option<u128>>() + // excess blob gas
        mem::size_of::<Option<B256>>() + // parent beacon block root
        mem::size_of::<Option<B256>>() + // requests root
        self.extra_data.len() // extra data
    }

    fn header_payload_length(&self) -> usize {
        let mut length = 0;
        length += self.parent_hash.length();
        length += self.ommers_hash.length();
        length += self.beneficiary.length();
        length += self.state_root.length();
        length += self.transactions_root.length();
        length += self.receipts_root.length();
        length += self.logs_bloom.length();
        length += self.difficulty.length();
        length += U256::from(self.number).length();
        length += U256::from(self.gas_limit).length();
        length += U256::from(self.gas_used).length();
        length += self.timestamp.length();
        length += self.extra_data.length();
        if self.is_post_merge() {
            // If the header is post-merge, we have mix_hash and nonce.
            length += self.mix_hash.as_ref().map_or(0, |hash| hash.length());
            length += self.nonce.as_ref().map_or(0, |nonce| nonce.length());
        } else {
            // If the header is pre-merge, we have aura_step and aura_seal.
            length += self.aura_step.unwrap_or(U256::ZERO).length();
            length += self.aura_seal.as_ref().map_or(0, |seal| seal.length());
        }

        if let Some(base_fee) = self.base_fee_per_gas {
            // Adding base fee length if it exists.
            length += U256::from(base_fee).length();
        }

        if let Some(root) = self.withdrawals_root {
            // Adding withdrawals_root length if it exists.
            length += root.length();
        }

        if let Some(blob_gas_used) = self.blob_gas_used {
            // Adding blob_gas_used length if it exists.
            length += U256::from(blob_gas_used).length();
        }

        if let Some(excess_blob_gas) = self.excess_blob_gas {
            // Adding excess_blob_gas length if it exists.
            length += U256::from(excess_blob_gas).length();
        }

        if let Some(parent_beacon_block_root) = self.parent_beacon_block_root {
            length += parent_beacon_block_root.length();
        }

        if let Some(requests_hash) = self.requests_hash {
            length += requests_hash.length();
        }

        if let Some(block_access_list_hash) = self.block_access_list_hash {
            length += block_access_list_hash.length();
        }

        if let Some(slot_number) = self.slot_number {
            length += U256::from(slot_number).length();
        }

        length
    }

    /// Returns the parent block's number and hash
    ///
    /// Note: for the genesis block the parent number is 0 and the parent hash is the zero hash.
    pub const fn parent_num_hash(&self) -> BlockNumHash {
        BlockNumHash {
            number: self.number.saturating_sub(1),
            hash: self.parent_hash,
        }
    }

    /// Returns the block's number and hash.
    ///
    /// Note: this hashes the header.
    pub fn num_hash_slow(&self) -> BlockNumHash {
        BlockNumHash {
            number: self.number,
            hash: self.hash_slow(),
        }
    }

    /// Returns the block's number and hash with the parent hash.
    ///
    /// Note: this hashes the header.
    pub fn num_hash_with_parent_slow(&self) -> BlockWithParent {
        BlockWithParent::new(self.parent_hash, self.num_hash_slow())
    }

    /// Seal the header with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    #[inline]
    pub const fn seal(self, hash: B256) -> Sealed<Self> {
        Sealed::new_unchecked(self, hash)
    }

    /// True if the shanghai hardfork is active.
    ///
    /// This function checks that the withdrawals root field is present.
    pub const fn shanghai_active(&self) -> bool {
        self.withdrawals_root.is_some()
    }

    /// True if the Cancun hardfork is active.
    ///
    /// This function checks that the blob gas used field is present.
    pub const fn cancun_active(&self) -> bool {
        self.blob_gas_used.is_some()
    }

    /// True if the Prague hardfork is active.
    ///
    /// This function checks that the requests hash is present.
    pub const fn prague_active(&self) -> bool {
        self.requests_hash.is_some()
    }

    /// True if the Amsterdam hardfork is active.
    ///
    /// This function checks that the block access list hash is present.
    pub const fn amsterdam_active(&self) -> bool {
        self.block_access_list_hash.is_some()
    }

    pub fn is_post_merge(&self) -> bool {
        self.mix_hash.is_some() && self.nonce.is_some()
    }

    pub fn is_pre_merge(&self) -> bool {
        self.aura_step.is_some() && self.aura_seal.is_some()
    }

    pub fn to_alloy_header(&self) -> Header {
        if self.mix_hash.is_none() || self.nonce.is_none() {
            panic!(
                "GnosisHeader must have mix_hash and nonce set to convert to alloy_consensus::Header. All post-merge headers have these fields set."
            );
        }
        Header {
            parent_hash: self.parent_hash,
            ommers_hash: self.ommers_hash,
            beneficiary: self.beneficiary,
            state_root: self.state_root,
            transactions_root: self.transactions_root,
            receipts_root: self.receipts_root,
            logs_bloom: self.logs_bloom,
            difficulty: self.difficulty,
            number: self.number,
            gas_limit: self.gas_limit,
            gas_used: self.gas_used,
            timestamp: self.timestamp,
            extra_data: self.extra_data.clone(),
            mix_hash: self.mix_hash.unwrap(),
            nonce: self.nonce.unwrap(),
            base_fee_per_gas: self.base_fee_per_gas,
            withdrawals_root: self.withdrawals_root,
            blob_gas_used: self.blob_gas_used,
            excess_blob_gas: self.excess_blob_gas,
            parent_beacon_block_root: self.parent_beacon_block_root,
            requests_hash: self.requests_hash,
            block_access_list_hash: self.block_access_list_hash,
            slot_number: self.slot_number,
        }
    }
}

// derive from alloy_consensus::Header
impl From<Header> for GnosisHeader {
    fn from(inner: Header) -> Self {
        Self {
            parent_hash: inner.parent_hash,
            ommers_hash: inner.ommers_hash,
            beneficiary: inner.beneficiary,
            state_root: inner.state_root,
            transactions_root: inner.transactions_root,
            receipts_root: inner.receipts_root,
            logs_bloom: inner.logs_bloom,
            difficulty: inner.difficulty,
            number: inner.number,
            gas_limit: inner.gas_limit,
            gas_used: inner.gas_used,
            timestamp: inner.timestamp,
            extra_data: inner.extra_data,
            mix_hash: Some(inner.mix_hash),
            nonce: Some(inner.nonce),
            aura_seal: None,
            aura_step: None,
            base_fee_per_gas: inner.base_fee_per_gas,
            withdrawals_root: inner.withdrawals_root,
            blob_gas_used: inner.blob_gas_used,
            excess_blob_gas: inner.excess_blob_gas,
            parent_beacon_block_root: inner.parent_beacon_block_root,
            requests_hash: inner.requests_hash,
            block_access_list_hash: inner.block_access_list_hash,
            slot_number: inner.slot_number,
        }
    }
}

impl From<GnosisHeader> for Header {
    fn from(gnosis_header: GnosisHeader) -> Self {
        if gnosis_header.mix_hash.is_none() || gnosis_header.nonce.is_none() {
            panic!(
                "GnosisHeader must have mix_hash and nonce set to convert to alloy_consensus::Header. All post-merge headers have these fields set."
            );
        }
        Header {
            parent_hash: gnosis_header.parent_hash,
            ommers_hash: gnosis_header.ommers_hash,
            beneficiary: gnosis_header.beneficiary,
            state_root: gnosis_header.state_root,
            transactions_root: gnosis_header.transactions_root,
            receipts_root: gnosis_header.receipts_root,
            logs_bloom: gnosis_header.logs_bloom,
            difficulty: gnosis_header.difficulty,
            number: gnosis_header.number,
            gas_limit: gnosis_header.gas_limit,
            gas_used: gnosis_header.gas_used,
            timestamp: gnosis_header.timestamp,
            extra_data: gnosis_header.extra_data,
            mix_hash: gnosis_header.mix_hash.unwrap(),
            nonce: gnosis_header.nonce.unwrap(),
            base_fee_per_gas: gnosis_header.base_fee_per_gas,
            withdrawals_root: gnosis_header.withdrawals_root,
            blob_gas_used: gnosis_header.blob_gas_used,
            excess_blob_gas: gnosis_header.excess_blob_gas,
            parent_beacon_block_root: gnosis_header.parent_beacon_block_root,
            requests_hash: gnosis_header.requests_hash,
            block_access_list_hash: gnosis_header.block_access_list_hash,
            slot_number: gnosis_header.slot_number,
        }
    }
}

impl AsRef<Self> for GnosisHeader {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Sealable for GnosisHeader {
    fn hash_slow(&self) -> B256 {
        let mut out = Vec::new();
        self.encode(&mut out);
        keccak256(&out)
    }
}

impl alloy_consensus::BlockHeader for GnosisHeader {
    fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    fn ommers_hash(&self) -> B256 {
        self.ommers_hash
    }

    fn beneficiary(&self) -> Address {
        self.beneficiary
    }

    fn state_root(&self) -> B256 {
        self.state_root
    }

    fn transactions_root(&self) -> B256 {
        self.transactions_root
    }

    fn receipts_root(&self) -> B256 {
        self.receipts_root
    }

    fn withdrawals_root(&self) -> Option<B256> {
        self.withdrawals_root
    }

    fn logs_bloom(&self) -> Bloom {
        self.logs_bloom
    }

    fn difficulty(&self) -> U256 {
        self.difficulty
    }

    fn number(&self) -> BlockNumber {
        self.number
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_used(&self) -> u64 {
        self.gas_used
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn mix_hash(&self) -> Option<B256> {
        self.mix_hash
    }

    fn nonce(&self) -> Option<B64> {
        self.nonce
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.base_fee_per_gas
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.blob_gas_used
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.excess_blob_gas
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.parent_beacon_block_root
    }

    fn requests_hash(&self) -> Option<B256> {
        self.requests_hash
    }

    fn block_access_list_hash(&self) -> Option<B256> {
        self.block_access_list_hash
    }

    fn slot_number(&self) -> Option<u64> {
        self.slot_number
    }

    fn extra_data(&self) -> &Bytes {
        &self.extra_data
    }
}

impl reth_primitives_traits::BlockHeader for GnosisHeader {}

impl InMemorySize for GnosisHeader {
    fn size(&self) -> usize {
        let mut size = self.size_of();
        if self.is_post_merge() {
            size += mem::size_of::<B64>() + mem::size_of::<u64>();
        } else {
            debug!("Pre-merge header detected");
            size += mem::size_of::<Option<u64>>()
                + self.aura_seal.as_ref().map_or(0, |seal| seal.len());
        }
        size
    }
}

impl Encodable for GnosisHeader {
    fn encode(&self, out: &mut dyn BufMut) {
        let mut buffer = Vec::new();

        let list_header = alloy_rlp::Header {
            list: true,
            payload_length: self.header_payload_length(),
        };
        list_header.encode(&mut buffer);
        self.parent_hash.encode(&mut buffer);
        self.ommers_hash.encode(&mut buffer);
        self.beneficiary.encode(&mut buffer);
        self.state_root.encode(&mut buffer);
        self.transactions_root.encode(&mut buffer);
        self.receipts_root.encode(&mut buffer);
        self.logs_bloom.encode(&mut buffer);
        self.difficulty.encode(&mut buffer);
        U256::from(self.number).encode(&mut buffer);
        U256::from(self.gas_limit).encode(&mut buffer);
        U256::from(self.gas_used).encode(&mut buffer);
        self.timestamp.encode(&mut buffer);
        self.extra_data.encode(&mut buffer);

        if self.is_post_merge() {
            self.mix_hash.unwrap().encode(&mut buffer);
            self.nonce.unwrap().encode(&mut buffer);
        } else {
            self.aura_step.unwrap().encode(&mut buffer);
            self.aura_seal.as_ref().unwrap().encode(&mut buffer);
        }

        // Encode all the fork specific fields
        if let Some(ref base_fee) = self.base_fee_per_gas {
            U256::from(*base_fee).encode(&mut buffer);
        }

        if let Some(ref root) = self.withdrawals_root {
            root.encode(&mut buffer);
        }

        if let Some(ref blob_gas_used) = self.blob_gas_used {
            U256::from(*blob_gas_used).encode(&mut buffer);
        }

        if let Some(ref excess_blob_gas) = self.excess_blob_gas {
            U256::from(*excess_blob_gas).encode(&mut buffer);
        }

        if let Some(ref parent_beacon_block_root) = self.parent_beacon_block_root {
            parent_beacon_block_root.encode(&mut buffer);
        }

        if let Some(ref requests_hash) = self.requests_hash {
            requests_hash.encode(&mut buffer);
        }

        if let Some(ref block_access_list_hash) = self.block_access_list_hash {
            block_access_list_hash.encode(out);
        }

        if let Some(ref slot_number) = self.slot_number {
            U256::from(*slot_number).encode(out);
        }

        // Write the encoded buffer to the output
        out.put_slice(&buffer);
    }

    fn length(&self) -> usize {
        let mut length = 0;
        length += self.header_payload_length();
        length += length_of_length(length);
        length
    }
}

impl Decodable for GnosisHeader {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let rlp_head = alloy_rlp::Header::decode(buf)?;
        if !rlp_head.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        let started_len = buf.len();
        let mut this = Self {
            parent_hash: Decodable::decode(buf)?,
            ommers_hash: Decodable::decode(buf)?,
            beneficiary: Decodable::decode(buf)?,
            state_root: Decodable::decode(buf)?,
            transactions_root: Decodable::decode(buf)?,
            receipts_root: Decodable::decode(buf)?,
            logs_bloom: Decodable::decode(buf)?,
            difficulty: Decodable::decode(buf)?,
            number: u64::decode(buf)?,
            gas_limit: u64::decode(buf)?,
            gas_used: u64::decode(buf)?,
            timestamp: Decodable::decode(buf)?,
            extra_data: Decodable::decode(buf)?,

            mix_hash: None,
            nonce: None,
            aura_step: None,
            aura_seal: None,

            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        };

        // Peek at the next RLP header without advancing buf
        // Create a temporary immutable slice to peek
        let peek_slice = &buf[..];
        let next_head = alloy_rlp::Header::decode(&mut &peek_slice[..])?;
        let is_post_merge = next_head.payload_length == 32; // 32 bytes for mix_hash

        if is_post_merge {
            // Next field is mix_hash (32 bytes)
            this.mix_hash = Some(Decodable::decode(buf)?);
            this.nonce = Some(B64::decode(buf)?);
        } else {
            // Next field is AuRaStep (u64, usually 8 bytes)
            this.aura_step = Some(U256::decode(buf)?);

            // Next field is AuRaSeal (variable length)
            let aura_seal_bytes = Bytes::decode(buf)?;
            this.aura_seal = Some(
                FixedBytes::<65>::try_from(aura_seal_bytes.as_ref()).map_err(|_| {
                    alloy_rlp::Error::Custom("Failed to decode aura_seal as FixedBytes<65>")
                })?,
            );
        }

        if started_len - buf.len() < rlp_head.payload_length {
            this.base_fee_per_gas = Some(u64::decode(buf)?);
        }

        // Withdrawals root for post-shanghai headers
        if started_len - buf.len() < rlp_head.payload_length {
            this.withdrawals_root = Some(Decodable::decode(buf)?);
        }

        // Blob gas used and excess blob gas for post-cancun headers
        if started_len - buf.len() < rlp_head.payload_length {
            this.blob_gas_used = Some(u64::decode(buf)?);
        }

        if started_len - buf.len() < rlp_head.payload_length {
            this.excess_blob_gas = Some(u64::decode(buf)?);
        }

        // Decode parent beacon block root.
        if started_len - buf.len() < rlp_head.payload_length {
            this.parent_beacon_block_root = Some(B256::decode(buf)?);
        }

        // Decode requests hash.
        if started_len - buf.len() < rlp_head.payload_length {
            this.requests_hash = Some(B256::decode(buf)?);
        }

        // Decode block access list hash.
        if started_len - buf.len() < rlp_head.payload_length {
            this.block_access_list_hash = Some(B256::decode(buf)?);
        }

        // Decode slot number.
        if started_len - buf.len() < rlp_head.payload_length {
            this.slot_number = Some(u64::decode(buf)?);
        }

        let consumed = started_len - buf.len();
        if consumed != rlp_head.payload_length {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            });
        }
        Ok(this)
    }
}

// TODO TO DROP-V1 SUPPORT:
// remove `CompactHeaderV1` + the round-trip / fallback branch in `GnosisHeader::from_compact`,
// and revert `HeaderExt` to `#[derive(Compact)]` (the manual impl exists only
// to keep that decode path panic-free against legacy bytes).
//
// v0.2.x on-disk layout. `to_compact` always writes this; `from_compact`
// (on `GnosisHeader`) tries this first and falls back to `CompactHeaderV1`
// if it can't validate. New header fields must go inside `HeaderExt`, not
// here, so the 4-byte parent bitflag stays stable
// (paradigmxyz/reth#11166).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
struct CompactHeader {
    parent_hash: B256,
    ommers_hash: B256,
    beneficiary: Address,
    state_root: B256,
    transactions_root: B256,
    receipts_root: B256,
    withdrawals_root: Option<B256>,
    logs_bloom: Bloom,
    difficulty: U256,
    number: BlockNumber,
    gas_limit: u64,
    gas_used: u64,
    timestamp: u64,
    mix_hash: Option<B256>,
    nonce: Option<u64>,
    aura_step: Option<U256>,
    aura_seal: Option<FixedBytes<65>>,
    base_fee_per_gas: Option<u64>,
    blob_gas_used: Option<u64>,
    excess_blob_gas: Option<u64>,
    parent_beacon_block_root: Option<B256>,
    extra_fields: Option<HeaderExt>,
    extra_data: Bytes,
}

/// Extension slot for new header fields, hidden behind a single
/// `Option<HeaderExt>` bit on `CompactHeader` so the parent bitflag layout
/// stays frozen.
///
/// `Compact` is hand-written (rather than derived) so `from_compact`
/// validates lengths instead of panicking on malformed input. The wire
/// format is byte-identical to what the derive would emit (verified
/// empirically): a 1-byte bitflag (bit 0 = requests_hash, bit 1 =
/// block_access_list_hash, bit 2 = slot_number, bits 3-7 = 0), followed
/// by each `Some` field's payload in declaration order. Without this,
/// v0.1.x rows feeding garbage into `slot_number`'s u64-length varuint
/// would underflow inside `u64::from_compact`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
struct HeaderExt {
    requests_hash: Option<B256>,
    block_access_list_hash: Option<B256>,
    slot_number: Option<u64>,
}

impl HeaderExt {
    /// Canonicalise: all-`None` HeaderExt → `None`, so encoder output is
    /// deterministic (required for the re-encode check in
    /// `GnosisHeader::from_compact`).
    const fn into_option(self) -> Option<Self> {
        if self.requests_hash.is_some()
            || self.block_access_list_hash.is_some()
            || self.slot_number.is_some()
        {
            Some(self)
        } else {
            None
        }
    }
}

/// Following varuint functions are copied from reth

/// Mirror of reth-codecs' private `encode_varuint`.
fn encode_varuint_local<B: alloy_rlp::bytes::BufMut>(mut n: usize, buf: &mut B) {
    while n >= 0x80 {
        buf.put_u8((n as u8) | 0x80);
        n >>= 7;
    }
    buf.put_u8(n as u8);
}

/// Bytes a `usize` needs in the varuint encoding above.
fn varuint_len_local(mut n: usize) -> usize {
    let mut len = 1;
    while n >= 0x80 {
        len += 1;
        n >>= 7;
    }
    len
}

/// Bounds-checked counterpart to reth-codecs' `decode_varuint`. Returns
/// `None` on truncation or overflow instead of panicking.
fn decode_varuint_local(buf: &[u8]) -> Option<(usize, &[u8])> {
    let mut value: usize = 0;
    // usize is at most 64 bits → 10 varuint bytes covers any valid value.
    let max = buf.len().min(10);
    for i in 0..max {
        let byte = buf[i];
        let shift = i.checked_mul(7)?;
        if shift >= usize::BITS as usize {
            return None;
        }
        value |= (usize::from(byte & 0x7F)).checked_shl(shift as u32)?;
        if byte < 0x80 {
            return Some((value, &buf[i + 1..]));
        }
    }
    None
}

// This impl is same as paradigm's:
// reth_codecs_derive::compact::flags::build_struct_field_flags
// reth-codecs-0.3.1/src/lib.rs:302-365
impl reth_codecs::Compact for HeaderExt {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        let mut bitflag = 0u8;
        if self.requests_hash.is_some() {
            bitflag |= 0b001;
        }
        if self.block_access_list_hash.is_some() {
            bitflag |= 0b010;
        }
        if self.slot_number.is_some() {
            bitflag |= 0b100;
        }
        buf.put_u8(bitflag);
        let mut total = 1;

        if let Some(rh) = self.requests_hash {
            buf.put_slice(rh.as_slice());
            total += 32;
        }
        if let Some(bah) = self.block_access_list_hash {
            buf.put_slice(bah.as_slice());
            total += 32;
        }
        if let Some(sn) = self.slot_number {
            // Mirror `Option<u64>` generic encoding: varuint(byte-length) +
            // big-endian bytes with leading zeros stripped.
            let leading = (sn.leading_zeros() / 8) as usize;
            let length = 8 - leading;
            encode_varuint_local(length, buf);
            buf.put_slice(&sn.to_be_bytes()[leading..]);
            total += varuint_len_local(length) + length;
        }

        total
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        // Malformed-input policy: stop early and return the partial
        // `HeaderExt`. `GnosisHeader::from_compact`'s re-encode check
        // will reject the result and fall back to the v0.1.x decoder.
        let Some((&bitflag, mut buf)) = buf.split_first() else {
            return (HeaderExt::default(), buf);
        };
        let mut hx = HeaderExt::default();

        if bitflag & 0b001 != 0 {
            if buf.len() < 32 {
                return (hx, buf);
            }
            hx.requests_hash = Some(B256::from_slice(&buf[..32]));
            buf = &buf[32..];
        }
        if bitflag & 0b010 != 0 {
            if buf.len() < 32 {
                return (hx, buf);
            }
            hx.block_access_list_hash = Some(B256::from_slice(&buf[..32]));
            buf = &buf[32..];
        }
        if bitflag & 0b100 != 0 {
            let Some((length, rest)) = decode_varuint_local(buf) else {
                return (hx, buf);
            };
            if length > 8 || rest.len() < length {
                return (hx, buf);
            }
            let mut arr = [0u8; 8];
            arr[8 - length..].copy_from_slice(&rest[..length]);
            hx.slot_number = Some(u64::from_be_bytes(arr));
            buf = &rest[length..];
        }

        (hx, buf)
    }
}

// Legacy v0.1.x layout. Same parent-bitflag bit positions as
// `CompactHeader`, but the last bit's payload differs: v0.1.x writes 32
// raw bytes of `B256` (`requests_hash`); v0.2.x writes a varuint + a
// `HeaderExt` encoding. Fallback decoder for rows written before v0.2.x.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
struct CompactHeaderV1 {
    parent_hash: B256,
    ommers_hash: B256,
    beneficiary: Address,
    state_root: B256,
    transactions_root: B256,
    receipts_root: B256,
    withdrawals_root: Option<B256>,
    logs_bloom: Bloom,
    difficulty: U256,
    number: BlockNumber,
    gas_limit: u64,
    gas_used: u64,
    timestamp: u64,
    mix_hash: Option<B256>,
    nonce: Option<u64>,
    aura_step: Option<U256>,
    aura_seal: Option<FixedBytes<65>>,
    base_fee_per_gas: Option<u64>,
    blob_gas_used: Option<u64>,
    excess_blob_gas: Option<u64>,
    parent_beacon_block_root: Option<B256>,
    requests_hash: Option<B256>,
    extra_data: Bytes,
}

impl reth_codecs::Compact for GnosisHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        let extra_fields = HeaderExt {
            requests_hash: self.requests_hash,
            block_access_list_hash: self.block_access_list_hash,
            slot_number: self.slot_number,
        };
        let header = CompactHeader {
            parent_hash: self.parent_hash,
            ommers_hash: self.ommers_hash,
            beneficiary: self.beneficiary,
            state_root: self.state_root,
            transactions_root: self.transactions_root,
            receipts_root: self.receipts_root,
            withdrawals_root: self.withdrawals_root,
            logs_bloom: self.logs_bloom,
            difficulty: self.difficulty,
            number: self.number,
            gas_limit: self.gas_limit,
            gas_used: self.gas_used,
            timestamp: self.timestamp,
            mix_hash: self.mix_hash,
            nonce: self.nonce.map(Into::into),
            aura_step: self.aura_step,
            aura_seal: self.aura_seal,
            base_fee_per_gas: self.base_fee_per_gas,
            blob_gas_used: self.blob_gas_used,
            excess_blob_gas: self.excess_blob_gas,
            parent_beacon_block_root: self.parent_beacon_block_root,
            extra_fields: extra_fields.into_option(),
            extra_data: self.extra_data.clone(),
        };
        header.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        // Try v0.2.x first; fall back to v0.1.x if the result doesn't
        // round-trip. The v0.2.x decoder is panic-free because `HeaderExt`
        // has a hand-written `Compact` impl that bounds-checks (see
        // above) — it returns a partial `HeaderExt` on malformed input
        // rather than tripping the u64 length underflow that the
        // auto-derive would. The round-trip check catches both partial
        // decodes and legitimate-looking-but-wrong v0.1.x payloads.
        let (header, _) = CompactHeader::from_compact(buf, len);
        let alloy_header = Self {
            parent_hash: header.parent_hash,
            ommers_hash: header.ommers_hash,
            beneficiary: header.beneficiary,
            state_root: header.state_root,
            transactions_root: header.transactions_root,
            receipts_root: header.receipts_root,
            withdrawals_root: header.withdrawals_root,
            logs_bloom: header.logs_bloom,
            difficulty: header.difficulty,
            number: header.number,
            gas_limit: header.gas_limit,
            gas_used: header.gas_used,
            timestamp: header.timestamp,
            mix_hash: header.mix_hash,
            nonce: header.nonce.map(Into::into),
            aura_step: header.aura_step,
            aura_seal: header.aura_seal,
            base_fee_per_gas: header.base_fee_per_gas,
            blob_gas_used: header.blob_gas_used,
            excess_blob_gas: header.excess_blob_gas,
            parent_beacon_block_root: header.parent_beacon_block_root,
            requests_hash: header.extra_fields.as_ref().and_then(|h| h.requests_hash),
            block_access_list_hash: header
                .extra_fields
                .as_ref()
                .and_then(|h| h.block_access_list_hash),
            slot_number: header.extra_fields.as_ref().and_then(|h| h.slot_number),
            extra_data: header.extra_data,
        };

        // Round-trip check: canonical v0.2.x rows re-encode byte-for-byte;
        // a v0.1.x row that snuck through the v2 decoder won't.
        let mut reencoded = Vec::with_capacity(len);
        let reencoded_len = alloy_header.to_compact(&mut reencoded);
        if reencoded_len == len && reencoded.as_slice() == &buf[..len] {
            return (alloy_header, &buf[len..]);
        }

        // Fallback: v0.1.x layout. New fields default to `None`.
        let (header_v1, remaining) = CompactHeaderV1::from_compact(buf, len);
        let alloy_header = Self {
            parent_hash: header_v1.parent_hash,
            ommers_hash: header_v1.ommers_hash,
            beneficiary: header_v1.beneficiary,
            state_root: header_v1.state_root,
            transactions_root: header_v1.transactions_root,
            receipts_root: header_v1.receipts_root,
            withdrawals_root: header_v1.withdrawals_root,
            logs_bloom: header_v1.logs_bloom,
            difficulty: header_v1.difficulty,
            number: header_v1.number,
            gas_limit: header_v1.gas_limit,
            gas_used: header_v1.gas_used,
            timestamp: header_v1.timestamp,
            mix_hash: header_v1.mix_hash,
            nonce: header_v1.nonce.map(Into::into),
            aura_step: header_v1.aura_step,
            aura_seal: header_v1.aura_seal,
            base_fee_per_gas: header_v1.base_fee_per_gas,
            blob_gas_used: header_v1.blob_gas_used,
            excess_blob_gas: header_v1.excess_blob_gas,
            parent_beacon_block_root: header_v1.parent_beacon_block_root,
            requests_hash: header_v1.requests_hash,
            block_access_list_hash: None,
            slot_number: None,
            extra_data: header_v1.extra_data,
        };
        (alloy_header, remaining)
    }
}

impl Compress for GnosisHeader {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: alloy_primitives::bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        let _ = Compact::to_compact(self, buf);
    }
}

impl Decompress for GnosisHeader {
    fn decompress(value: &[u8]) -> Result<GnosisHeader, DecompressError> {
        let (obj, _) = Compact::from_compact(value, value.len());
        Ok(obj)
    }
}

impl HeaderMut for GnosisHeader {
    fn set_parent_hash(&mut self, hash: BlockHash) {
        self.parent_hash = hash;
    }

    fn set_block_number(&mut self, number: BlockNumber) {
        self.number = number;
    }

    fn set_timestamp(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
    }

    fn set_state_root(&mut self, state_root: B256) {
        self.state_root = state_root;
    }

    fn set_difficulty(&mut self, difficulty: U256) {
        self.difficulty = difficulty;
    }

    fn set_mix_hash(&mut self, mix_hash: B256) {
        self.mix_hash = Some(mix_hash);
    }

    fn set_extra_data(&mut self, extra_data: Bytes) {
        self.extra_data = extra_data;
    }

    fn set_parent_beacon_block_root(&mut self, parent_beacon_block_root: Option<B256>) {
        self.parent_beacon_block_root = parent_beacon_block_root;
    }
}

// tests
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, b256};

    fn get_sample_pre_merge_header() -> GnosisHeader {
        let sample_aura_seal: FixedBytes<65> = FixedBytes::from_slice(
            b"sample_aura_seal_000000000000000000000000000000000000000000000000",
        );
        GnosisHeader {
            parent_hash: B256::ZERO,
            ommers_hash: B256::ZERO,
            beneficiary: Address::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            logs_bloom: Bloom::default(),
            difficulty: U256::from(1000),
            number: 1,
            gas_limit: 1000000,
            gas_used: 500000,
            timestamp: 1622547800,
            extra_data: Bytes::from_static(b"extra data"),
            mix_hash: None,
            nonce: None,
            aura_step: Some(U256::from(1637394693478219i128)),
            aura_seal: Some(sample_aura_seal),
            base_fee_per_gas: Some(73468),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        }
    }

    fn get_sample_post_merge_header() -> GnosisHeader {
        GnosisHeader {
            parent_hash: B256::ZERO,
            ommers_hash: B256::ZERO,
            beneficiary: Address::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            logs_bloom: Bloom::default(),
            difficulty: U256::from(10000000),
            number: 1,
            gas_limit: 1000000,
            gas_used: 500000,
            timestamp: 1622547800,
            extra_data: Bytes::from_static(b"extra data"),
            mix_hash: Some(b256!(
                "661da523f3e44725f3a1cee38183d35424155a05674609a9f6ed81243adf9e26"
            )),
            nonce: Some(B64::from(938473940u64)),
            aura_step: None,
            aura_seal: None,
            base_fee_per_gas: Some(2374659),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        }
    }

    /// CI gate. The compact on-disk layout must stay byte-compatible with
    /// gnosis-primitives v0.1.x. The parent `CompactHeader` bitflag is
    /// exactly 4 bytes (32 bits, 0 unused); any addition that bumps this
    /// would silently break every existing database on next read. Add new
    /// header fields inside `HeaderExt` instead — see
    /// paradigmxyz/reth#11166 for the rationale.
    #[test]
    fn freeze_compact_header_bitflag_layout() {
        assert_eq!(
            CompactHeader::bitflag_encoded_bytes(),
            4,
            "CompactHeader bitflag size must not change — add new fields to HeaderExt instead",
        );
        assert_eq!(CompactHeader::bitflag_unused_bits(), 0);
        // HeaderExt's 1-byte bitflag is pinned separately by
        // `headerext_byte_layout_is_stable` against the exact wire bytes.
    }

    #[test]
    fn test_pre_merge_header_size() {
        let header = get_sample_pre_merge_header();
        println!("Header: {:?}", &header);
        assert_eq!(header.size(), 802); // Adjusted size based on fields
    }

    #[test]
    fn test_post_merge_header_size() {
        let header = get_sample_post_merge_header();
        assert_eq!(header.size(), 737); // Adjusted size based on fields
    }

    #[test]
    fn test_pre_merge_encode_decode() {
        // let header = get_sample_post_merge_header();
        let header = get_sample_pre_merge_header();
        let mut buf = Vec::new();
        header.encode(&mut buf);
        assert!(
            !buf.is_empty(),
            "Header encoding should produce non-empty output"
        );
        assert_eq!(
            buf.len(),
            header.length(),
            "Encoded length should match expected length"
        );

        // Decode the header back
        let mut buf_slice = &buf[..];
        let decoded_header = GnosisHeader::decode(&mut buf_slice).expect("Failed to decode header");
        println!("Decoded Header: {:?}", decoded_header);
        assert_eq!(
            decoded_header, header,
            "Decoded header should match original header"
        );
        // panic!("check")
    }

    #[test]
    fn test_post_merge_encode_decode() {
        let header = get_sample_post_merge_header();
        let mut buf = Vec::new();
        header.encode(&mut buf);
        assert!(
            !buf.is_empty(),
            "Header encoding should produce non-empty output"
        );
        assert_eq!(
            buf.len(),
            header.length(),
            "Encoded length should match expected length"
        );

        // Decode the header back
        let mut buf_slice = &buf[..];
        let decoded_header = GnosisHeader::decode(&mut buf_slice).expect("Failed to decode header");
        println!("Decoded Header: {:?}", decoded_header);
        assert_eq!(
            decoded_header, header,
            "Decoded header should match original header"
        );
        // panic!("check")
    }

    #[test]
    fn test_pre_merge_header_compact_decompact() {
        let header = get_sample_pre_merge_header();
        let mut buf = Vec::new();

        let compact_len = header.to_compact(&mut buf);
        assert!(
            compact_len > 0,
            "Compact encoding should produce non-empty output"
        );

        // Decode the header back
        let (decoded_header, _) = GnosisHeader::from_compact(&buf, compact_len);
        println!("Decoded Header: {:?}", decoded_header);
        assert_eq!(
            decoded_header, header,
            "Decoded header should match original header"
        );
    }

    #[test]
    fn test_post_merge_header_compact_decompact() {
        let header = get_sample_post_merge_header();
        let mut buf = Vec::new();

        let compact_len = header.to_compact(&mut buf);
        assert!(
            compact_len > 0,
            "Compact encoding should produce non-empty output"
        );

        // Decode the header back
        let (decoded_header, _) = GnosisHeader::from_compact(&buf, compact_len);
        println!("Decoded Header: {:?}", decoded_header);
        assert_eq!(
            decoded_header, header,
            "Decoded header should match original header"
        );
    }

    #[test]
    fn test_is_post_merge() {
        let post_merge_header = get_sample_post_merge_header();
        assert!(post_merge_header.is_post_merge());
        assert!(!post_merge_header.is_pre_merge());

        let pre_merge_header = get_sample_pre_merge_header();
        assert!(!pre_merge_header.is_post_merge());
        assert!(pre_merge_header.is_pre_merge());
    }

    #[test]
    fn test_is_pre_merge_edge_cases() {
        // Test header with only mix_hash (missing nonce) - should NOT be post-merge
        let mut header = get_sample_post_merge_header();
        header.nonce = None;
        assert!(!header.is_post_merge());

        // Test header with only nonce (missing mix_hash) - should NOT be post-merge
        let mut header = get_sample_post_merge_header();
        header.mix_hash = None;
        assert!(!header.is_post_merge());

        // Test header with only aura_step (missing aura_seal) - should NOT be pre-merge
        let mut header = get_sample_pre_merge_header();
        header.aura_seal = None;
        assert!(!header.is_pre_merge());

        // Test header with only aura_seal (missing aura_step) - should NOT be pre-merge
        let mut header = get_sample_pre_merge_header();
        header.aura_step = None;
        assert!(!header.is_pre_merge());

        // Test header with NEITHER pre-merge nor post-merge fields
        let mut header = get_sample_post_merge_header();
        header.mix_hash = None;
        header.nonce = None;
        header.aura_step = None;
        header.aura_seal = None;
        assert!(!header.is_post_merge());
        assert!(!header.is_pre_merge());
    }

    #[test]
    fn test_hash_slow() {
        let header = get_sample_post_merge_header();
        let hash1 = header.hash_slow();
        let hash2 = header.hash_slow();
        assert_eq!(hash1, hash2, "Hash should be deterministic");
        assert_ne!(hash1, B256::ZERO, "Hash should not be zero");
    }

    #[test]
    fn test_ommers_hash_is_empty() {
        let mut header = get_sample_post_merge_header();
        header.ommers_hash = EMPTY_OMMER_ROOT_HASH;
        assert!(header.ommers_hash_is_empty());

        header.ommers_hash = B256::ZERO;
        assert!(!header.ommers_hash_is_empty());
    }

    #[test]
    fn test_transaction_root_is_empty() {
        let mut header = get_sample_post_merge_header();
        header.transactions_root = EMPTY_ROOT_HASH;
        assert!(header.transaction_root_is_empty());

        header.transactions_root = B256::ZERO;
        assert!(!header.transaction_root_is_empty());
    }

    #[test]
    fn test_parent_num_hash() {
        // Test normal case: block 100 should have parent 99
        let mut header = get_sample_post_merge_header();
        header.number = 100;
        header.parent_hash = B256::from([0xAB; 32]);

        let parent = header.parent_num_hash();
        assert_eq!(parent.number, 99);
        assert_eq!(parent.hash, B256::from([0xAB; 32]));

        // Test genesis block edge case: block 0 parent is also 0 (saturating_sub)
        header.number = 0;
        header.parent_hash = B256::ZERO;
        let parent = header.parent_num_hash();
        assert_eq!(parent.number, 0);
        assert_eq!(parent.hash, B256::ZERO);
    }

    #[test]
    fn test_seal() {
        let header = get_sample_post_merge_header();
        let hash = B256::from([1u8; 32]);
        let sealed = header.clone().seal(hash);

        // Verify the hash is stored correctly
        assert_eq!(sealed.hash(), hash);

        // Verify the sealed header still contains the original header data
        assert_eq!(sealed.inner(), &header);
        assert_eq!(sealed.inner().number, header.number);
        assert_eq!(sealed.inner().parent_hash, header.parent_hash);
    }

    #[test]
    fn test_shanghai_active() {
        let mut header = get_sample_post_merge_header();
        header.withdrawals_root = None;
        assert!(!header.shanghai_active());

        header.withdrawals_root = Some(B256::ZERO);
        assert!(header.shanghai_active());
    }

    #[test]
    fn test_cancun_active() {
        let mut header = get_sample_post_merge_header();
        header.blob_gas_used = None;
        assert!(!header.cancun_active());

        header.blob_gas_used = Some(100);
        assert!(header.cancun_active());
    }

    #[test]
    fn test_prague_active() {
        let mut header = get_sample_post_merge_header();
        header.requests_hash = None;
        assert!(!header.prague_active());

        header.requests_hash = Some(B256::ZERO);
        assert!(header.prague_active());
    }

    #[test]
    fn test_header_equality() {
        let header1 = get_sample_post_merge_header();
        let header2 = get_sample_post_merge_header();
        assert_eq!(header1, header2);

        let header3 = get_sample_pre_merge_header();
        assert_ne!(header1, header3);
    }

    #[test]
    fn test_header_clone() {
        let header = get_sample_post_merge_header();
        let cloned = header.clone();
        assert_eq!(header, cloned);
    }

    #[test]
    fn test_header_default() {
        let header = GnosisHeader::default();
        assert_eq!(header.parent_hash, B256::ZERO);
        assert_eq!(header.number, 0);
        assert_eq!(header.gas_limit, 0);
    }

    #[test]
    fn test_from_alloy_header() {
        let alloy_header = Header {
            parent_hash: B256::ZERO,
            ommers_hash: B256::ZERO,
            beneficiary: Address::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            logs_bloom: Bloom::default(),
            difficulty: U256::from(1000),
            number: 1,
            gas_limit: 1000000,
            gas_used: 500000,
            timestamp: 1622547800,
            extra_data: Bytes::from_static(b"extra data"),
            mix_hash: b256!("661da523f3e44725f3a1cee38183d35424155a05674609a9f6ed81243adf9e26"),
            nonce: B64::from(938473940u64),
            base_fee_per_gas: Some(2374659),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        };

        let gnosis_header: GnosisHeader = alloy_header.clone().into();
        assert_eq!(gnosis_header.parent_hash, alloy_header.parent_hash);
        assert_eq!(gnosis_header.number, alloy_header.number);
        assert_eq!(gnosis_header.mix_hash, Some(alloy_header.mix_hash));
        assert_eq!(gnosis_header.nonce, Some(alloy_header.nonce));
        assert!(gnosis_header.aura_step.is_none());
        assert!(gnosis_header.aura_seal.is_none());
    }

    #[test]
    fn test_to_alloy_header() {
        let gnosis_header = get_sample_post_merge_header();
        let alloy_header = gnosis_header.to_alloy_header();
        assert_eq!(alloy_header.parent_hash, gnosis_header.parent_hash);
        assert_eq!(alloy_header.number, gnosis_header.number);
        assert_eq!(alloy_header.mix_hash, gnosis_header.mix_hash.unwrap());
        assert_eq!(alloy_header.nonce, gnosis_header.nonce.unwrap());
    }

    #[test]
    #[should_panic(expected = "GnosisHeader must have mix_hash and nonce set")]
    fn test_to_alloy_header_panics_without_mix_hash() {
        let mut header = get_sample_post_merge_header();
        header.mix_hash = None;
        header.to_alloy_header();
    }

    #[test]
    #[should_panic(expected = "GnosisHeader must have mix_hash and nonce set")]
    fn test_to_alloy_header_panics_without_nonce() {
        let mut header = get_sample_post_merge_header();
        header.nonce = None;
        header.to_alloy_header();
    }

    #[test]
    fn test_into_alloy_header() {
        let gnosis_header = get_sample_post_merge_header();
        let alloy_header: Header = gnosis_header.clone().into();
        assert_eq!(alloy_header.parent_hash, gnosis_header.parent_hash);
        assert_eq!(alloy_header.number, gnosis_header.number);
    }

    #[test]
    #[should_panic(expected = "GnosisHeader must have mix_hash and nonce set")]
    fn test_into_alloy_header_panics_pre_merge() {
        let pre_merge_header = get_sample_pre_merge_header();
        let _: Header = pre_merge_header.into();
    }

    #[test]
    fn test_block_header_trait_methods() {
        let header = get_sample_post_merge_header();

        assert_eq!(
            alloy_consensus::BlockHeader::parent_hash(&header),
            header.parent_hash
        );
        assert_eq!(
            alloy_consensus::BlockHeader::ommers_hash(&header),
            header.ommers_hash
        );
        assert_eq!(
            alloy_consensus::BlockHeader::beneficiary(&header),
            header.beneficiary
        );
        assert_eq!(
            alloy_consensus::BlockHeader::state_root(&header),
            header.state_root
        );
        assert_eq!(
            alloy_consensus::BlockHeader::transactions_root(&header),
            header.transactions_root
        );
        assert_eq!(
            alloy_consensus::BlockHeader::receipts_root(&header),
            header.receipts_root
        );
        assert_eq!(
            alloy_consensus::BlockHeader::logs_bloom(&header),
            header.logs_bloom
        );
        assert_eq!(
            alloy_consensus::BlockHeader::difficulty(&header),
            header.difficulty
        );
        assert_eq!(alloy_consensus::BlockHeader::number(&header), header.number);
        assert_eq!(
            alloy_consensus::BlockHeader::gas_limit(&header),
            header.gas_limit
        );
        assert_eq!(
            alloy_consensus::BlockHeader::gas_used(&header),
            header.gas_used
        );
        assert_eq!(
            alloy_consensus::BlockHeader::timestamp(&header),
            header.timestamp
        );
        assert_eq!(
            alloy_consensus::BlockHeader::mix_hash(&header),
            header.mix_hash
        );
        assert_eq!(alloy_consensus::BlockHeader::nonce(&header), header.nonce);
        assert_eq!(
            alloy_consensus::BlockHeader::base_fee_per_gas(&header),
            header.base_fee_per_gas
        );
        assert_eq!(
            alloy_consensus::BlockHeader::extra_data(&header),
            &header.extra_data
        );
    }

    #[test]
    fn test_next_block_base_fee() {
        let mut header = get_sample_post_merge_header();
        header.base_fee_per_gas = Some(1000);

        let base_fee_params = BaseFeeParams::ethereum();
        let next_base_fee = header.next_block_base_fee(base_fee_params);
        assert!(next_base_fee.is_some());

        header.base_fee_per_gas = None;
        assert!(header.next_block_base_fee(base_fee_params).is_none());
    }

    #[test]
    fn test_blob_fee() {
        let mut header = get_sample_post_merge_header();
        header.excess_blob_gas = Some(100000);

        let blob_params = BlobParams::cancun();
        let blob_fee = header.blob_fee(blob_params);
        assert!(blob_fee.is_some());

        header.excess_blob_gas = None;
        assert!(header.blob_fee(blob_params).is_none());
    }

    #[test]
    fn test_next_block_blob_fee() {
        let mut header = get_sample_post_merge_header();
        header.excess_blob_gas = Some(100000);
        header.blob_gas_used = Some(50000);

        let blob_params = BlobParams::cancun();
        let next_blob_fee = header.next_block_blob_fee(blob_params);
        assert!(next_blob_fee.is_some());
    }

    #[test]
    fn test_next_block_excess_blob_gas() {
        let mut header = get_sample_post_merge_header();
        header.excess_blob_gas = Some(100000);
        header.blob_gas_used = Some(50000);

        let blob_params = BlobParams::cancun();
        let next_excess = header.next_block_excess_blob_gas(blob_params);
        assert!(next_excess.is_some());

        header.excess_blob_gas = None;
        assert!(header.next_block_excess_blob_gas(blob_params).is_none());
    }

    #[test]
    fn test_header_with_all_eip_fields() {
        let header = GnosisHeader {
            parent_hash: B256::from([1u8; 32]),
            ommers_hash: B256::from([2u8; 32]),
            beneficiary: Address::from([3u8; 20]),
            state_root: B256::from([4u8; 32]),
            transactions_root: B256::from([5u8; 32]),
            receipts_root: B256::from([6u8; 32]),
            logs_bloom: Bloom::default(),
            difficulty: U256::from(1000),
            number: 100,
            gas_limit: 8000000,
            gas_used: 4000000,
            timestamp: 1622547800,
            extra_data: Bytes::from_static(b"test"),
            mix_hash: Some(B256::from([7u8; 32])),
            nonce: Some(B64::from(12345u64)),
            aura_step: None,
            aura_seal: None,
            base_fee_per_gas: Some(1000),
            withdrawals_root: Some(B256::from([8u8; 32])),
            blob_gas_used: Some(100000),
            excess_blob_gas: Some(50000),
            parent_beacon_block_root: Some(B256::from([9u8; 32])),
            requests_hash: Some(B256::from([10u8; 32])),
            block_access_list_hash: None,
            slot_number: None,
        };

        assert!(header.shanghai_active());
        assert!(header.cancun_active());
        assert!(header.prague_active());
        assert!(header.is_post_merge());

        // Test encoding/decoding
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_pre_merge_header_with_minimal_fields() {
        let mut seal_bytes = [0u8; 65];
        seal_bytes[..20].copy_from_slice(b"minimal_aura_seal_00");
        let sample_aura_seal: FixedBytes<65> = FixedBytes::from_slice(&seal_bytes);
        let header = GnosisHeader {
            parent_hash: B256::ZERO,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::ZERO,
            state_root: EMPTY_ROOT_HASH,
            transactions_root: EMPTY_ROOT_HASH,
            receipts_root: EMPTY_ROOT_HASH,
            logs_bloom: Bloom::default(),
            difficulty: U256::from(0),
            number: 0,
            gas_limit: 0,
            gas_used: 0,
            timestamp: 0,
            extra_data: Bytes::new(),
            mix_hash: None,
            nonce: None,
            aura_step: Some(U256::from(0)),
            aura_seal: Some(sample_aura_seal),
            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        };

        assert!(header.is_pre_merge());
        assert!(!header.is_post_merge());
        assert!(header.ommers_hash_is_empty());
        assert!(header.transaction_root_is_empty());

        // Test encoding/decoding
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_encode_decode_roundtrip_different_aura_steps() {
        // Test with small aura_step
        let mut header = get_sample_pre_merge_header();
        header.aura_step = Some(U256::from(1));
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);

        // Test with large aura_step
        header.aura_step = Some(U256::from(u128::MAX));
        buf.clear();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_as_ref() {
        let header = get_sample_post_merge_header();
        let header_ref: &GnosisHeader = header.as_ref();
        assert_eq!(&header, header_ref);
    }

    #[test]
    fn test_sealable_trait() {
        let header = get_sample_post_merge_header();
        let hash1 = Sealable::hash_slow(&header);
        let hash2 = header.hash_slow();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_header_payload_length_consistency() {
        let header = get_sample_post_merge_header();
        let payload_length = header.header_payload_length();

        let mut buf = Vec::new();
        header.encode(&mut buf);

        // The total length should be payload + length of length
        assert_eq!(buf.len(), header.length());
        assert!(payload_length > 0);
    }

    #[test]
    fn test_pre_merge_header_payload_length_consistency() {
        let header = get_sample_pre_merge_header();
        let payload_length = header.header_payload_length();

        let mut buf = Vec::new();
        header.encode(&mut buf);

        assert_eq!(buf.len(), header.length());
        assert!(payload_length > 0);
    }

    #[test]
    fn test_compress_decompress() {
        let header = get_sample_post_merge_header();
        let compressed = header.clone().compress();
        let decompressed = GnosisHeader::decompress(&compressed).unwrap();
        assert_eq!(header, decompressed);
    }

    #[test]
    fn test_compress_decompress_pre_merge() {
        let header = get_sample_pre_merge_header();
        let compressed = header.clone().compress();
        let decompressed = GnosisHeader::decompress(&compressed).unwrap();
        assert_eq!(header, decompressed);
    }

    #[test]
    fn test_header_hash_uniqueness() {
        let header1 = get_sample_post_merge_header();
        let mut header2 = get_sample_post_merge_header();

        assert_eq!(header1.hash_slow(), header2.hash_slow());

        header2.number += 1;
        assert_ne!(header1.hash_slow(), header2.hash_slow());
    }

    #[test]
    fn test_different_extra_data_sizes() {
        let mut header = get_sample_post_merge_header();

        // Empty extra data
        header.extra_data = Bytes::new();
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);

        // 32 bytes (max size)
        header.extra_data = Bytes::from(vec![0xFF; 32]);
        buf.clear();
        header.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_memory_size_calculation() {
        let post_merge = get_sample_post_merge_header();
        let pre_merge = get_sample_pre_merge_header();

        let post_size = post_merge.size();
        let pre_size = pre_merge.size();

        assert!(post_size > 0);
        assert!(pre_size > 0);
        // Pre-merge should generally be larger due to variable-length aura_seal
        assert!(pre_size > post_size);
    }

    #[test]
    fn test_hash_changes_with_different_fields() {
        let base = get_sample_post_merge_header();
        let base_hash = base.hash_slow();

        // Changing each field should produce a different hash
        let mut modified = base.clone();
        modified.parent_hash = B256::from([1u8; 32]);
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "parent_hash change should change hash"
        );

        let mut modified = base.clone();
        modified.beneficiary = Address::from([1u8; 20]);
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "beneficiary change should change hash"
        );

        let mut modified = base.clone();
        modified.number += 1;
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "number change should change hash"
        );

        let mut modified = base.clone();
        modified.timestamp += 1;
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "timestamp change should change hash"
        );

        let mut modified = base.clone();
        modified.gas_limit += 1;
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "gas_limit change should change hash"
        );

        let mut modified = base.clone();
        modified.extra_data = Bytes::from_static(b"different");
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "extra_data change should change hash"
        );

        let mut modified = base.clone();
        modified.mix_hash = Some(B256::from([2u8; 32]));
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "mix_hash change should change hash"
        );

        let mut modified = base.clone();
        modified.base_fee_per_gas = Some(999999);
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "base_fee_per_gas change should change hash"
        );
    }

    #[test]
    fn test_pre_merge_hash_changes_with_aura_fields() {
        let base = get_sample_pre_merge_header();
        let base_hash = base.hash_slow();

        // Changing aura_step should change hash
        let mut modified = base.clone();
        modified.aura_step = Some(U256::from(999999));
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "aura_step change should change hash"
        );

        // Changing aura_seal should change hash
        let mut modified = base.clone();
        let mut seal_bytes = [0u8; 65];
        seal_bytes[0] = 0xFF;
        modified.aura_seal = Some(FixedBytes::from_slice(&seal_bytes));
        assert_ne!(
            modified.hash_slow(),
            base_hash,
            "aura_seal change should change hash"
        );
    }

    #[test]
    fn test_encoding_differences_pre_vs_post_merge() {
        let pre = get_sample_pre_merge_header();
        let post = get_sample_post_merge_header();

        let mut pre_buf = Vec::new();
        pre.encode(&mut pre_buf);

        let mut post_buf = Vec::new();
        post.encode(&mut post_buf);

        // The encoded buffers should be different
        assert_ne!(
            pre_buf, post_buf,
            "Pre-merge and post-merge headers should encode differently"
        );

        // Verify we can decode them back correctly
        let pre_decoded = GnosisHeader::decode(&mut &pre_buf[..]).unwrap();
        assert!(pre_decoded.is_pre_merge());
        assert!(pre_decoded.aura_step.is_some());
        assert!(pre_decoded.aura_seal.is_some());

        let post_decoded = GnosisHeader::decode(&mut &post_buf[..]).unwrap();
        assert!(post_decoded.is_post_merge());
        assert!(post_decoded.mix_hash.is_some());
        assert!(post_decoded.nonce.is_some());
    }

    #[test]
    fn test_rlp_length_calculation_accuracy() {
        // Test that the length() method matches actual encoded length
        let headers = vec![
            get_sample_pre_merge_header(),
            get_sample_post_merge_header(),
        ];

        for header in headers {
            let mut buf = Vec::new();
            header.encode(&mut buf);

            assert_eq!(
                buf.len(),
                header.length(),
                "Calculated length should match actual encoded length"
            );
        }
    }

    #[test]
    fn test_blob_params_without_required_fields() {
        let mut header = get_sample_post_merge_header();

        // Remove blob gas fields
        header.blob_gas_used = None;
        header.excess_blob_gas = None;

        let blob_params = BlobParams::cancun();

        // These should return None when required fields are missing
        assert!(header.blob_fee(blob_params).is_none());
        assert!(header.next_block_blob_fee(blob_params).is_none());
        assert!(header.next_block_excess_blob_gas(blob_params).is_none());
    }

    #[test]
    fn test_decode_distinguishes_32byte_vs_variable_length() {
        // This is a CRITICAL test: the decoder peeks at the next RLP field
        // and decides if it's 32 bytes (post-merge mix_hash) or variable (pre-merge aura_step)

        // Create a pre-merge header with a small aura_step (will be < 32 bytes when RLP encoded)
        let mut pre_merge = get_sample_pre_merge_header();
        pre_merge.aura_step = Some(U256::from(42)); // Small value

        let mut buf = Vec::new();
        pre_merge.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();

        // Should be decoded as pre-merge
        assert!(decoded.is_pre_merge());
        assert_eq!(decoded.aura_step, Some(U256::from(42)));
        assert!(decoded.mix_hash.is_none());

        // Create a post-merge header - mix_hash is always 32 bytes
        let post_merge = get_sample_post_merge_header();
        buf.clear();
        post_merge.encode(&mut buf);
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();

        // Should be decoded as post-merge
        assert!(decoded.is_post_merge());
        assert!(decoded.aura_step.is_none());
        assert!(decoded.mix_hash.is_some());
    }

    #[test]
    fn test_mixed_consensus_header_encodes_as_post_merge() {
        // A header with BOTH consensus fields is weird but technically possible
        let mut header = get_sample_post_merge_header();

        // Add aura fields (this would be invalid in practice)
        let mut seal_bytes = [0u8; 65];
        seal_bytes[..20].copy_from_slice(b"sample_aura_seal_000");
        let sample_aura_seal: FixedBytes<65> = FixedBytes::from_slice(&seal_bytes);

        header.aura_step = Some(U256::from(123));
        header.aura_seal = Some(sample_aura_seal);

        // is_post_merge() returns true because mix_hash && nonce are Some
        assert!(header.is_post_merge());

        // When encoded, it should use post-merge path (mix_hash + nonce)
        // and IGNORE the aura fields
        let mut buf = Vec::new();
        header.encode(&mut buf);

        // When decoded, it should only have post-merge fields
        // The aura fields were NOT encoded, so they'll be None
        let decoded = GnosisHeader::decode(&mut &buf[..]).unwrap();
        assert!(decoded.is_post_merge());
        assert!(decoded.mix_hash.is_some());
        assert!(decoded.nonce.is_some());
        assert!(decoded.aura_step.is_none());
        assert!(decoded.aura_seal.is_none());
    }

    #[test]
    fn test_header_payload_length_differs_by_consensus_type() {
        let pre = get_sample_pre_merge_header();
        let post = get_sample_post_merge_header();

        let pre_len = pre.header_payload_length();
        let post_len = post.header_payload_length();

        // They should have different payload lengths because:
        // - pre-merge has aura_step (U256) and aura_seal (65 bytes)
        // - post-merge has mix_hash (32 bytes) and nonce (8 bytes)
        assert_ne!(
            pre_len, post_len,
            "Pre-merge and post-merge should have different payload lengths"
        );
    }

    #[test]
    fn test_encode_then_modify_doesnt_change_original() {
        let header = get_sample_post_merge_header();
        let original_hash = header.hash_slow();

        let mut buf = Vec::new();
        header.encode(&mut buf);

        // Verify encoding didn't mutate the header
        assert_eq!(header.hash_slow(), original_hash);
    }

    #[test]
    fn test_alloy_header_roundtrip_preserves_all_fields() {
        // Create a complex post-merge header with many EIP fields
        let mut original = get_sample_post_merge_header();
        original.parent_hash = B256::from([1u8; 32]);
        original.beneficiary = Address::from([2u8; 20]);
        original.number = 12345;
        original.base_fee_per_gas = Some(7890);
        original.withdrawals_root = Some(B256::from([3u8; 32]));
        original.blob_gas_used = Some(111);
        original.excess_blob_gas = Some(222);
        original.parent_beacon_block_root = Some(B256::from([4u8; 32]));
        original.requests_hash = Some(B256::from([5u8; 32]));

        // Convert to alloy and back
        let alloy: Header = original.clone().into();
        let back: GnosisHeader = alloy.into();

        // All fields should be preserved
        assert_eq!(back.parent_hash, original.parent_hash);
        assert_eq!(back.beneficiary, original.beneficiary);
        assert_eq!(back.number, original.number);
        assert_eq!(back.base_fee_per_gas, original.base_fee_per_gas);
        assert_eq!(back.withdrawals_root, original.withdrawals_root);
        assert_eq!(back.blob_gas_used, original.blob_gas_used);
        assert_eq!(back.excess_blob_gas, original.excess_blob_gas);
        assert_eq!(
            back.parent_beacon_block_root,
            original.parent_beacon_block_root
        );
        assert_eq!(back.requests_hash, original.requests_hash);
        assert_eq!(back.mix_hash, original.mix_hash);
        assert_eq!(back.nonce, original.nonce);

        // Aura fields should be None
        assert!(back.aura_step.is_none());
        assert!(back.aura_seal.is_none());
    }

    #[test]
    fn test_compact_encoding_is_deterministic() {
        let header = get_sample_post_merge_header();

        let mut buf1 = Vec::new();
        let len1 = header.to_compact(&mut buf1);

        let mut buf2 = Vec::new();
        let len2 = header.to_compact(&mut buf2);

        assert_eq!(len1, len2);
        assert_eq!(buf1, buf2, "Compact encoding should be deterministic");
    }

    #[test]
    fn test_size_increases_with_extra_data() {
        let mut header1 = get_sample_post_merge_header();
        header1.extra_data = Bytes::new();
        let size1 = header1.size();

        let mut header2 = get_sample_post_merge_header();
        header2.extra_data = Bytes::from(vec![0xFF; 32]);
        let size2 = header2.size();

        assert!(
            size2 > size1,
            "Header with more extra_data should have larger size"
        );
        assert_eq!(
            size2 - size1,
            32,
            "Size difference should equal extra_data length difference"
        );
    }

    #[test]
    fn test_header_set_number() {
        let mut header = get_sample_post_merge_header();
        header.set_number(42);
        assert_eq!(header.number, 42);
    }

    // Produces real v0.1.x bytes for a given GnosisHeader.
    fn v1_image(header: &GnosisHeader) -> CompactHeaderV1 {
        CompactHeaderV1 {
            parent_hash: header.parent_hash,
            ommers_hash: header.ommers_hash,
            beneficiary: header.beneficiary,
            state_root: header.state_root,
            transactions_root: header.transactions_root,
            receipts_root: header.receipts_root,
            withdrawals_root: header.withdrawals_root,
            logs_bloom: header.logs_bloom,
            difficulty: header.difficulty,
            number: header.number,
            gas_limit: header.gas_limit,
            gas_used: header.gas_used,
            timestamp: header.timestamp,
            mix_hash: header.mix_hash,
            nonce: header.nonce.map(|n| n.into()),
            aura_step: header.aura_step,
            aura_seal: header.aura_seal,
            base_fee_per_gas: header.base_fee_per_gas,
            blob_gas_used: header.blob_gas_used,
            excess_blob_gas: header.excess_blob_gas,
            parent_beacon_block_root: header.parent_beacon_block_root,
            requests_hash: header.requests_hash,
            extra_data: header.extra_data.clone(),
        }
    }

    /// Regression: v0.1.x rows with `requests_hash: Some` (every post-Prague
    /// Gnosis header) must decode without panicking after the v0.2.x bump.
    #[test]
    fn legacy_v1_post_prague_header_decodes() {
        // Realistic post-Prague header: `requests_hash` is Some (this is
        // the case that used to panic). New extension fields are absent,
        // because v0.1.x didn't know about them.
        let original = GnosisHeader {
            requests_hash: Some(b256!(
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
            )),
            block_access_list_hash: None,
            slot_number: None,
            ..get_sample_post_merge_header()
        };

        // Encode using the v0.1.x layout (CompactHeaderV1) to produce
        // bytes that match what a pre-v0.2 release would have written.
        let mut buf = Vec::new();
        let len = v1_image(&original).to_compact(&mut buf);

        let (decoded, remaining) = GnosisHeader::from_compact(&buf, len);
        assert!(
            remaining.is_empty(),
            "decoder over-/under-consumed: remaining = {} bytes",
            remaining.len(),
        );
        assert_eq!(decoded, original);
    }

    /// Stress test: `requests_hash = Some(B256::ZERO)` is the case where v0.2.x
    /// decodes *without* panicking but with the wrong fields (the leading
    /// `0x00` looks like a valid empty `HeaderExt` encoding). Pins the
    /// round-trip check — `consumed == len` alone wouldn't catch this.
    #[test]
    fn legacy_v1_header_with_zero_prefix_requests_hash() {
        let original = GnosisHeader {
            requests_hash: Some(B256::ZERO),
            block_access_list_hash: None,
            slot_number: None,
            ..get_sample_post_merge_header()
        };

        let mut buf = Vec::new();
        let len = v1_image(&original).to_compact(&mut buf);

        let (decoded, _) = GnosisHeader::from_compact(&buf, len);
        assert_eq!(
            decoded, original,
            "decoder must recover requests_hash; \
             v0.2.x layout misreads `Option<HeaderExt>` from leading 0x00 \
             bytes without panicking, so the fallback path needs a check \
             stronger than `consumed == len`",
        );
    }

    /// Pins the byte layout of `HeaderExt::to_compact` against the bytes
    /// the auto-derive used to emit. If the manual impl ever drifts from
    /// that layout, the next read against an existing static-file row
    /// will mis-decode silently — this test catches that before release.
    #[test]
    fn headerext_byte_layout_is_stable() {
        use reth_codecs::Compact;

        fn encode(h: HeaderExt) -> Vec<u8> {
            let mut buf = Vec::new();
            h.to_compact(&mut buf);
            buf
        }

        // bitflag 0x01 + 32-byte B256.
        let mut expected = vec![0x01];
        expected.extend_from_slice(&[0xaa; 32]);
        assert_eq!(
            encode(HeaderExt {
                requests_hash: Some(B256::from([0xaa; 32])),
                block_access_list_hash: None,
                slot_number: None,
            }),
            expected,
        );

        // bitflag 0x04 + varuint(1) + one u64 byte.
        assert_eq!(
            encode(HeaderExt {
                requests_hash: None,
                block_access_list_hash: None,
                slot_number: Some(42),
            }),
            vec![0x04, 0x01, 0x2a],
        );

        // bitflag 0x04 + varuint(8) + all 8 u64 bytes.
        assert_eq!(
            encode(HeaderExt {
                requests_hash: None,
                block_access_list_hash: None,
                slot_number: Some(0x1234_5678_90ab_cdef),
            }),
            vec![0x04, 0x08, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef],
        );

        // bitflag 0x07 + B256 + B256 + varuint(2) + 2 u64 bytes.
        let mut expected = vec![0x07];
        expected.extend_from_slice(&[0xaa; 32]);
        expected.extend_from_slice(&[0xbb; 32]);
        expected.extend_from_slice(&[0x02, 0xca, 0xfe]);
        assert_eq!(
            encode(HeaderExt {
                requests_hash: Some(B256::from([0xaa; 32])),
                block_access_list_hash: Some(B256::from([0xbb; 32])),
                slot_number: Some(0xcafe),
            }),
            expected,
        );
    }

    /// v0.2.x rows with every extension field populated (post-Osaka shape)
    /// must still round-trip via the v2 path.
    #[test]
    fn v2_header_with_all_extension_fields_roundtrips() {
        let original = GnosisHeader {
            requests_hash: Some(b256!(
                "1111111111111111111111111111111111111111111111111111111111111111"
            )),
            block_access_list_hash: Some(b256!(
                "2222222222222222222222222222222222222222222222222222222222222222"
            )),
            slot_number: Some(0x0123_4567_89ab_cdef),
            ..get_sample_post_merge_header()
        };

        let mut buf = Vec::new();
        let len = original.to_compact(&mut buf);

        let (decoded, remaining) = GnosisHeader::from_compact(&buf, len);
        assert!(remaining.is_empty());
        assert_eq!(decoded, original);
    }
}
