//! Strict codec for version `00` of the EEZ standardized blob message stream.
//!
//! The logical stream consists of an EIP-4844 blob portion followed by a
//! calldata tail. The blob portion starts with [`PROTOCOL_VERSION`], contains
//! exactly one [`Message::CloseBlobStream`], and is zero-padded after that
//! marker. The calldata tail resumes at the next message and must not contain a
//! second close marker.
//!
//! This crate deliberately contains no table or chain-operation policy. It is
//! the small, deterministic blob boundary shared by the composer, proof signer,
//! and deriver.

mod limits;
mod transport;

pub use limits::{BlobLimitError, BlobLimits};

pub use transport::{
    BlobSidecarEncoding, BlobTransactionParams, MAX_BLOBS_PER_TRANSACTION, PreparedBlobSidecar,
    PreparedBlobTransaction, SignedBlobTransaction, TransportError, blob_kzg_commitment_from_bytes,
    blob_kzg_commitments, blob_versioned_hash_from_bytes, blob_versioned_hashes,
    pooled_blob_transaction_hash, prepare_blob_sidecar, prepare_blob_sidecar_with_limit,
    prepare_blob_transaction, prepare_blob_transaction_with_sidecar, reprice_blob_transaction,
    sign_blob_transaction,
};

use alloy_primitives::{Address, Bytes, U256};

/// Version byte defined by the current protocol specification.
pub const PROTOCOL_VERSION: u8 = 0;
/// Maximum context-stack and snapshot-stack depth in the reference protocol.
pub const MAX_CALL_DEPTH: usize = 64;
/// Number of field elements in an EIP-4844 blob.
pub const FIELD_ELEMENTS_PER_BLOB: usize = 4096;
/// Serialized bytes in one field element.
pub const FIELD_ELEMENT_BYTES: usize = 32;
/// Stream bytes carried by one field element.
pub const DATA_BYTES_PER_FIELD_ELEMENT: usize = 31;
/// Physical bytes in an EIP-4844 blob.
pub const BLOB_BYTES: usize = FIELD_ELEMENTS_PER_BLOB * FIELD_ELEMENT_BYTES;
/// Useful stream bytes carried by one blob under the EEZ packing.
pub const DATA_BYTES_PER_BLOB: usize = FIELD_ELEMENTS_PER_BLOB * DATA_BYTES_PER_FIELD_ELEMENT;

/// One typed message in the version-`00` logical stream.
///
/// A typed enum prevents callers from populating fields that do not exist on
/// the wire for a particular message type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// End of meaningful bytes in the blob portion.
    CloseBlobStream,
    /// Chain-local opaque operations.
    ChainOperation { chain_id: u64, operations: Bytes },
    /// Start of one cross-chain transaction.
    InitiateCrossChainTransaction { chain_id: u64, tx_data: Bytes },
    /// Mutable cross-chain call.
    Call {
        to_chain: u64,
        from_address: Address,
        to_address: Address,
        value: U256,
        gas: u64,
        data: Bytes,
    },
    /// Read-only cross-chain call.
    StaticCall {
        to_chain: u64,
        from_address: Address,
        to_address: Address,
        gas: u64,
        data: Bytes,
    },
    /// Successful result of the most recent open call.
    ReturnSuccess { return_data: Bytes },
    /// Reverted result of the most recent open call.
    ReturnFail { return_data: Bytes },
    /// Start of a forced-revert region.
    Snapshot,
    /// End of a forced-revert region.
    Revert,
    /// End of the open cross-chain transaction.
    FinishCrossChainTransaction,
}

impl Message {
    const fn type_byte(&self) -> u8 {
        match self {
            Self::CloseBlobStream => 1,
            Self::ChainOperation { .. } => 2,
            Self::InitiateCrossChainTransaction { .. } => 3,
            Self::Call { .. } => 4,
            Self::StaticCall { .. } => 5,
            Self::ReturnSuccess { .. } => 6,
            Self::ReturnFail { .. } => 7,
            Self::Snapshot => 8,
            Self::Revert => 9,
            Self::FinishCrossChainTransaction => 10,
        }
    }
}

/// The two physical segments of one validated logical message stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedStream {
    /// Version byte, messages through `CloseBlobStream`, and no padding.
    pub blob_portion: Bytes,
    /// Messages after `CloseBlobStream`.
    pub call_data_tail: Bytes,
}

/// A fully sized serialized EIP-4844 blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob(Box<[u8]>);

impl Blob {
    /// Validate and copy one serialized blob.
    pub fn try_from_slice(blob_index: usize, bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != BLOB_BYTES {
            return Err(Error::InvalidBlobLength {
                blob_index,
                expected: BLOB_BYTES,
                actual: bytes.len(),
            });
        }
        let physical = bytes.to_vec().into_boxed_slice();
        validate_field_elements(blob_index, &physical)?;
        Ok(Self(physical))
    }

    /// Borrow the physical blob bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume the wrapper and return the physical blob.
    pub fn into_bytes(self) -> Box<[u8]> {
        self.0
    }
}

impl AsRef<[u8]> for Blob {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Codec and packing failures. Every error rejects the whole logical stream.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("encoded stream length exceeds addressable memory")]
    LengthOverflow,
    #[error("blob message stream is empty")]
    EmptyStream,
    #[error("unknown blob protocol version {0:#04x}")]
    UnknownProtocolVersion(u8),
    #[error("invalid blob message type {0}")]
    InvalidMessageType(u8),
    #[error("blob message stream is truncated")]
    TruncatedStream,
    #[error("length varint uses more than five bytes")]
    VarintTooLong,
    #[error("length varint exceeds u32::MAX")]
    VarintOverflow,
    #[error("payload length {0} exceeds u32::MAX")]
    PayloadTooLong(usize),
    #[error("CloseBlobStream is missing from the blob portion")]
    CloseBlobStreamMissing,
    #[error("CloseBlobStream appears more than once")]
    DuplicateCloseBlobStream,
    #[error("CloseBlobStream appears in the calldata tail")]
    CloseBlobStreamInCallData,
    #[error("non-zero blob padding at decoded stream offset {offset}")]
    NonZeroPadding { offset: usize },
    #[error("a cross-chain transaction cannot be nested")]
    NestedTransaction,
    #[error("message type {message_type} appears outside a cross-chain transaction")]
    MessageOutsideTransaction { message_type: u8 },
    #[error("ChainOperation appears inside a cross-chain transaction")]
    ChainOperationInsideTransaction,
    #[error("a return appears without an open call")]
    ReturnWithoutOpenCall,
    #[error("a return crosses an open Snapshot boundary")]
    ReturnCrossesSnapshot,
    #[error("Revert appears without an open Snapshot")]
    RevertWithoutSnapshot,
    #[error("Revert appears while a call opened after its Snapshot is still open")]
    RevertWithOpenCall,
    #[error("FinishCrossChainTransaction appears with an open call")]
    FinishWithOpenCall,
    #[error("FinishCrossChainTransaction appears with an open Snapshot")]
    FinishWithOpenSnapshot,
    #[error("the message stream ends with an unclosed bracket")]
    UnclosedBracket,
    #[error("the maximum call or snapshot depth of {MAX_CALL_DEPTH} was exceeded")]
    DepthLimitExceeded,
    #[error("cannot pack an empty blob stream")]
    EmptyBlobStream,
    #[error("blob {blob_index} has {actual} bytes; expected {expected}")]
    InvalidBlobLength {
        blob_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error("blob {blob_index} field element {element_index} has a non-zero most-significant byte")]
    InvalidFieldElement {
        blob_index: usize,
        element_index: usize,
    },
}

/// Encode and structurally validate a message list.
///
/// Honest encoders emit minimal protobuf varints. Decoders also accept
/// non-minimal encodings, as required by the protocol specification.
pub fn encode(messages: &[Message]) -> Result<EncodedStream, Error> {
    validate_messages(messages)?;

    let mut blob_portion = Vec::new();
    let mut call_data_tail = Vec::new();
    blob_portion.push(PROTOCOL_VERSION);
    let mut closed = false;

    for message in messages {
        let destination = if closed {
            &mut call_data_tail
        } else {
            &mut blob_portion
        };
        encode_one(message, destination)?;
        if matches!(message, Message::CloseBlobStream) {
            closed = true;
        }
    }

    Ok(EncodedStream {
        blob_portion: blob_portion.into(),
        call_data_tail: call_data_tail.into(),
    })
}

/// Decode and validate the complete logical stream.
pub fn decode(blob_portion: &[u8], call_data_tail: &[u8]) -> Result<Vec<Message>, Error> {
    let Some((&version, _)) = blob_portion.split_first() else {
        return Err(Error::EmptyStream);
    };
    if version != PROTOCOL_VERSION {
        return Err(Error::UnknownProtocolVersion(version));
    }

    let mut state = DecodeState::default();
    decode_segment(&mut state, blob_portion, 1, Segment::Blob)?;
    if !state.close_seen {
        return Err(Error::CloseBlobStreamMissing);
    }
    decode_segment(&mut state, call_data_tail, 0, Segment::CallData)?;
    if !state.context.is_empty() || !state.snapshot_depths.is_empty() {
        return Err(Error::UnclosedBracket);
    }
    Ok(state.messages)
}

/// Pack the unpadded blob portion into full-size serialized EIP-4844 blobs.
///
/// The EEZ stream is assigned from the least-significant byte of each field
/// element upward. Ethereum serializes that field element in big-endian order,
/// so the unused most-significant zero is raw byte `0`, and the 31 stream bytes
/// occupy raw bytes `31..=1` in reverse order.
pub fn pack(blob_portion: &[u8]) -> Result<Vec<Blob>, Error> {
    if blob_portion.is_empty() {
        return Err(Error::EmptyBlobStream);
    }

    let blob_count = blob_portion.len().div_ceil(DATA_BYTES_PER_BLOB);
    let mut blobs = Vec::with_capacity(blob_count);
    let mut source = 0;
    for _ in 0..blob_count {
        let mut physical = vec![0_u8; BLOB_BYTES].into_boxed_slice();
        for element in 0..FIELD_ELEMENTS_PER_BLOB {
            let destination = element * FIELD_ELEMENT_BYTES;
            let remaining = blob_portion.len().saturating_sub(source);
            let count = remaining.min(DATA_BYTES_PER_FIELD_ELEMENT);
            if count == 0 {
                break;
            }
            for stream_offset in 0..count {
                physical[destination + FIELD_ELEMENT_BYTES - 1 - stream_offset] =
                    blob_portion[source + stream_offset];
            }
            source += count;
        }
        blobs.push(Blob(physical));
    }
    Ok(blobs)
}

/// Unpack and validate full-size serialized EIP-4844 blobs.
///
/// The returned stream includes zero padding after `CloseBlobStream`; [`decode`]
/// validates that padding and ignores it.
pub fn unpack(blobs: &[Blob]) -> Result<Bytes, Error> {
    let mut stream = Vec::with_capacity(blobs.len().saturating_mul(DATA_BYTES_PER_BLOB));
    for (blob_index, blob) in blobs.iter().enumerate() {
        validate_field_elements(blob_index, blob.as_bytes())?;
        for element in blob.as_bytes().as_chunks::<FIELD_ELEMENT_BYTES>().0 {
            stream.extend(element[1..].iter().rev());
        }
    }
    Ok(stream.into())
}

/// Unpack physical blobs, then decode the complete logical stream.
pub fn decode_blobs(blobs: &[Blob], call_data_tail: &[u8]) -> Result<Vec<Message>, Error> {
    let blob_portion = unpack(blobs)?;
    decode(&blob_portion, call_data_tail)
}

/// One native EEZ blob payload: chain-local synchronization data plus the
/// standardized cross-chain semantic messages that authorize its effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePayload {
    /// Opaque, chain-defined data needed to synchronize `chain_id`.
    pub operations: Bytes,
    /// Complete semantic transaction brackets, without carrier markers.
    pub semantic_messages: Vec<Message>,
}

/// Encode the native deployment profile entirely into EIP-4844 blobs.
///
/// The profile is deliberately narrow and deterministic: exactly one leading
/// [`Message::ChainOperation`], zero or more complete cross-chain transaction
/// brackets, then [`Message::CloseBlobStream`]. `semantic_messages` must not
/// contain either carrier marker.
pub fn encode_native_payload(
    chain_id: u64,
    operations: Bytes,
    semantic_messages: Vec<Message>,
) -> Result<Vec<Blob>, NativeProfileError> {
    let encoded = encode_native_stream(chain_id, operations, semantic_messages)?;
    Ok(pack(&encoded.blob_portion)?)
}

/// Build the exact unpadded native stream. Packing callers can check actual
/// meaningful byte capacity before allocating physical blobs or KZG material.
pub fn encode_native_stream(
    chain_id: u64,
    operations: Bytes,
    semantic_messages: Vec<Message>,
) -> Result<EncodedStream, NativeProfileError> {
    if semantic_messages.iter().any(|message| {
        matches!(
            message,
            Message::ChainOperation { .. } | Message::CloseBlobStream
        )
    }) {
        return Err(NativeProfileError::UnexpectedMessages);
    }
    let mut messages = Vec::with_capacity(semantic_messages.len() + 2);
    messages.push(Message::ChainOperation {
        chain_id,
        operations,
    });
    messages.extend(semantic_messages);
    messages.push(Message::CloseBlobStream);
    let encoded = encode(&messages)?;
    debug_assert!(encoded.call_data_tail.is_empty());
    Ok(encoded)
}

/// Exact meaningful bytes in the native blob stream, before field packing and
/// padding. `semantic_bytes` is the encoded length of complete semantic
/// messages. This includes the version, operation type/id/length, and closing
/// marker even when all operation data bytes are zero.
pub fn native_stream_len(operations_bytes: usize, semantic_bytes: usize) -> Result<usize, Error> {
    let mut length =
        u32::try_from(operations_bytes).map_err(|_| Error::PayloadTooLong(operations_bytes))?;
    let mut varint_bytes = 1usize;
    while length >= 0x80 {
        length >>= 7;
        varint_bytes += 1;
    }
    // version + ChainOperation type + chain_id + length + close marker
    operations_bytes
        .checked_add(semantic_bytes)
        .and_then(|bytes| bytes.checked_add(1 + 1 + size_of::<u64>() + varint_bytes + 1))
        .ok_or(Error::LengthOverflow)
}

/// Decode and validate the native deployment profile.
///
/// The underlying codec validates all transaction, call, return, snapshot,
/// and revert brackets before this function returns the semantic messages.
pub fn decode_native_payload(
    blobs: &[Blob],
    expected_chain_id: u64,
) -> Result<NativePayload, NativeProfileError> {
    let messages = decode_blobs(blobs, &[])?;
    let Some((first, remainder)) = messages.split_first() else {
        return Err(NativeProfileError::UnexpectedMessages);
    };
    let Some((last, semantic_messages)) = remainder.split_last() else {
        return Err(NativeProfileError::UnexpectedMessages);
    };
    let Message::ChainOperation {
        chain_id,
        operations,
    } = first
    else {
        return Err(NativeProfileError::UnexpectedMessages);
    };
    if !matches!(last, Message::CloseBlobStream)
        || semantic_messages.iter().any(|message| {
            matches!(
                message,
                Message::ChainOperation { .. } | Message::CloseBlobStream
            )
        })
    {
        return Err(NativeProfileError::UnexpectedMessages);
    }
    if *chain_id != expected_chain_id {
        return Err(NativeProfileError::ChainIdMismatch {
            expected: expected_chain_id,
            actual: *chain_id,
        });
    }
    Ok(NativePayload {
        operations: operations.clone(),
        semantic_messages: semantic_messages.to_vec(),
    })
}

/// Native-profile encoding or decoding failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NativeProfileError {
    /// The underlying blob stream is malformed.
    #[error(transparent)]
    Codec(#[from] Error),
    /// The stream is valid version `00`, but is outside the native profile.
    #[error(
        "blob stream must contain one leading ChainOperation, complete semantic transactions, and a trailing CloseBlobStream"
    )]
    UnexpectedMessages,
    /// The opaque synchronization payload belongs to another rollup.
    #[error("ChainOperation chain id {actual} does not match expected rollup {expected}")]
    ChainIdMismatch { expected: u64, actual: u64 },
}

#[derive(Clone, Copy)]
enum Segment {
    Blob,
    CallData,
}

#[derive(Default)]
struct DecodeState {
    messages: Vec<Message>,
    context: Vec<u64>,
    snapshot_depths: Vec<usize>,
    close_seen: bool,
}

fn decode_segment(
    state: &mut DecodeState,
    segment: &[u8],
    mut position: usize,
    kind: Segment,
) -> Result<(), Error> {
    while position < segment.len() {
        let message_type = read_u8(segment, &mut position)?;
        if !(1..=10).contains(&message_type) {
            return Err(Error::InvalidMessageType(message_type));
        }
        if message_type == 1 {
            if matches!(kind, Segment::CallData) {
                return Err(Error::CloseBlobStreamInCallData);
            }
            if state.close_seen {
                return Err(Error::DuplicateCloseBlobStream);
            }
            state.close_seen = true;
            state.messages.push(Message::CloseBlobStream);
            if let Some(relative) = segment[position..].iter().position(|byte| *byte != 0) {
                return Err(Error::NonZeroPadding {
                    offset: position + relative,
                });
            }
            return Ok(());
        }

        let message = decode_one(message_type, segment, &mut position)?;
        apply_message(state, &message)?;
        state.messages.push(message);
    }
    Ok(())
}

fn decode_one(message_type: u8, input: &[u8], position: &mut usize) -> Result<Message, Error> {
    match message_type {
        2 => Ok(Message::ChainOperation {
            chain_id: read_u64_le(input, position)?,
            operations: read_bytes(input, position)?,
        }),
        3 => Ok(Message::InitiateCrossChainTransaction {
            chain_id: read_u64_le(input, position)?,
            tx_data: read_bytes(input, position)?,
        }),
        4 => Ok(Message::Call {
            to_chain: read_u64_le(input, position)?,
            from_address: read_address(input, position)?,
            to_address: read_address(input, position)?,
            value: read_u256_le(input, position)?,
            gas: read_u64_le(input, position)?,
            data: read_bytes(input, position)?,
        }),
        5 => Ok(Message::StaticCall {
            to_chain: read_u64_le(input, position)?,
            from_address: read_address(input, position)?,
            to_address: read_address(input, position)?,
            gas: read_u64_le(input, position)?,
            data: read_bytes(input, position)?,
        }),
        6 => Ok(Message::ReturnSuccess {
            return_data: read_bytes(input, position)?,
        }),
        7 => Ok(Message::ReturnFail {
            return_data: read_bytes(input, position)?,
        }),
        8 => Ok(Message::Snapshot),
        9 => Ok(Message::Revert),
        10 => Ok(Message::FinishCrossChainTransaction),
        _ => Err(Error::InvalidMessageType(message_type)),
    }
}

fn encode_one(message: &Message, output: &mut Vec<u8>) -> Result<(), Error> {
    output.push(message.type_byte());
    match message {
        Message::ChainOperation {
            chain_id,
            operations,
        } => {
            output.extend_from_slice(&chain_id.to_le_bytes());
            write_bytes(operations, output)?;
        }
        Message::InitiateCrossChainTransaction { chain_id, tx_data } => {
            output.extend_from_slice(&chain_id.to_le_bytes());
            write_bytes(tx_data, output)?;
        }
        Message::Call {
            to_chain,
            from_address,
            to_address,
            value,
            gas,
            data,
        } => {
            output.extend_from_slice(&to_chain.to_le_bytes());
            output.extend_from_slice(from_address.as_slice());
            output.extend_from_slice(to_address.as_slice());
            output.extend_from_slice(&value.to_le_bytes::<32>());
            output.extend_from_slice(&gas.to_le_bytes());
            write_bytes(data, output)?;
        }
        Message::StaticCall {
            to_chain,
            from_address,
            to_address,
            gas,
            data,
        } => {
            output.extend_from_slice(&to_chain.to_le_bytes());
            output.extend_from_slice(from_address.as_slice());
            output.extend_from_slice(to_address.as_slice());
            output.extend_from_slice(&gas.to_le_bytes());
            write_bytes(data, output)?;
        }
        Message::ReturnSuccess { return_data } | Message::ReturnFail { return_data } => {
            write_bytes(return_data, output)?;
        }
        Message::CloseBlobStream
        | Message::Snapshot
        | Message::Revert
        | Message::FinishCrossChainTransaction => {}
    }
    Ok(())
}

fn validate_messages(messages: &[Message]) -> Result<(), Error> {
    let mut state = DecodeState::default();
    let mut in_tail = false;
    for message in messages {
        if matches!(message, Message::CloseBlobStream) {
            if in_tail {
                return Err(Error::DuplicateCloseBlobStream);
            }
            state.close_seen = true;
            state.messages.push(message.clone());
            in_tail = true;
            continue;
        }
        apply_message(&mut state, message)?;
    }
    if !state.close_seen {
        return Err(Error::CloseBlobStreamMissing);
    }
    if !state.context.is_empty() || !state.snapshot_depths.is_empty() {
        return Err(Error::UnclosedBracket);
    }
    Ok(())
}

fn apply_message(state: &mut DecodeState, message: &Message) -> Result<(), Error> {
    match message {
        Message::CloseBlobStream => {}
        Message::ChainOperation { .. } => {
            if !state.context.is_empty() {
                return Err(Error::ChainOperationInsideTransaction);
            }
        }
        Message::InitiateCrossChainTransaction { chain_id, .. } => {
            if !state.context.is_empty() {
                return Err(Error::NestedTransaction);
            }
            push_context(state, *chain_id)?;
        }
        Message::Call { to_chain, .. } | Message::StaticCall { to_chain, .. } => {
            if state.context.is_empty() {
                return Err(Error::MessageOutsideTransaction {
                    message_type: message.type_byte(),
                });
            }
            push_context(state, *to_chain)?;
        }
        Message::ReturnSuccess { .. } | Message::ReturnFail { .. } => {
            if state.context.len() < 2 {
                return Err(Error::ReturnWithoutOpenCall);
            }
            if state
                .snapshot_depths
                .last()
                .is_some_and(|snapshot_depth| state.context.len() - 1 < *snapshot_depth)
            {
                return Err(Error::ReturnCrossesSnapshot);
            }
            state.context.pop();
        }
        Message::Snapshot => {
            if state.context.is_empty() {
                return Err(Error::MessageOutsideTransaction {
                    message_type: message.type_byte(),
                });
            }
            if state.snapshot_depths.len() == MAX_CALL_DEPTH {
                return Err(Error::DepthLimitExceeded);
            }
            state.snapshot_depths.push(state.context.len());
        }
        Message::Revert => {
            let Some(snapshot_depth) = state.snapshot_depths.last().copied() else {
                return Err(Error::RevertWithoutSnapshot);
            };
            if state.context.len() != snapshot_depth {
                return Err(Error::RevertWithOpenCall);
            }
            state.snapshot_depths.pop();
        }
        Message::FinishCrossChainTransaction => {
            if state.context.is_empty() {
                return Err(Error::MessageOutsideTransaction {
                    message_type: message.type_byte(),
                });
            }
            if state.context.len() > 1 {
                return Err(Error::FinishWithOpenCall);
            }
            if !state.snapshot_depths.is_empty() {
                return Err(Error::FinishWithOpenSnapshot);
            }
            state.context.pop();
        }
    }
    Ok(())
}

fn push_context(state: &mut DecodeState, chain_id: u64) -> Result<(), Error> {
    if state.context.len() == MAX_CALL_DEPTH {
        return Err(Error::DepthLimitExceeded);
    }
    state.context.push(chain_id);
    Ok(())
}

fn write_bytes(bytes: &[u8], output: &mut Vec<u8>) -> Result<(), Error> {
    let length = u32::try_from(bytes.len()).map_err(|_| Error::PayloadTooLong(bytes.len()))?;
    write_varint(length, output);
    output.extend_from_slice(bytes);
    Ok(())
}

fn write_varint(mut value: u32, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_bytes(input: &[u8], position: &mut usize) -> Result<Bytes, Error> {
    let length = read_varint(input, position)? as usize;
    let end = position.checked_add(length).ok_or(Error::TruncatedStream)?;
    let value = input.get(*position..end).ok_or(Error::TruncatedStream)?;
    *position = end;
    Ok(Bytes::copy_from_slice(value))
}

fn read_varint(input: &[u8], position: &mut usize) -> Result<u32, Error> {
    let mut value = 0_u32;
    for byte_index in 0..5 {
        let byte = read_u8(input, position)?;
        if byte_index == 4 && byte & 0xf0 != 0 {
            return Err(Error::VarintOverflow);
        }
        value |= u32::from(byte & 0x7f) << (7 * byte_index);
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(Error::VarintTooLong)
}

fn read_u8(input: &[u8], position: &mut usize) -> Result<u8, Error> {
    let value = *input.get(*position).ok_or(Error::TruncatedStream)?;
    *position += 1;
    Ok(value)
}

fn read_exact<const N: usize>(input: &[u8], position: &mut usize) -> Result<[u8; N], Error> {
    let end = position.checked_add(N).ok_or(Error::TruncatedStream)?;
    let bytes: [u8; N] = input
        .get(*position..end)
        .ok_or(Error::TruncatedStream)?
        .try_into()
        .map_err(|_| Error::TruncatedStream)?;
    *position = end;
    Ok(bytes)
}

fn read_u64_le(input: &[u8], position: &mut usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(read_exact(input, position)?))
}

fn read_u256_le(input: &[u8], position: &mut usize) -> Result<U256, Error> {
    Ok(U256::from_le_bytes(read_exact::<32>(input, position)?))
}

fn read_address(input: &[u8], position: &mut usize) -> Result<Address, Error> {
    Ok(Address::from(read_exact(input, position)?))
}

fn validate_field_elements(blob_index: usize, blob: &[u8]) -> Result<(), Error> {
    for (element_index, element) in blob.as_chunks::<FIELD_ELEMENT_BYTES>().0.iter().enumerate() {
        if element[0] != 0 {
            return Err(Error::InvalidFieldElement {
                blob_index,
                element_index,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes};

    fn complete_flow() -> Vec<Message> {
        vec![
            Message::ChainOperation {
                chain_id: 7,
                operations: bytes!("deadbeef"),
            },
            Message::InitiateCrossChainTransaction {
                chain_id: 7,
                tx_data: Bytes::from_static(b"rlp-tx"),
            },
            Message::Call {
                to_chain: 8,
                from_address: address!("1111111111111111111111111111111111111111"),
                to_address: address!("2222222222222222222222222222222222222222"),
                value: U256::from(1_000_000_000_000_000_000_u64),
                gas: 250_000,
                data: Bytes::from_static(b"bounded"),
            },
            Message::StaticCall {
                to_chain: 0,
                from_address: address!("2222222222222222222222222222222222222222"),
                to_address: address!("3333333333333333333333333333333333333333"),
                gas: 30_000,
                data: Bytes::from_static(b"read"),
            },
            Message::ReturnSuccess {
                return_data: Bytes::from_static(b"read-result"),
            },
            Message::ReturnFail {
                return_data: Bytes::from_static(b"revert-payload"),
            },
            Message::Snapshot,
            Message::Call {
                to_chain: 8,
                from_address: address!("1111111111111111111111111111111111111111"),
                to_address: address!("2222222222222222222222222222222222222222"),
                value: U256::ZERO,
                gas: 0,
                data: Bytes::from_static(b"reverted-later"),
            },
            Message::ReturnSuccess {
                return_data: Bytes::new(),
            },
            Message::Revert,
            Message::FinishCrossChainTransaction,
            Message::CloseBlobStream,
            Message::ChainOperation {
                chain_id: 7,
                operations: Bytes::from_static(b"tail"),
            },
        ]
    }

    #[test]
    fn round_trip_all_message_shapes_and_tail() {
        let messages = complete_flow();
        let encoded = encode(&messages).unwrap();
        assert_eq!(
            decode(&encoded.blob_portion, &encoded.call_data_tail).unwrap(),
            messages
        );
    }

    #[test]
    fn matches_solidity_little_endian_vector() {
        let messages = vec![
            Message::ChainOperation {
                chain_id: 0x0102_0304_0506_0708,
                operations: bytes!("aa"),
            },
            Message::CloseBlobStream,
        ];
        let encoded = encode(&messages).unwrap();
        assert_eq!(
            hex::encode(encoded.blob_portion),
            "0002080706050403020101aa01"
        );
        assert!(encoded.call_data_tail.is_empty());
    }

    #[test]
    fn accepts_non_minimal_varint() {
        let stream = hex::decode("000207000000000000008100aa01").unwrap();
        assert_eq!(
            decode(&stream, &[]).unwrap(),
            vec![
                Message::ChainOperation {
                    chain_id: 7,
                    operations: bytes!("aa")
                },
                Message::CloseBlobStream,
            ]
        );
    }

    #[test]
    fn rejects_u32_overflowing_fifth_varint_byte() {
        let stream = hex::decode("00020700000000000000ffffffff1001").unwrap();
        assert_eq!(decode(&stream, &[]), Err(Error::VarintOverflow));
    }

    #[test]
    fn pack_round_trip_preserves_spec_layout() {
        let encoded = encode(&complete_flow()).unwrap();
        let blobs = pack(&encoded.blob_portion).unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].as_bytes()[0], 0);
        assert_eq!(
            decode_blobs(&blobs, &encoded.call_data_tail).unwrap(),
            complete_flow()
        );
    }

    #[test]
    fn message_can_span_blob_boundary() {
        let messages = vec![
            Message::ChainOperation {
                chain_id: 7,
                operations: Bytes::from(vec![0x5a; DATA_BYTES_PER_BLOB + 100]),
            },
            Message::CloseBlobStream,
        ];
        let encoded = encode(&messages).unwrap();
        let blobs = pack(&encoded.blob_portion).unwrap();
        assert_eq!(blobs.len(), 2);
        assert_eq!(decode_blobs(&blobs, &[]).unwrap(), messages);
    }

    #[test]
    fn native_size_counts_zero_bytes_varints_and_framing_without_padding() {
        for size in [
            0,
            1,
            127,
            128,
            16383,
            16384,
            DATA_BYTES_PER_BLOB,
            DATA_BYTES_PER_BLOB * 3,
        ] {
            let messages = vec![
                Message::ChainOperation {
                    chain_id: 7,
                    operations: vec![0; size].into(),
                },
                Message::CloseBlobStream,
            ];
            let encoded = encode(&messages).unwrap();
            let measured = native_stream_len(size, 0).unwrap();
            assert_eq!(measured, encoded.blob_portion.len());
            let blobs = encode_native_payload(7, vec![0; size].into(), vec![]).unwrap();
            assert_eq!(blobs.len(), measured.div_ceil(DATA_BYTES_PER_BLOB));
        }
        assert!(native_stream_len(usize::MAX, 0).is_err());
        assert!(native_stream_len(1, usize::MAX).is_err());
    }

    #[test]
    fn native_profile_round_trips_semantic_transactions() {
        let semantic_messages = complete_flow()[1..11].to_vec();
        let blobs = encode_native_payload(7, bytes!("deadbeef"), semantic_messages.clone())
            .expect("encode native profile");
        assert_eq!(
            decode_native_payload(&blobs, 7).expect("decode native profile"),
            NativePayload {
                operations: bytes!("deadbeef"),
                semantic_messages,
            }
        );
    }

    #[test]
    fn native_profile_rejects_carrier_markers_in_semantics() {
        assert_eq!(
            encode_native_payload(7, Bytes::new(), vec![Message::CloseBlobStream]),
            Err(NativeProfileError::UnexpectedMessages)
        );
    }

    #[test]
    fn rejects_non_zero_field_element_msb() {
        let mut physical = vec![0_u8; BLOB_BYTES];
        physical[5 * FIELD_ELEMENT_BYTES] = 1;
        assert_eq!(
            Blob::try_from_slice(0, &physical),
            Err(Error::InvalidFieldElement {
                blob_index: 0,
                element_index: 5
            })
        );
    }

    #[test]
    fn rejects_non_zero_padding_after_close() {
        assert_eq!(
            decode(&[PROTOCOL_VERSION, 1, 0, 1], &[]),
            Err(Error::NonZeroPadding { offset: 3 })
        );
    }

    #[test]
    fn rejects_unclosed_and_misplaced_brackets() {
        assert_eq!(
            encode(&[Message::Snapshot, Message::CloseBlobStream]),
            Err(Error::MessageOutsideTransaction { message_type: 8 })
        );
        assert_eq!(
            encode(&[
                Message::InitiateCrossChainTransaction {
                    chain_id: 1,
                    tx_data: Bytes::new()
                },
                Message::CloseBlobStream,
            ]),
            Err(Error::UnclosedBracket)
        );
    }

    #[test]
    fn rejects_call_depth_above_reference_limit() {
        let mut messages = vec![Message::InitiateCrossChainTransaction {
            chain_id: 0,
            tx_data: Bytes::new(),
        }];
        for index in 0..MAX_CALL_DEPTH {
            messages.push(Message::Call {
                to_chain: index as u64 + 1,
                from_address: Address::ZERO,
                to_address: Address::ZERO,
                value: U256::ZERO,
                gas: 0,
                data: Bytes::new(),
            });
        }
        messages.push(Message::CloseBlobStream);
        assert_eq!(encode(&messages), Err(Error::DepthLimitExceeded));
    }
}
