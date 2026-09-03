//! Execution validation and settlement-evidence normalization.
//!
//! Values move through explicit trust stages: Composer supplies structurally
//! admitted blocks; this module exact-decodes and re-executes them; a consuming
//! cross-check binds every backend result to its admitted block; only then does
//! [`ValidatedWindow`] expose the evidence used by settlement.

use std::path::Path;

use alloy_primitives::B256;
use thiserror::Error;

use crate::cancel::CancellationToken;
use crate::window::{AdmittedBlock, AdmittedBlocks};

mod stateless;

type EthereumBlock = reth_ethereum_primitives::Block;

/// A canonical prefix-block hash locally recomputed during successful replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransactionStateCheckpoint {
    /// Zero-based position of the transaction within its block.
    pub(crate) transaction_index: usize,
    /// Hash of the canonical block containing the transaction prefix ending at
    /// `transaction_index`.
    pub(crate) block_hash: B256,
}

/// Successful backend output for one block before the shared consuming check.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BackendBlockOutput {
    /// Block number exact-decoded by the backend from consensus RLP.
    pub(crate) decoded_number: u64,
    /// Parent hash exact-decoded by the backend from consensus RLP.
    pub(crate) decoded_parent_hash: B256,
    /// Hash computed from the exact-decoded block header.
    pub(crate) computed_hash: B256,
    /// Transaction count in the exact-decoded block body.
    pub(crate) decoded_transaction_count: usize,
    /// Receipt success flag for every replayed transaction, in block order.
    pub(crate) receipt_successes: Vec<bool>,
    /// Locally computed prefix block hashes at selected transaction boundaries, strictly
    /// ordered by transaction index. Non-settling blocks have no checkpoints.
    pub(crate) transaction_state_checkpoints: Vec<TransactionStateCheckpoint>,
    /// Post-state root recomputed and matched against the block header.
    pub(crate) post_state_root: B256,
    /// Locally derived facts retained specifically for settlement gates.
    pub(crate) settlement_evidence: SettlementBlockEvidence,
}

/// Successful backend output for a contiguous block window.
///
/// This is not settlement-ready until the consuming cross-check binds each
/// computed hash and transaction count to its corresponding admitted block.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BackendWindowOutput {
    /// Validated state root from which the first block was replayed.
    pub(crate) pre_state_root: B256,
    /// One associated output per replayed block, oldest first.
    pub(crate) blocks: Vec<BackendBlockOutput>,
}

/// Canonically decoded fields used to bind an outbound effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodedOutboundEvent {
    call_hash: B256,
    call_gas: u64,
}

impl DecodedOutboundEvent {
    /// Construct fields recovered from one canonical `EEZL2` event.
    pub(in crate::validate) const fn new(call_hash: B256, call_gas: u64) -> Self {
        Self {
            call_hash,
            call_gas,
        }
    }

    /// Hash emitted for the executed call.
    pub(crate) const fn call_hash(&self) -> B256 {
        self.call_hash
    }

    /// Manager-entry gas value included in the emitted hash.
    ///
    /// This is distinct from any gas limit forwarded to the destination.
    pub(crate) const fn call_gas(&self) -> u64 {
        self.call_gas
    }
}

/// One outbound-event candidate observed in the validated execution output.
///
/// `decoded_event` is absent when a log from EEZL2 has the outbound event
/// signature but its complete event encoding is malformed. Retaining that
/// candidate lets settlement fail closed instead of silently ignoring it.
/// Production construction is restricted to validation backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutboundEventObservation {
    /// Zero-based transaction position within the block.
    pub(in crate::validate) transaction_index: usize,
    /// Zero-based log position within that transaction's receipt.
    pub(in crate::validate) receipt_log_index: usize,
    /// Decoded fields, when the complete event is canonical.
    pub(in crate::validate) decoded_event: Option<DecodedOutboundEvent>,
}

impl OutboundEventObservation {
    /// Zero-based transaction position within the validated block.
    pub(crate) const fn transaction_index(&self) -> usize {
        self.transaction_index
    }

    /// Zero-based receipt-log position within the transaction.
    pub(crate) const fn receipt_log_index(&self) -> usize {
        self.receipt_log_index
    }

    /// Canonically decoded event, or `None` for a malformed candidate.
    pub(crate) const fn decoded_event(&self) -> Option<DecodedOutboundEvent> {
        self.decoded_event
    }

    /// Construct synthetic backend evidence with decoded event fields.
    #[cfg(test)]
    pub(crate) const fn decoded_for_test(
        transaction_index: usize,
        receipt_log_index: usize,
        call_hash: B256,
        call_gas: u64,
    ) -> Self {
        Self {
            transaction_index,
            receipt_log_index,
            decoded_event: Some(DecodedOutboundEvent::new(call_hash, call_gas)),
        }
    }

    /// Construct a malformed synthetic event candidate for rejection tests.
    #[cfg(test)]
    pub(crate) const fn malformed_for_test(
        transaction_index: usize,
        receipt_log_index: usize,
    ) -> Self {
        Self {
            transaction_index,
            receipt_log_index,
            decoded_event: None,
        }
    }
}

/// Facts derived locally from the same block and receipts accepted by replay.
/// Production construction is restricted to validation backends; settlement
/// receives only immutable views.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SettlementBlockEvidence {
    /// Whether each transaction's fork-aware recovered signer is the reserved
    /// system address, in exact block order.
    pub(in crate::validate) system_sender_flags: Vec<bool>,
    /// Outbound event candidates, in transaction and receipt-log order.
    pub(in crate::validate) observed_outbound_events: Vec<OutboundEventObservation>,
}

impl SettlementBlockEvidence {
    /// Recovered system-sender classification in exact transaction order.
    pub(crate) fn system_sender_flags(&self) -> &[bool] {
        &self.system_sender_flags
    }

    /// Outbound event candidates in transaction and receipt-log order.
    pub(crate) fn observed_outbound_events(&self) -> &[OutboundEventObservation] {
        &self.observed_outbound_events
    }

    /// Construct synthetic backend evidence for tests outside `validate`.
    #[cfg(test)]
    pub(crate) fn for_test(
        system_sender_flags: Vec<bool>,
        observed_outbound_events: Vec<OutboundEventObservation>,
    ) -> Self {
        Self {
            system_sender_flags,
            observed_outbound_events,
        }
    }

    /// Replace recovered sender evidence in tests that exercise rejection paths.
    #[cfg(test)]
    pub(crate) fn set_system_sender_flags_for_test(&mut self, flags: Vec<bool>) {
        self.system_sender_flags = flags;
    }
}

/// Block data retained after backend validation and output cross-checks.
///
/// Settlement receives the exact RLP and the execution facts required by its
/// framing and effect gates. This type omits the witness consumed by validation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ValidatedBlock {
    /// Number exact-decoded from RLP and matched to the Composer declaration.
    number: u64,
    /// Exact Composer-supplied bytes accepted by Stateless replay.
    rlp: Vec<u8>,
    /// Locally derived evidence from the accepted block and its receipts.
    settlement_evidence: SettlementBlockEvidence,
}

impl ValidatedBlock {
    /// Number exact-decoded from the validated block RLP.
    pub(crate) fn number(&self) -> u64 {
        self.number
    }

    /// Exact block RLP accepted by the validation backend.
    pub(crate) fn rlp(&self) -> &[u8] {
        &self.rlp
    }

    /// Locally derived execution evidence bound to this block.
    pub(crate) fn settlement_evidence(&self) -> &SettlementBlockEvidence {
        &self.settlement_evidence
    }

    /// Construct an explicitly synthetic validated block for unit tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        number: u64,
        rlp: Vec<u8>,
        settlement_evidence: SettlementBlockEvidence,
    ) -> Self {
        Self {
            number,
            rlp,
            settlement_evidence,
        }
    }
}

/// The terminal block and its execution outcome, kept together so settlement
/// cannot accidentally combine evidence from different positions.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ValidatedSettlingBlock {
    block: ValidatedBlock,
    /// Receipt status for every transaction in the settling block.
    receipt_successes: Vec<bool>,
    /// Locally recomputed prefix-block hashes selected for settlement effects.
    transaction_state_checkpoints: Vec<TransactionStateCheckpoint>,
    /// Exact terminal witness retained after successful validation so later
    /// lookup/static replay cannot substitute different state data.
    replay_input: SettlingReplayInput,
}

/// Authenticated input for a second, observation-only replay of the terminal
/// block. The normal validation backend already accepted this exact witness.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SettlingReplayInput {
    pub(crate) pre_state_root: B256,
    pub(crate) witness: alloy_rpc_types_debug::ExecutionWitness,
}

impl ValidatedSettlingBlock {
    /// Terminal validated block to which these replay results belong.
    pub(crate) fn block(&self) -> &ValidatedBlock {
        &self.block
    }

    /// Receipt success flag for every transaction, in block order.
    pub(crate) fn receipt_successes(&self) -> &[bool] {
        &self.receipt_successes
    }

    /// Locally recomputed block hashes requested at selected transaction boundaries.
    pub(crate) fn transaction_state_checkpoints(&self) -> &[TransactionStateCheckpoint] {
        &self.transaction_state_checkpoints
    }

    /// Exact witness and pre-state root accepted for this terminal block.
    pub(crate) const fn replay_input(&self) -> &SettlingReplayInput {
        &self.replay_input
    }

    /// Construct explicitly synthetic settling-block evidence for unit tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        block: ValidatedBlock,
        receipt_successes: Vec<bool>,
        transaction_state_checkpoints: Vec<TransactionStateCheckpoint>,
    ) -> Self {
        Self {
            block,
            receipt_successes,
            transaction_state_checkpoints,
            replay_input: SettlingReplayInput {
                pre_state_root: B256::ZERO,
                witness: alloy_rpc_types_debug::ExecutionWitness::default(),
            },
        }
    }
}

/// Validation output normalized for direct settlement consumption.
///
/// Construction consumes fully cross-checked backend output, separates the
/// terminal block from its predecessors, and derives the root immediately before that
/// terminal block. Callers therefore need no fallible indexing or parallel
/// vector manipulation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ValidatedWindow {
    /// Validated parent block hash before the first streamed block. Settlement still
    /// has to bind the batch's claimed anchor to this value.
    window_pre_block_hash: B256,
    /// Validated parent block hash immediately before the settling block.
    settling_pre_block_hash: B256,
    /// Validated terminal block hash after the complete window.
    window_post_block_hash: B256,
    /// Validated blocks preceding the terminal settling block.
    preceding_blocks: Vec<ValidatedBlock>,
    /// Terminal block together with its settlement-specific replay results.
    settling_block: ValidatedSettlingBlock,
}

impl ValidatedWindow {
    /// Validated parent block hash before the first streamed block.
    pub(crate) fn window_pre_block_hash(&self) -> B256 {
        self.window_pre_block_hash
    }

    /// Validated parent block hash immediately before the terminal block.
    pub(crate) fn settling_pre_block_hash(&self) -> B256 {
        self.settling_pre_block_hash
    }

    /// Validated terminal block hash after the complete window.
    pub(crate) fn window_post_block_hash(&self) -> B256 {
        self.window_post_block_hash
    }

    /// Validated blocks preceding the terminal settling block.
    pub(crate) fn preceding_blocks(&self) -> &[ValidatedBlock] {
        &self.preceding_blocks
    }

    /// Terminal block and its settlement-specific replay evidence.
    pub(crate) fn settling_block(&self) -> &ValidatedSettlingBlock {
        &self.settling_block
    }

    /// Construct an explicitly synthetic validated window for unit tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        window_pre_block_hash: B256,
        settling_pre_block_hash: B256,
        window_post_block_hash: B256,
        preceding_blocks: Vec<ValidatedBlock>,
        settling_block: ValidatedSettlingBlock,
    ) -> Self {
        Self {
            window_pre_block_hash,
            settling_pre_block_hash,
            window_post_block_hash,
            preceding_blocks,
            settling_block,
        }
    }
}

/// One admitted block paired with the successful backend output it was checked against.
struct CheckedBlock {
    admitted: AdmittedBlock,
    output: BackendBlockOutput,
}

/// Backend output whose shape and Composer-facing claims have been consumed
/// and cross-checked. Private fields make bypassing that boundary impossible.
struct CheckedBackendWindowOutput {
    preceding_blocks: Vec<CheckedBlock>,
    settling_block: CheckedBlock,
    settling_pre_state_root: B256,
}

/// A validation failure classified for the RPC boundary.
#[derive(Debug, Error)]
pub(crate) enum ValidationError {
    /// The backend rejected the input before claiming successful validation.
    #[error("{0}")]
    Rejected(String),
    /// A backend claimed success with output that violates its internal contract.
    #[error("{0}")]
    InvalidBackendOutput(String),
    /// Locally prepared validation state violated an implementation invariant.
    #[error("{0}")]
    InternalInvariant(String),
    /// The request disappeared while a synchronous backend was still active.
    #[error("validation cancelled")]
    Cancelled,
}

/// A lookup/static claim could not be reproduced from the authenticated
/// terminal witness at its pinned FIFO state.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ObservationValidationError {
    /// The pin is not the parent, a validated transaction checkpoint, or the
    /// complete terminal block.
    #[error("observation {index} pins unknown terminal-block prefix {block_hash}")]
    UnknownPinnedRoot { index: usize, block_hash: B256 },
    /// Stateless EVM replay failed, including for incomplete witness data.
    #[error("authenticated observation replay failed: {0}")]
    Replay(String),
    /// Execution completed but did not reproduce the claimed result exactly.
    #[error("observation {index} result differs from authenticated EVM replay")]
    OutcomeMismatch { index: usize },
}

/// Validation backend selected at startup.
#[derive(Debug)]
pub(crate) enum Validator {
    /// In-process re-execution using each block's streamed witness.
    Stateless(stateless::Backend),
    /// Canned responses for service-level tests.
    #[cfg(test)]
    Stub(testing::StubValidator),
}

impl Validator {
    /// Construct the production backend from the operator-configured execution
    /// rules. Composer never supplies this trust input.
    pub(crate) fn stateless(
        chain_document_path: &Path,
        expected_l2_system_address: alloy_primitives::Address,
    ) -> eyre::Result<Self> {
        stateless::Backend::from_chain_document_file(
            chain_document_path,
            expected_l2_system_address,
        )
        .map(Self::Stateless)
    }

    #[cfg(test)]
    pub(crate) fn stateless_for_test(
        chain_config: alloy_genesis::ChainConfig,
        expected_l2_system_address: alloy_primitives::Address,
    ) -> Self {
        Self::Stateless(stateless::Backend::new(
            chain_config,
            expected_l2_system_address,
        ))
    }

    /// Static, non-sensitive identifier for startup logs.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Stateless(_) => "stateless",
            #[cfg(test)]
            Self::Stub(_) => "stub",
        }
    }

    /// EIP-155 chain id from the operator-configured replay specification.
    pub(crate) fn chain_id(&self) -> u64 {
        match self {
            Self::Stateless(backend) => backend.chain_id(),
            #[cfg(test)]
            Self::Stub(_) => 1,
        }
    }

    /// Deployment address used by this backend to classify system transactions.
    pub(crate) fn expected_l2_system_address(&self) -> alloy_primitives::Address {
        match self {
            Self::Stateless(backend) => backend.expected_l2_system_address(),
            #[cfg(test)]
            Self::Stub(backend) => backend.expected_l2_system_address,
        }
    }

    /// Validate admitted blocks through the same checked boundary as production.
    #[cfg(test)]
    pub(crate) fn validate(
        &self,
        blocks: &[AdmittedBlock],
    ) -> Result<ValidatedWindow, ValidationError> {
        if blocks.is_empty() {
            return Err(ValidationError::Rejected(
                "refusing to validate an empty window".to_owned(),
            ));
        }
        self.validate_window(
            AdmittedBlocks::for_test(blocks.to_vec()),
            &CancellationToken::default(),
        )
    }

    /// Consume and validate a window with cooperative cancellation between
    /// non-interruptible backend work units. Ownership lets re-executing
    /// backends move witnesses without cloning their outer collections.
    pub(crate) fn validate_window(
        &self,
        blocks: AdmittedBlocks,
        cancellation: &CancellationToken,
    ) -> Result<ValidatedWindow, ValidationError> {
        let mut blocks = blocks.into_vec();
        let replay_witness = blocks
            .last()
            .map(|block| block.witness.clone())
            .ok_or_else(|| {
                ValidationError::Rejected("refusing to validate an empty window".into())
            })?;
        let output = self.run_backend(&mut blocks, cancellation)?;
        check_backend_window_output(blocks, output)
            .map(|checked| checked.into_validated_window(replay_witness))
            .map_err(|error| ValidationError::InvalidBackendOutput(error.to_string()))
    }

    /// Independently replay lookup/static semantic outcomes after the ordinary
    /// block validation has authenticated their witness and pinned roots.
    pub(crate) fn verify_observations(
        &self,
        window: &ValidatedWindow,
        claims: &[eez_protocol::settlement::FifoObservationClaim],
    ) -> Result<(), ObservationValidationError> {
        if claims.is_empty() {
            return Ok(());
        }
        match self {
            Self::Stateless(backend) => backend.verify_observations(window, claims),
            // Service-level tests use synthetic validated windows with no
            // authenticated trie. Semantic extraction and production replay
            // are exercised separately by their owning modules.
            #[cfg(test)]
            Self::Stub(_) => Ok(()),
        }
    }

    /// Run the selected backend. Its result remains unchecked against the
    /// Composer-facing claims until `check_backend_window_output` consumes it.
    fn run_backend(
        &self,
        blocks: &mut [AdmittedBlock],
        cancellation: &CancellationToken,
    ) -> Result<BackendWindowOutput, ValidationError> {
        match self {
            Self::Stateless(backend) => backend.validate_blocks(blocks, cancellation),
            #[cfg(test)]
            Self::Stub(stub) => {
                if cancellation.is_cancelled() {
                    return Err(ValidationError::Cancelled);
                }
                stub.next_response()
                    .map_err(|error| ValidationError::Rejected(error.to_string()))
            }
        }
    }
}

/// Consume the backend result while binding every computed value to the
/// admitted Composer input. Successful construction is the type-state boundary
/// after which settlement evidence may be exposed.
fn check_backend_window_output(
    admitted_blocks: Vec<AdmittedBlock>,
    backend_output: BackendWindowOutput,
) -> eyre::Result<CheckedBackendWindowOutput> {
    eyre::ensure!(
        backend_output.blocks.len() == admitted_blocks.len(),
        "backend output covers {} blocks, the admitted window has {}",
        backend_output.blocks.len(),
        admitted_blocks.len(),
    );

    let total_blocks = admitted_blocks.len();
    let settling_pre_state_root = total_blocks
        .checked_sub(2)
        .and_then(|index| backend_output.blocks.get(index))
        .map_or(backend_output.pre_state_root, |block| block.post_state_root);
    let mut checked_blocks = Vec::with_capacity(total_blocks);
    for (position, (admitted, output)) in admitted_blocks
        .into_iter()
        .zip(backend_output.blocks)
        .enumerate()
    {
        eyre::ensure!(
            output.decoded_number == admitted.declared_number,
            "backend decoded block number {} for Composer-declared block {}",
            output.decoded_number,
            admitted.declared_number,
        );
        eyre::ensure!(
            output.decoded_parent_hash == admitted.claimed_parent_hash,
            "backend decoded parent hash {} for block {} but Composer claimed {}",
            output.decoded_parent_hash,
            admitted.declared_number,
            admitted.claimed_parent_hash,
        );
        eyre::ensure!(
            output.computed_hash == admitted.claimed_hash,
            "computed hash {} for block {} does not match Composer-claimed hash {}",
            output.computed_hash,
            admitted.declared_number,
            admitted.claimed_hash,
        );
        let rlp_transaction_count = decode_transaction_count(&admitted)?;
        eyre::ensure!(
            output.decoded_transaction_count == rlp_transaction_count,
            "backend decoded {} transactions for block {}, exact RLP contains {}",
            output.decoded_transaction_count,
            admitted.declared_number,
            rlp_transaction_count,
        );
        let transaction_count = output.decoded_transaction_count;
        eyre::ensure!(
            output.receipt_successes.len() == transaction_count,
            "receipt statuses for block {} cover {} transactions, expected {}",
            admitted.declared_number,
            output.receipt_successes.len(),
            transaction_count,
        );
        let settlement_evidence = &output.settlement_evidence;
        eyre::ensure!(
            settlement_evidence.system_sender_flags.len() == transaction_count,
            "system-sender flags for block {} cover {} transactions, expected {}",
            admitted.declared_number,
            settlement_evidence.system_sender_flags.len(),
            transaction_count,
        );
        for observation in &settlement_evidence.observed_outbound_events {
            eyre::ensure!(
                observation.transaction_index < transaction_count,
                "outbound event observation targets transaction {} in block {} with {} \
                 transactions",
                observation.transaction_index,
                admitted.declared_number,
                transaction_count,
            );
        }
        for pair in settlement_evidence.observed_outbound_events.windows(2) {
            let previous = (pair[0].transaction_index, pair[0].receipt_log_index);
            let current = (pair[1].transaction_index, pair[1].receipt_log_index);
            eyre::ensure!(
                previous < current,
                "outbound event observations for block {} are not strictly ordered: {:?} is \
                 followed by {:?}",
                admitted.declared_number,
                previous,
                current,
            );
        }
        let checkpoints = &output.transaction_state_checkpoints;
        eyre::ensure!(
            position + 1 == total_blocks || checkpoints.is_empty(),
            "backend output supplied transaction state checkpoints for preceding block {}",
            admitted.declared_number,
        );
        for pair in checkpoints.windows(2) {
            eyre::ensure!(
                pair[0].transaction_index < pair[1].transaction_index,
                "transaction state checkpoints for block {} are not strictly ordered: index \
                 {} is followed by {}",
                admitted.declared_number,
                pair[0].transaction_index,
                pair[1].transaction_index,
            );
        }
        for checkpoint in checkpoints {
            eyre::ensure!(
                checkpoint.transaction_index < transaction_count,
                "transaction state checkpoint index {} is out of bounds for block {} with {} \
                 transactions",
                checkpoint.transaction_index,
                admitted.declared_number,
                transaction_count,
            );
        }

        checked_blocks.push(CheckedBlock { admitted, output });
    }

    let settling_block = checked_blocks
        .pop()
        .ok_or_else(|| eyre::eyre!("backend output unexpectedly has no blocks"))?;
    Ok(CheckedBackendWindowOutput {
        preceding_blocks: checked_blocks,
        settling_block,
        settling_pre_state_root,
    })
}

impl CheckedBackendWindowOutput {
    /// Rearrange already checked evidence for settlement without performing
    /// another trust transition.
    fn into_validated_window(
        self,
        replay_witness: alloy_rpc_types_debug::ExecutionWitness,
    ) -> ValidatedWindow {
        let window_pre_block_hash = self
            .preceding_blocks
            .first()
            .map_or(self.settling_block.output.decoded_parent_hash, |block| {
                block.output.decoded_parent_hash
            });
        let settling_pre_block_hash = self.settling_block.output.decoded_parent_hash;
        let window_post_block_hash = self.settling_block.output.computed_hash;
        ValidatedWindow {
            window_pre_block_hash,
            settling_pre_block_hash,
            window_post_block_hash,
            preceding_blocks: self
                .preceding_blocks
                .into_iter()
                .map(CheckedBlock::into_validated_block)
                .collect(),
            settling_block: self
                .settling_block
                .into_validated_settling_block(self.settling_pre_state_root, replay_witness),
        }
    }
}

impl CheckedBlock {
    /// Retain only the admitted bytes and locally derived settlement evidence.
    fn into_validated_block(self) -> ValidatedBlock {
        ValidatedBlock {
            number: self.output.decoded_number,
            rlp: self.admitted.rlp,
            settlement_evidence: self.output.settlement_evidence,
        }
    }

    /// Preserve the settling block's receipt statuses and computed checkpoints.
    fn into_validated_settling_block(
        self,
        pre_state_root: B256,
        witness: alloy_rpc_types_debug::ExecutionWitness,
    ) -> ValidatedSettlingBlock {
        let BackendBlockOutput {
            decoded_number,
            receipt_successes,
            transaction_state_checkpoints,
            settlement_evidence,
            ..
        } = self.output;
        ValidatedSettlingBlock {
            block: ValidatedBlock {
                number: decoded_number,
                rlp: self.admitted.rlp,
                settlement_evidence,
            },
            receipt_successes,
            transaction_state_checkpoints,
            replay_input: SettlingReplayInput {
                pre_state_root,
                witness,
            },
        }
    }
}

/// Exact-decode Composer-supplied consensus RLP for result-coverage checks.
fn decode_transaction_count(block: &AdmittedBlock) -> eyre::Result<usize> {
    alloy_rlp::decode_exact::<EthereumBlock>(&block.rlp)
        .map(|block| block.body.transactions.len())
        .map_err(|error| {
            eyre::eyre!(
                "block {} RLP does not decode exactly: {error}",
                block.declared_number
            )
        })
}

/// Test-support stub backend, shared with the service-level tests.
#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;
