//! Adapter for in-process Stateless validation.
//!
//! This module exact-decodes and binds streamed block claims, loads the
//! operator-configured chain trust input, derives checkpoint plans, invokes
//! Stateless/Reth, checks window continuity, and maps validated execution facts
//! into associated per-block output.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use alloy_consensus::{
    BlockHeader as _, Transaction as _,
    proofs::{calculate_receipt_root, calculate_transaction_root},
};
#[cfg(test)]
use alloy_genesis::{ChainConfig, Genesis};
use alloy_primitives::{Address, B256, Bloom, Bytes, U256, keccak256, map::B256IndexMap};
use alloy_sol_types::SolEvent as _;
use eez_driver::observation::{BlockObservation, ObservedOutcome, replay_block_with_observations};
use eez_evm::EezEvmConfig;
use eez_primitives::Block;
use eez_protocol::abi::eez_l2_events::CrossChainCallExecuted;
use eez_protocol::settlement::FifoObservationClaim;
use eez_protocol::settlement::{is_system_tx, pair_end_positions};
use reth_chainspec::{ChainSpec, EthereumHardforks as _};
use reth_primitives_traits::RecoveredBlock;
use revm::Database;
use revm::bytecode::Bytecode;
use revm::database::State;
use revm::state::AccountInfo;
use stateless_reth::validation::StatelessValidationError;
use stateless_reth::{
    StatelessValidationOutput, stateless_validation_recovered,
    stateless_validation_recovered_with_state_checkpoints,
};
use stateless_tries::{WitnessDbError, default::StatelessSparseTrie};
use tracing::{debug, info, trace};

use super::{
    BackendBlockOutput, BackendWindowOutput, DecodedOutboundEvent, ObservationValidationError,
    OutboundEventObservation, SettlementBlockEvidence, TransactionStateCheckpoint, ValidatedWindow,
    ValidationError,
};
use crate::EEZL2_ADDRESS;
use crate::cancel::CancellationToken;
use crate::window::AdmittedBlock;

use eez_l1::chain_config::load_l2_genesis;

const DEBUG_PROGRESS_INTERVAL: usize = 100;
const BLOCKHASH_ANCESTOR_LIMIT: usize = 256;

pub(super) mod reconstruction;

/// Transaction boundaries at which settlement framing needs replay-state
/// checkpoints in order to derive canonical prefix-block hashes.
///
/// The indices are derived locally from the recovered block. Composer-supplied
/// metadata never participates in checkpoint selection.
#[derive(Debug, PartialEq, Eq)]
struct CheckpointPlan {
    transaction_indices: Vec<usize>,
}

impl CheckpointPlan {
    /// Derive effect-candidate boundaries and system-sender flags from the
    /// locally recovered transactions.
    fn from_recovered_block(
        block: &RecoveredBlock<Block>,
        expected_l2_system_address: Address,
    ) -> (Self, Vec<bool>) {
        let transaction_count = block.body().transactions.len();
        let mut system_sender_flags = Vec::with_capacity(transaction_count);
        let mut sync_system_transaction_flags = Vec::with_capacity(transaction_count);
        for transaction in block.transactions_recovered() {
            let is_system_sender = transaction.signer() == expected_l2_system_address
                && !transaction.inner().is_history();
            system_sender_flags.push(is_system_sender);
            sync_system_transaction_flags.push(is_system_tx(
                alloy_consensus::Typed2718::ty(&transaction),
                transaction.signer(),
                transaction.to(),
                expected_l2_system_address,
                EEZL2_ADDRESS,
            ));
        }
        let ordinary = sync_system_transaction_flags
            .iter()
            .position(|&native| native)
            .unwrap_or(transaction_count);
        let mut transaction_indices = Vec::new();
        if ordinary > 0 {
            transaction_indices.push(ordinary - 1);
        }
        transaction_indices.extend(
            pair_end_positions(&sync_system_transaction_flags[ordinary..])
                .into_iter()
                .map(|index| ordinary + index),
        );
        let plan = Self {
            transaction_indices,
        };
        (plan, system_sender_flags)
    }

    /// Check that a checkpoint-capable backend honored the selection exactly.
    fn verify_returned(
        &self,
        checkpoints: &[ReplayStateCheckpoint],
    ) -> Result<(), ValidationError> {
        let selection_matches = checkpoints.len() == self.transaction_indices.len()
            && checkpoints
                .iter()
                .zip(&self.transaction_indices)
                .all(|(checkpoint, requested)| checkpoint.transaction_index == *requested);
        if !selection_matches {
            let returned = checkpoints
                .iter()
                .map(|checkpoint| checkpoint.transaction_index)
                .collect::<Vec<_>>();
            return Err(ValidationError::InvalidBackendOutput(format!(
                "stateless checkpoint response targeted transaction indices {returned:?}; \
                 requested {:?}",
                self.transaction_indices,
            )));
        }
        Ok(())
    }
}

/// Stateless replay's internal state commitment at a transaction boundary.
/// It is never exposed to settlement directly: the accepted block template,
/// prefix transactions and replayed receipts are required to turn it into a
/// canonical prefix-block hash first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReplayStateCheckpoint {
    transaction_index: usize,
    state_root: B256,
}

/// Signer-recovered settling block prepared before replay.
///
/// This block is exact-decoded and its signers are recovered, but it remains
/// untrusted until Stateless accepts the same `RecoveredBlock`. Preparing it
/// early keeps checkpoint selection independent from execution side effects.
struct PreparedSettlingBlock {
    block: RecoveredBlock<Block>,
    checkpoint_plan: CheckpointPlan,
    system_sender_flags: Vec<bool>,
}

/// In-process Stateless/Reth backend configured from operator-selected rules.
#[derive(Debug)]
pub(crate) struct Backend {
    chain_spec: Arc<ChainSpec>,
    evm_config: EezEvmConfig,
    expected_l2_system_address: Address,
    chain_document_digest: B256,
    #[cfg(test)]
    pub(super) derivation_enabled: bool,
    #[cfg(test)]
    checkpoint_replays: std::sync::atomic::AtomicUsize,
}

impl Backend {
    pub(super) fn schedule(&self) -> eyre::Result<eez_protocol::l2::Schedule> {
        Ok(eez_evm::profile::schedule(self.chain_spec.as_ref())?)
    }
    pub(super) const fn chain_document_digest(&self) -> B256 {
        self.chain_document_digest
    }

    /// EIP-155 identity pinned by the loaded execution chain specification.
    pub(super) fn chain_id(&self) -> u64 {
        self.chain_spec.chain().id()
    }

    /// Deployment address used to classify privileged L2 transactions.
    pub(super) const fn expected_l2_system_address(&self) -> Address {
        self.expected_l2_system_address
    }

    pub(super) fn from_chain_document_file(
        path: &Path,
        expected_l2_system_address: Address,
    ) -> eyre::Result<Self> {
        let loaded = load_l2_genesis(path)?;
        let chain_document_digest = loaded.digest();
        let genesis = loaded.into_genesis();
        let chain_config = &genesis.config;
        let genesis_timestamp = genesis.timestamp;
        info!(
            path = %path.display(),
            source = "Genesis",
            chain_id = chain_config.chain_id,
            "loaded stateless chain configuration",
        );
        debug!(
            ?chain_config,
            ?genesis_timestamp,
            "stateless chain configuration details",
        );
        Ok(Self::from_chain_spec(
            eez_evm::profile::chain_spec(genesis)?,
            expected_l2_system_address,
            chain_document_digest,
        ))
    }

    #[cfg(test)]
    pub(super) fn new(genesis: Genesis, expected_l2_system_address: Address) -> Self {
        let document = serde_json::to_vec(&genesis).unwrap();
        let digest = keccak256(eez_l1::chain_config::canonicalize_chain_json(&document).unwrap());
        let mut spec = ChainSpec::from_genesis(genesis);
        spec.base_fee_params = eez_protocol::l2::BASE_FEE_PARAMS.into();
        Self::from_chain_spec(spec, expected_l2_system_address, digest)
    }

    fn from_chain_spec(
        spec: ChainSpec,
        expected_l2_system_address: Address,
        chain_document_digest: B256,
    ) -> Self {
        let chain_spec = Arc::new(spec);
        let evm_config = EezEvmConfig::new(Arc::clone(&chain_spec));
        Self {
            chain_spec,
            evm_config,
            expected_l2_system_address,
            chain_document_digest,
            #[cfg(test)]
            derivation_enabled: true,
            #[cfg(test)]
            checkpoint_replays: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    pub(super) fn validate(
        &self,
        blocks: &[AdmittedBlock],
    ) -> Result<BackendWindowOutput, ValidationError> {
        self.validate_blocks(
            &mut blocks.to_vec(),
            &CancellationToken::default(),
            crate::testkit::checked_history(blocks)?,
        )
    }

    /// Replay every admitted block and return associated per-block output. The
    /// caller still consumes and checks that output before exposing settlement evidence.
    pub(super) fn validate_blocks(
        &self,
        blocks: &mut [AdmittedBlock],
        cancellation: &CancellationToken,
        history: Arc<eez_protocol::l1_history::HeaderChain>,
    ) -> Result<BackendWindowOutput, ValidationError> {
        let evm_config = self
            .evm_config
            .clone()
            .with_l1_history(Arc::new(eez_protocol::l1_history::WitnessHistory(history)))
            .map_err(|error| ValidationError::Rejected(error.to_string()))?;
        let validation_started = Instant::now();
        let total_blocks = blocks.len();
        let mut block_outputs = Vec::<BackendBlockOutput>::with_capacity(total_blocks);
        let mut window_pre_state_root = None;

        // Derive the final block's complete checkpoint plan before any witness
        // is consumed or earlier block is replayed.
        check_cancellation(cancellation, 0, total_blocks)?;
        let mut prepared_settling_block = blocks
            .last()
            .map(|admitted| {
                let recovered_block = decode_match_and_recover_signers(admitted, &self.chain_spec)?;
                let (checkpoint_plan, system_sender_flags) = CheckpointPlan::from_recovered_block(
                    &recovered_block,
                    self.expected_l2_system_address,
                );
                Ok(PreparedSettlingBlock {
                    block: recovered_block,
                    checkpoint_plan,
                    system_sender_flags,
                })
            })
            .transpose()?;

        for (index, admitted) in blocks.iter_mut().enumerate() {
            check_cancellation(cancellation, index, total_blocks)?;
            let block_started = Instant::now();
            let block_ordinal = index + 1;
            let (recovered_block, checkpoint_plan, system_sender_flags) = if block_ordinal
                == total_blocks
            {
                let prepared = prepared_settling_block.take().ok_or_else(|| {
                    ValidationError::InternalInvariant(
                        "prepared settling block unexpectedly missing".to_owned(),
                    )
                })?;
                (
                    prepared.block,
                    // Ordinary (including empty) terminals use their fully
                    // validated block hash as the ordinary-prefix commitment.
                    // Only native effect boundaries need intermediate roots.
                    prepared
                        .system_sender_flags
                        .iter()
                        .any(|&native| native)
                        .then_some(prepared.checkpoint_plan),
                    prepared.system_sender_flags,
                )
            } else {
                let recovered_block = decode_match_and_recover_signers(admitted, &self.chain_spec)?;
                let system_sender_flags =
                    system_sender_flags(&recovered_block, self.expected_l2_system_address);
                (recovered_block, None, system_sender_flags)
            };
            let block_number = recovered_block.header().number;
            let decoded_parent_hash = recovered_block.header().parent_hash;
            let timestamp = recovered_block.header().timestamp;
            let transaction_count = recovered_block.body().transactions.len();
            let settling_block_template = checkpoint_plan.as_ref().map(|_| recovered_block.clone());
            // Retain only this block's inputs while its witness is consumed. The
            // descriptor is created after replay, never from claimed roots.
            let reconstruct = true;
            #[cfg(test)]
            let reconstruct = reconstruct && self.derivation_enabled;
            let reconstruction_input = reconstruct.then(|| {
                (
                    recovered_block.clone().into_sealed_block().into_block(),
                    admitted.witness.headers.last().cloned(),
                )
            });
            trace!(
                block_ordinal,
                block_number,
                claimed_block_hash = %admitted.claimed_hash,
                timestamp,
                transactions = transaction_count,
                "validating stateless block",
            );

            // Before this call the decoded block and witness remain untrusted.
            // Successful Stateless output means consensus, witness pre-state,
            // execution receipts, and the recomputed post-state commitment all
            // passed. The terminal also captures its pre-transaction state so
            // the B[0] variant includes pre-execution changes.
            let witness = std::mem::take(&mut admitted.witness);
            let (stateless_output, replay_state_checkpoints, pre_transaction_state_root) =
                match checkpoint_plan {
                    Some(plan) => {
                        #[cfg(test)]
                        self.checkpoint_replays
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let output = stateless_validation_recovered_with_state_checkpoints(
                            recovered_block,
                            witness,
                            Arc::clone(&self.chain_spec),
                            evm_config.clone(),
                            &plan.transaction_indices,
                        )
                        .map_err(|error| map_stateless_error(block_number, error))?;
                        let checkpoints = output
                            .checkpoints
                            .transaction_state_checkpoints
                            .into_iter()
                            .map(|checkpoint| ReplayStateCheckpoint {
                                transaction_index: checkpoint.transaction_index,
                                state_root: checkpoint.state_root,
                            })
                            .collect::<Vec<_>>();
                        plan.verify_returned(&checkpoints)?;
                        (
                            output.validation,
                            checkpoints,
                            Some(output.checkpoints.pre_transaction_state_root),
                        )
                    }
                    _ => {
                        let output = stateless_validation_recovered(
                            recovered_block,
                            witness,
                            Arc::clone(&self.chain_spec),
                            evm_config.clone(),
                        )
                        .map_err(|error| map_stateless_error(block_number, error))?;
                        (output, Vec::new(), None)
                    }
                };

            let reconstruction = reconstruction_input
                .map(|(block, parent)| {
                    reconstruction::reconstruct(
                        &evm_config,
                        &block,
                        &admitted.rlp,
                        parent.as_ref().map(AsRef::as_ref),
                        &stateless_output,
                    )
                    .map_err(|error| {
                        ValidationError::Rejected(format!(
                            "block {block_number} cannot reconstruct under the operations profile: {error}"
                        ))
                    })
                })
                .transpose()?;
            let StatelessValidationOutput {
                block_hash: computed_hash,
                pre_state_root,
                post_state_root,
                execution_output,
                block_access_list: _validated_block_access_list,
            } = stateless_output;
            if let Some(previous_post_state_root) =
                block_outputs.last().map(|output| output.post_state_root)
                && pre_state_root != previous_post_state_root
            {
                return Err(ValidationError::Rejected(format!(
                    "stateless state roots do not telescope at block {block_number}: expected \
                     pre-state root {previous_post_state_root}, got {pre_state_root}",
                )));
            }
            window_pre_state_root.get_or_insert(pre_state_root);

            // These receipts come from successful replay. Observed outbound
            // events are retained here but authorized against batch entries only
            // later by settlement.
            let has_execution_requests = !execution_output.result.requests.is_empty();
            let validated_receipts = execution_output.result.receipts;
            let (ordinary_prefix_hash, transaction_state_checkpoints) =
                match settling_block_template {
                    Some(block) => {
                        let mut checkpoints = derive_prefix_block_hashes(
                            &block,
                            &validated_receipts,
                            &replay_state_checkpoints,
                            has_execution_requests,
                        )?;
                        let ordinary = system_sender_flags
                            .iter()
                            .position(|&native| native)
                            .unwrap_or(transaction_count);
                        let base = if ordinary == 0 {
                            // The checkpoint backend ran pre-execution changes before
                            // the first native transaction; the parent state root
                            // cannot stand in for that state.
                            let state_root = pre_transaction_state_root.ok_or_else(|| {
                                ValidationError::InvalidBackendOutput(
                                    "missing pre-transaction state root".into(),
                                )
                            })?;
                            prefix_block_hash(&block, &validated_receipts, 0, state_root)?
                        } else {
                            let checkpoint = checkpoints.first().ok_or_else(|| {
                                ValidationError::InvalidBackendOutput(
                                    "missing ordinary-prefix checkpoint".into(),
                                )
                            })?;
                            if checkpoint.transaction_index != ordinary - 1 {
                                return Err(ValidationError::InvalidBackendOutput(
                                    "wrong ordinary-prefix checkpoint".into(),
                                ));
                            }
                            checkpoints.remove(0).block_hash
                        };
                        (base, checkpoints)
                    }
                    None => (computed_hash, Vec::new()),
                };
            let receipt_successes = validated_receipts
                .iter()
                .map(|receipt| receipt.success)
                .collect::<Vec<_>>();
            let observed_outbound_events = observe_outbound_events(&validated_receipts);

            if tracing::enabled!(tracing::Level::TRACE) {
                let failed_transactions = receipt_successes
                    .iter()
                    .filter(|&&success| !success)
                    .count();
                let malformed_outbound_events = observed_outbound_events
                    .iter()
                    .filter(|observation| observation.decoded_event.is_none())
                    .count();
                trace!(
                    block_ordinal,
                    total_blocks,
                    block_number,
                    computed_block_hash = %computed_hash,
                    validated_pre_state_root = %pre_state_root,
                    validated_post_state_root = %post_state_root,
                    transactions = transaction_count,
                    failed_transactions,
                    outbound_events = observed_outbound_events.len(),
                    malformed_outbound_events,
                    block_elapsed_ms = elapsed_millis(block_started),
                    "stateless block validated",
                );
            }
            block_outputs.push(BackendBlockOutput {
                reconstruction,
                decoded_number: block_number,
                decoded_parent_hash,
                computed_hash,
                decoded_transaction_count: transaction_count,
                receipt_successes,
                transaction_state_checkpoints,
                post_state_root,
                settlement_evidence: SettlementBlockEvidence {
                    ordinary_prefix_hash,
                    cumulative_gas_used: validated_receipts
                        .iter()
                        .map(alloy_consensus::TxReceipt::cumulative_gas_used)
                        .collect(),
                    system_sender_flags,
                    observed_outbound_events,
                },
            });

            if block_ordinal.is_multiple_of(DEBUG_PROGRESS_INTERVAL) {
                debug!(
                    validated_blocks = block_ordinal,
                    total_blocks,
                    block_number,
                    validated_post_state_root = %post_state_root,
                    elapsed_ms = elapsed_millis(validation_started),
                    "stateless validation progress",
                );
            }
        }

        let pre_state_root = window_pre_state_root.ok_or_else(|| {
            ValidationError::Rejected("refusing to validate an empty window".to_owned())
        })?;
        Ok(BackendWindowOutput {
            pre_state_root,
            blocks: block_outputs,
        })
    }

    /// Re-execute every non-applying lookup/static root from the exact witness
    /// already accepted for the terminal block.
    pub(super) fn verify_observations(
        &self,
        window: &ValidatedWindow,
        claims: &[FifoObservationClaim],
        history: Arc<eez_protocol::l1_history::HeaderChain>,
    ) -> Result<(), ObservationValidationError> {
        let outcomes = self.replay_observations(window, claims, history)?;
        for (index, (claim, outcome)) in claims.iter().zip(outcomes).enumerate() {
            if claim.expected_success != outcome.success || claim.expected_output != outcome.output
            {
                return Err(ObservationValidationError::OutcomeMismatch { index });
            }
        }
        Ok(())
    }

    fn replay_observations(
        &self,
        window: &ValidatedWindow,
        claims: &[FifoObservationClaim],
        history: Arc<eez_protocol::l1_history::HeaderChain>,
    ) -> Result<Vec<ObservedOutcome>, ObservationValidationError> {
        let settling = window.settling_block();
        let replay = settling.replay_input();
        let block = alloy_rlp::decode_exact::<Block>(settling.block().rlp())
            .map_err(|error| ObservationValidationError::Replay(error.to_string()))?;
        let block = recover_block(block, &self.chain_spec)
            .map_err(|error| ObservationValidationError::Replay(error.to_string()))?;
        let transaction_count = block.body().transactions.len();

        let mut observations = Vec::with_capacity(claims.len());
        for (index, claim) in claims.iter().cloned().enumerate() {
            let after_transactions = if claim.pinned_block_hash
                == settling
                    .block()
                    .settlement_evidence()
                    .ordinary_prefix_hash()
            {
                block
                    .body()
                    .transactions
                    .iter()
                    .position(|tx| matches!(tx, eez_primitives::EezTxEnvelope::System(_)))
                    .unwrap_or(transaction_count)
            } else if let Some(checkpoint) = settling
                .transaction_state_checkpoints()
                .iter()
                .find(|checkpoint| checkpoint.block_hash == claim.pinned_block_hash)
            {
                checkpoint.transaction_index + 1
            } else if claim.pinned_block_hash == window.window_post_block_hash() {
                transaction_count
            } else {
                return Err(ObservationValidationError::UnknownPinnedRoot {
                    index,
                    block_hash: claim.pinned_block_hash,
                });
            };
            observations.push(BlockObservation {
                after_transactions,
                manager_address: crate::EEZL2_ADDRESS,
                claim,
            });
        }
        if observations
            .windows(2)
            .any(|pair| pair[0].after_transactions > pair[1].after_transactions)
        {
            return Err(ObservationValidationError::Replay(
                "observation pins are not in FIFO prefix order".to_owned(),
            ));
        }

        // The ordinary validation pass consumed an identical clone and
        // accepted its ancestor chain and pre-state commitment. Rebuilding the
        // sparse trie here authenticates every supplemental observation read;
        // missing account/code/storage nodes fail closed.
        let (trie, bytecode) =
            StatelessSparseTrie::new(replay.witness.clone(), replay.pre_state_root)
                .map_err(|error| ObservationValidationError::Replay(format!("{error:?}")))?;
        // Validate the retained header chain again before exposing it through
        // BLOCKHASH. This is redundant with the first Stateless pass, but it
        // keeps the observation verifier fail-closed if the retained replay
        // representation is ever refactored or made mutable.
        let ancestor_hashes = authenticated_ancestor_hashes(&block, &replay.witness.headers)?;
        let db = ObservationWitnessDatabase::new(&trie, bytecode, ancestor_hashes);
        let has_bal = block.sealed_header().block_access_list_hash().is_some();
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .with_bal_builder_if(has_bal)
            .build();
        let evm_config = self
            .evm_config
            .clone()
            .with_l1_history(Arc::new(eez_protocol::l1_history::WitnessHistory(history)))
            .map_err(|error| ObservationValidationError::Replay(error.to_string()))?;
        replay_block_with_observations(&mut state, &evm_config, &block, &observations)
            .map_err(|error| ObservationValidationError::Replay(error.to_string()))
    }
}

/// Decode and authenticate the EVM `BLOCKHASH` window against the exact
/// terminal block header. Witness headers are ordered oldest-to-newest, so the
/// parent is checked first by iterating in reverse.
fn authenticated_ancestor_hashes(
    block: &RecoveredBlock<Block>,
    headers: &[Bytes],
) -> Result<BTreeMap<u64, B256>, ObservationValidationError> {
    if headers.len() > BLOCKHASH_ANCESTOR_LIMIT {
        return Err(ObservationValidationError::Replay(format!(
            "ancestor header count {} exceeds limit {BLOCKHASH_ANCESTOR_LIMIT}",
            headers.len(),
        )));
    }
    if headers.is_empty() {
        return Err(ObservationValidationError::Replay(
            "terminal replay witness has no parent header".to_owned(),
        ));
    }

    let decoded = headers
        .iter()
        .map(|bytes| {
            alloy_rlp::decode_exact::<alloy_consensus::Header>(bytes)
                .map(|header| (header, keccak256(bytes)))
                .map_err(|error| ObservationValidationError::Replay(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut hashes = BTreeMap::new();
    let mut child_number = block.header().number;
    let mut expected_parent_hash = block.header().parent_hash;
    for (parent, actual_hash) in decoded.iter().rev() {
        let expected_parent_number = child_number.checked_sub(1).ok_or_else(|| {
            ObservationValidationError::Replay(
                "genesis block cannot have an ancestor-header witness".to_owned(),
            )
        })?;
        if parent.number != expected_parent_number {
            return Err(ObservationValidationError::Replay(format!(
                "invalid ancestor number: block {child_number} expects parent {expected_parent_number}, got {}",
                parent.number,
            )));
        }
        if *actual_hash != expected_parent_hash {
            return Err(ObservationValidationError::Replay(format!(
                "invalid ancestor hash at block {}: expected {expected_parent_hash}, got {actual_hash}",
                parent.number,
            )));
        }
        hashes.insert(parent.number, *actual_hash);
        child_number = parent.number;
        expected_parent_hash = parent.parent_hash;
    }
    Ok(hashes)
}

/// Minimal witness-backed revm database used only for observation replay.
/// This mirrors Stateless's private validation adapter while deliberately
/// keeping account/storage reads fail-closed through `StatelessSparseTrie`.
#[derive(Debug)]
struct ObservationWitnessDatabase<'a> {
    trie: &'a StatelessSparseTrie,
    bytecode: B256IndexMap<Bytes>,
    block_hashes: BTreeMap<u64, B256>,
}

impl<'a> ObservationWitnessDatabase<'a> {
    const fn new(
        trie: &'a StatelessSparseTrie,
        bytecode: B256IndexMap<Bytes>,
        block_hashes: BTreeMap<u64, B256>,
    ) -> Self {
        Self {
            trie,
            bytecode,
            block_hashes,
        }
    }
}

impl Database for ObservationWitnessDatabase<'_> {
    type Error = WitnessDbError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.trie.account(address).map(|account| {
            account.map(|account| AccountInfo {
                balance: account.balance,
                nonce: account.nonce,
                code_hash: account.code_hash,
                code: None,
                account_id: None,
            })
        })
    }

    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        self.trie.storage(address, slot)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.bytecode
            .get(&code_hash)
            .cloned()
            .map(Bytecode::new_raw)
            .ok_or_else(|| {
                WitnessDbError::TrieWitness(format!("bytecode for {code_hash} not found"))
            })
    }

    fn block_hash(&mut self, block_number: u64) -> Result<B256, Self::Error> {
        self.block_hashes
            .get(&block_number)
            .copied()
            .ok_or(WitnessDbError::StateNotFound(block_number))
    }
}

/// Convert replayed transaction-state checkpoints into complete canonical
/// block-hash commitments. Every header field not affected by truncating the
/// transaction list is inherited from the exact block accepted by Stateless.
/// Transaction/receipt roots, bloom, gas and state commitment are recomputed
/// for each prefix.
fn derive_prefix_block_hashes(
    block: &RecoveredBlock<Block>,
    receipts: &[eez_primitives::Receipt],
    checkpoints: &[ReplayStateCheckpoint],
    has_execution_requests: bool,
) -> Result<Vec<TransactionStateCheckpoint>, ValidationError> {
    if receipts.len() != block.body().transactions.len() {
        return Err(ValidationError::InvalidBackendOutput(format!(
            "cannot derive prefix block hashes for block {}: replay returned {} receipts for {} transactions",
            block.header().number,
            receipts.len(),
            block.body().transactions.len(),
        )));
    }
    if block
        .body()
        .withdrawals
        .as_ref()
        .is_some_and(|withdrawals| !withdrawals.is_empty())
    {
        return Err(ValidationError::Rejected(format!(
            "settling block {} has withdrawals; prefix block-hash commitments require no post-transaction state changes",
            block.header().number,
        )));
    }
    if !block.body().ommers.is_empty() {
        return Err(ValidationError::Rejected(format!(
            "settling block {} has ommers; prefix block-hash commitments require a post-merge block body",
            block.header().number,
        )));
    }
    if has_execution_requests {
        return Err(ValidationError::Rejected(format!(
            "settling block {} produces EIP-7685 requests; prefix block-hash commitments currently support only empty requests",
            block.header().number,
        )));
    }

    let mut block_hashes = Vec::with_capacity(checkpoints.len());
    for checkpoint in checkpoints {
        let block_hash = prefix_block_hash(
            block,
            receipts,
            checkpoint.transaction_index + 1,
            checkpoint.state_root,
        )?;
        block_hashes.push(TransactionStateCheckpoint {
            transaction_index: checkpoint.transaction_index,
            block_hash,
        });
    }
    Ok(block_hashes)
}

fn prefix_block_hash(
    block: &RecoveredBlock<Block>,
    receipts: &[eez_primitives::Receipt],
    prefix_len: usize,
    state_root: B256,
) -> Result<B256, ValidationError> {
    if prefix_len > receipts.len() || receipts.len() != block.body().transactions.len() {
        return Err(ValidationError::InvalidBackendOutput(
            "prefix receipt count mismatch".into(),
        ));
    }
    let prefix_receipts = receipts[..prefix_len]
        .iter()
        .map(alloy_consensus::TxReceipt::with_bloom_ref)
        .collect::<Vec<_>>();
    let mut header = block.header().clone();
    header.state_root = state_root;
    header.transactions_root = calculate_transaction_root(&block.body().transactions[..prefix_len]);
    header.receipts_root = calculate_receipt_root(&prefix_receipts);
    header.logs_bloom = prefix_receipts
        .iter()
        .fold(Bloom::ZERO, |bloom, receipt| bloom | receipt.bloom_ref());
    header.gas_used = receipts[..prefix_len]
        .last()
        .map_or(0, alloy_consensus::TxReceipt::cumulative_gas_used);
    if header.blob_gas_used.is_some() {
        header.blob_gas_used = Some(
            block.body().transactions[..prefix_len]
                .iter()
                .try_fold(0u64, |total, tx| {
                    total.checked_add(tx.blob_gas_used().unwrap_or(0))
                })
                .ok_or_else(|| {
                    ValidationError::InvalidBackendOutput("prefix blob gas overflow".into())
                })?,
        );
    }
    let hash = header.hash_slow();
    if prefix_len == block.body().transactions.len() && hash != block.header().hash_slow() {
        return Err(ValidationError::InvalidBackendOutput(format!(
            "final prefix hash {hash} does not match validated full block {}",
            block.header().hash_slow()
        )));
    }
    Ok(hash)
}

/// Classify a rejected local checkpoint plan as an internal invariant failure;
/// all other Stateless errors reject the input.
fn map_stateless_error(block_number: u64, error: StatelessValidationError) -> ValidationError {
    match error {
        error @ (StatelessValidationError::UnorderedTransactionCheckpoints { .. }
        | StatelessValidationError::TransactionCheckpointOutOfBounds { .. }) => {
            ValidationError::InternalInvariant(format!(
                "stateless rejected the locally derived checkpoint plan for block {block_number}: \
                 {error}",
            ))
        }
        error => ValidationError::Rejected(format!(
            "stateless validation rejected block {block_number}: {error}",
        )),
    }
}

/// Classify each locally recovered signer for later settlement policy.
fn system_sender_flags(
    block: &RecoveredBlock<Block>,
    expected_l2_system_address: Address,
) -> Vec<bool> {
    block
        .transactions_recovered()
        .map(|transaction| {
            transaction.signer() == expected_l2_system_address && !transaction.inner().is_history()
        })
        .collect()
}

/// Stop between non-interruptible block executions after request cancellation.
fn check_cancellation(
    cancellation: &CancellationToken,
    validated_blocks: usize,
    total_blocks: usize,
) -> Result<(), ValidationError> {
    if cancellation.is_cancelled() {
        debug!(
            validated_blocks,
            total_blocks, "stateless validation stopped after request cancellation",
        );
        Err(ValidationError::Cancelled)
    } else {
        Ok(())
    }
}

fn elapsed_millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Exact-decode Composer-supplied RLP and match its identity to stream metadata.
/// The returned block is still untrusted until Stateless accepts it.
fn decode_and_match_stream_metadata(admitted: &AdmittedBlock) -> Result<Block, ValidationError> {
    let block = alloy_rlp::decode_exact::<Block>(&admitted.rlp).map_err(|error| {
        ValidationError::Rejected(format!(
            "block {} carries invalid consensus RLP: {error}",
            admitted.declared_number,
        ))
    })?;
    if block.header.number != admitted.declared_number {
        return Err(ValidationError::Rejected(format!(
            "decoded block number {} does not match streamed block {}",
            block.header.number, admitted.declared_number,
        )));
    }
    if block.header.parent_hash != admitted.claimed_parent_hash {
        return Err(ValidationError::Rejected(format!(
            "decoded parent hash {} for block {} does not match streamed parent hash {}",
            block.header.parent_hash, admitted.declared_number, admitted.claimed_parent_hash,
        )));
    }
    let computed_hash = block.header.hash_slow();
    if computed_hash != admitted.claimed_hash {
        return Err(ValidationError::Rejected(format!(
            "computed block hash {computed_hash} for block {} does not match streamed block hash {}",
            admitted.declared_number, admitted.claimed_hash,
        )));
    }
    Ok(block)
}

/// Exact-decode, match stream metadata, and recover transaction signers under
/// the configured fork rules. Signer recovery is not execution validation.
fn decode_match_and_recover_signers(
    admitted: &AdmittedBlock,
    chain_spec: &ChainSpec,
) -> Result<RecoveredBlock<Block>, ValidationError> {
    let block = decode_and_match_stream_metadata(admitted)?;
    block.body.transactions.iter().try_for_each(|tx| {
        eez_evm::ensure_supported_transaction(tx).map_err(|error| {
            ValidationError::Rejected(format!("L2 transaction admission: {error}"))
        })
    })?;
    let number = block.header.number;
    recover_block(block, chain_spec).map_err(|error| {
        ValidationError::Rejected(format!(
            "block {number} transaction recovery failed: {error}",
        ))
    })
}

/// Homestead (EIP-2) forbids high-s signatures, so enforce the low-s rule
/// during recovery only for blocks where that fork is active.
fn recover_block(
    block: Block,
    chain_spec: &ChainSpec,
) -> Result<RecoveredBlock<Block>, reth_primitives_traits::transaction::signed::RecoveryError> {
    if chain_spec.is_homestead_active_at_block(block.header.number) {
        RecoveredBlock::try_recover(block)
    } else {
        RecoveredBlock::try_recover_unchecked(block)
    }
}

/// Retain every EEZL2 outbound-event candidate with receipt provenance.
///
/// Matching emitter/signature logs remain observations in receipt order.
/// Canonical encodings expose the emitted hash and manager-entry gas;
/// malformed or non-canonical encodings remain undecoded observations.
fn observe_outbound_events(
    validated_receipts: &[eez_primitives::Receipt],
) -> Vec<OutboundEventObservation> {
    let mut observations = Vec::new();
    for (transaction_index, receipt) in validated_receipts.iter().enumerate() {
        for (receipt_log_index, log) in receipt.logs.iter().enumerate() {
            if log.address != EEZL2_ADDRESS
                || log.data.topics().first() != Some(&CrossChainCallExecuted::SIGNATURE_HASH)
            {
                continue;
            }
            let decoded_event = CrossChainCallExecuted::decode_log_validate(log)
                .ok()
                .filter(|decoded| &CrossChainCallExecuted::encode_log(decoded) == log)
                .map(|decoded| {
                    DecodedOutboundEvent::new(decoded.data.crossChainCallHash, decoded.data.callGas)
                });
            observations.push(OutboundEventObservation {
                transaction_index,
                receipt_log_index,
                decoded_event,
            });
        }
    }
    observations
}

#[cfg(test)]
mod tests;
