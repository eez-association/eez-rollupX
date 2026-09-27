//! Per-block execution-witness generation for the proving feed (Phase 0a).
//!
//! Replicates reth's `debug_executionWitness` path as a standalone
//! function so the block committer can call it once per canonical block:
//!   1. re-execute the block on its parent state → exact access set,
//!   2. record it (`ExecutionWitnessRecord`, cheap cache walk, no trie),
//!   3. one batched trie pass → the trie nodes + ancestor headers.
//!
//! Mirrors `reth_engine_tree`'s invalid-block witness hook
//! (`re_execute_block`) and `DebugApi::debug_execution_witness_for_block`.
//! The node's prover feed defaults to `ExecutionWitnessMode::Legacy`
//! (ZisK's `native-validate` needs it for outbound/cross-chain blocks);
//! `Canonical` is the minimal witness (sorted, deduped, no empty nodes).
//!
//! Why re-execute instead of capturing during the build: the witness must
//! be **exact** (the prover requires a minimal witness), and only an
//! execution of the *final* tx list yields that. The Normal build path
//! (mempool fill) warms a superset; `newPayload` validation would be exact
//! but lives in reth's engine tree (a deep fork). Re-executing the
//! canonical block here is `debug_executionWitness`'s own approach and is
//! cheap for small L2 blocks.

use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_rpc_types_debug::ExecutionWitness;
use eez_prover::BlockWitness;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{ConfigureEvm, execute::Executor};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::RecoveredBlock;
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord};
use reth_storage_api::{
    EvmStateProviderAdapter, HashedPostStateProvider, HeaderProvider, StateProviderFactory,
};
use reth_trie::{HashedPostState, HashedStorage};
use revm::database::State;
use std::collections::HashSet;
use tracing::{debug, trace};

use crate::observation::{BlockObservation, ObservedOutcome, replay_block_with_observations};
// Re-exported so the prover-feed binary can pick a mode without depending on reth-trie.
pub use reth_trie::ExecutionWitnessMode;

/// Re-execute `block` on its parent state and produce its execution witness
/// (trie nodes + contract codes + key preimages + ancestor headers).
///
/// The endpoint witness is augmented with a removals-first closure over every
/// touched storage slot. EEZ's prover recomputes intermediate per-transaction
/// state roots, not only the final block root. A parent->final witness can be
/// sufficient for whole-block stateless execution while still missing sibling
/// nodes needed to collapse a storage trie after an intermediate deletion. The
/// extra closure supplies those content-addressed nodes without changing the
/// consumer: validators still compute roots independently and reject corrupted
/// or unrelated nodes by hash mismatch.
///
/// `provider` must expose the block's parent state. When the parent is an
/// in-memory (not-yet-persisted) block, the memory-overlay provider folds
/// the parent's `TrieInput` in internally — no manual `TrieInput` needed.
///
/// The block's txs are executed exactly as committed, so the recorded
/// access set is exact; the removal-closure augmentation is intentionally a
/// proof superset for intermediate roots.
/// Build the full [`BlockWitness`] for an already-recovered block: its consensus
/// RLP plus the augmented execution witness the prover's re-execution needs.
///
/// The caller supplies the block — freshly built (the composer's Sync block, not
/// yet committed) or fetched from the provider (the node's commit-time capture) —
/// and this re-executes it on its parent's state to record the witness. Single
/// source of truth for turning a block into prover input.
///
/// # Errors
///
/// Propagates re-execution / witness-augmentation failures.
pub fn block_witness<P, E>(
    provider: &P,
    evm_config: &E,
    block: &RecoveredBlock<Block>,
    mode: ExecutionWitnessMode,
) -> eyre::Result<BlockWitness>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
    E: ConfigureEvm<Primitives = EthPrimitives>,
{
    let witness = block_execution_witness(provider, evm_config, block, mode)?;
    Ok(BlockWitness {
        number: block.header().number(),
        hash: block.hash(),
        parent_hash: block.header().parent_hash(),
        rlp: alloy_rlp::encode(block.clone().into_sealed_block().into_block()).into(),
        witness,
    })
}

/// Build a terminal block witness augmented with every state read needed to
/// replay lookup/static observations.
///
/// The synthetic calls are executed at their exact transaction prefixes and
/// never committed. Their reads become part of the authenticated witness while
/// the block's canonical post-state remains unchanged. The independently
/// replayed outcomes are returned alongside the witness so the caller can
/// reject claims that already disagree locally.
pub fn block_witness_with_observations<P>(
    provider: &P,
    evm_config: &EthEvmConfig,
    block: &RecoveredBlock<Block>,
    mode: ExecutionWitnessMode,
    observations: &[BlockObservation],
) -> eyre::Result<(BlockWitness, Vec<ObservedOutcome>)>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
{
    let (witness, outcomes) =
        block_execution_witness_with_observations(provider, evm_config, block, mode, observations)?;
    Ok((
        BlockWitness {
            number: block.header().number(),
            hash: block.hash(),
            parent_hash: block.header().parent_hash(),
            rlp: alloy_rlp::encode(block.clone().into_sealed_block().into_block()).into(),
            witness,
        },
        outcomes,
    ))
}

/// Replay a terminal block and its observations without constructing a trie
/// witness. Mock-prover configurations use this path so Composer still checks
/// source-claimed outcomes locally.
///
/// # Errors
///
/// Propagates parent-state lookup, block execution, and observation failures.
pub fn block_observation_outcomes<P>(
    provider: &P,
    evm_config: &EthEvmConfig,
    block: &RecoveredBlock<Block>,
    observations: &[BlockObservation],
) -> eyre::Result<Vec<ObservedOutcome>>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
{
    let parent_hash = block.header().parent_hash();
    let has_bal = block.header().block_access_list_hash().is_some();
    let mut state = State::builder()
        .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
            provider.state_by_block_hash(parent_hash)?,
        )))
        .with_bundle_update()
        .with_bal_builder_if(has_bal)
        .build();
    replay_block_with_observations(&mut state, evm_config, block, observations)
}

pub fn block_execution_witness<P, E>(
    provider: &P,
    evm_config: &E,
    block: &RecoveredBlock<Block>,
    mode: ExecutionWitnessMode,
) -> eyre::Result<ExecutionWitness>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
    E: ConfigureEvm<Primitives = EthPrimitives>,
{
    let parent_hash = block.header().parent_hash();
    let block_number = block.header().number();
    trace!(
        target: "eez::witness",
        block_number,
        %parent_hash,
        "block_execution_witness: re-executing block on parent state",
    );

    // 1. Re-execute the exact canonical block on the parent state. The
    //    executor builds its own `State` over the parent provider; reads
    //    warm its cache → that cache is the exact access set.
    let mut executor = evm_config.batch_executor(StateProviderDatabase::new(
        EvmStateProviderAdapter(provider.state_by_block_hash(parent_hash)?),
    ));
    executor.execute_one(block)?;
    let state = executor.into_state();

    execution_witness_from_state(provider, block, mode, &state)
}

fn block_execution_witness_with_observations<P>(
    provider: &P,
    evm_config: &EthEvmConfig,
    block: &RecoveredBlock<Block>,
    mode: ExecutionWitnessMode,
    observations: &[BlockObservation],
) -> eyre::Result<(ExecutionWitness, Vec<ObservedOutcome>)>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
{
    let parent_hash = block.header().parent_hash();
    let has_bal = block.header().block_access_list_hash().is_some();
    let mut state = State::builder()
        .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
            provider.state_by_block_hash(parent_hash)?,
        )))
        .with_bundle_update()
        .with_bal_builder_if(has_bal)
        .build();
    let outcomes = replay_block_with_observations(&mut state, evm_config, block, observations)?;
    let witness = execution_witness_from_state(provider, block, mode, &state)?;
    Ok((witness, outcomes))
}

fn execution_witness_from_state<P, DB>(
    provider: &P,
    block: &RecoveredBlock<Block>,
    mode: ExecutionWitnessMode,
    state: &State<DB>,
) -> eyre::Result<ExecutionWitness>
where
    P: StateProviderFactory + HeaderProvider<Header = Header>,
{
    let parent_hash = block.header().parent_hash();
    let block_number = block.header().number();

    // Record the access set (cheap: cache walk, no trie).
    let record = ExecutionWitnessRecord::new(state);

    // One batched trie pass against the parent → trie nodes + headers.
    //    Re-open the parent provider (the first was consumed by the executor).
    let state_provider = provider.state_by_block_hash(parent_hash)?;
    let accesses = witness_access_state(state, state_provider.as_ref())?;
    let (account_removal_state, removal_accounts) = touched_account_removal_state(&accesses);
    let (storage_removal_state, removal_slots) = touched_storage_removal_state(&accesses)?;
    let mut witness =
        record.into_execution_witness(state_provider.as_ref(), provider, block_number, mode)?;
    let endpoint_state_nodes = witness.state.len();

    // EEZ-specific augmentation: two removals-first witness passes emit the
    //    trie collapse siblings the prover's sparse trie needs but the final
    //    post-state witness omits.
    //    (a) account-collapse: an account deleted mid-block and re-funded before
    //        block end (EIP-6780 CREATE2+SELFDESTRUCT+refund) shows a present
    //        end-state that masks the deletion, so stage every touched account as
    //        a removal to force the sibling.
    //    (b) storage-collapse: a slot cleared mid-block needs its branch sibling.
    //    Separate passes: staging an account as removed discards its storage
    //    trie, which would suppress (b). Both are content-addressed → superset-safe.
    let mut extra_state_nodes = 0usize;
    if removal_accounts > 0 {
        let extra = state_provider.as_ref().witness(
            Default::default(),
            account_removal_state,
            ExecutionWitnessMode::Legacy,
        )?;
        extra_state_nodes += extend_unique_by_hash(&mut witness.state, extra);
    }
    if removal_slots > 0 {
        let extra = state_provider.as_ref().witness(
            Default::default(),
            storage_removal_state,
            ExecutionWitnessMode::Legacy,
        )?;
        extra_state_nodes += extend_unique_by_hash(&mut witness.state, extra);
    }

    debug!(
        target: "eez::witness",
        block_number,
        state_nodes = witness.state.len(),
        endpoint_state_nodes,
        removal_accounts,
        removal_slots,
        extra_state_nodes,
        codes = witness.codes.len(),
        keys = witness.keys.len(),
        headers = witness.headers.len(),
        "block_execution_witness: generated augmented witness",
    );
    Ok(witness)
}

/// Every touched account staged as a removal (`None`), so reth fetches the
/// account-trie collapse siblings even for accounts whose end-state is present
/// (deleted mid-block then re-funded). Returns the state and the account count.
fn touched_account_removal_state(accesses: &HashedPostState) -> (HashedPostState, usize) {
    let mut state = HashedPostState::default();
    for &hashed_address in accesses.accounts.keys() {
        state.accounts.insert(hashed_address, None);
    }
    let count = state.accounts.len();
    (state, count)
}

/// Every touched storage slot staged as removed, account kept present, so reth
/// fetches the storage-trie collapse siblings. Returns the state and slot count.
fn touched_storage_removal_state(
    accesses: &HashedPostState,
) -> eyre::Result<(HashedPostState, usize)> {
    let mut state = HashedPostState::with_capacity(accesses.storages.len());
    let mut slot_count = 0usize;

    for (&hashed_address, storage) in &accesses.storages {
        if storage.storage.is_empty() {
            continue;
        }

        let account = accesses
            .accounts
            .get(&hashed_address)
            .copied()
            .ok_or_else(|| {
                eyre::eyre!(
                    "execution witness record has storage changes for {hashed_address} \
                     but no matching account entry"
                )
            })?;
        state.accounts.insert(hashed_address, account);

        let mut removals = HashedStorage::default();
        for &hashed_slot in storage.storage.keys() {
            removals.storage.insert(hashed_slot, U256::ZERO);
            slot_count += 1;
        }
        state.storages.insert(hashed_address, removals);
    }

    Ok((state, slot_count))
}

/// Mirror Reth's private witness access collection for EEZ's prefix-root closure.
/// Include read-only cached slots as well as the provider's expanded storage
/// resets; final bundle values take precedence over the execution cache.
fn witness_access_state<DB>(
    state: &State<DB>,
    provider: &(impl HashedPostStateProvider + ?Sized),
) -> eyre::Result<HashedPostState> {
    let mut accesses = HashedPostState::default();
    for (address, account) in &state.cache.accounts {
        let hashed_address = keccak256(address);
        accesses.accounts.insert(
            hashed_address,
            account.account.as_ref().map(|a| (&a.info).into()),
        );
        let storage = accesses.storages.entry(hashed_address).or_default();
        if let Some(account) = &account.account {
            for (slot, value) in &account.storage {
                storage.storage.insert(keccak256(B256::from(*slot)), *value);
            }
        }
    }
    accesses.extend(provider.hashed_post_state(&state.bundle_state)?);
    Ok(accesses)
}

fn extend_unique_by_hash(target: &mut Vec<Bytes>, extra: impl IntoIterator<Item = Bytes>) -> usize {
    let mut seen: HashSet<B256> = target.iter().map(keccak256).collect();
    let mut added = 0usize;

    for node in extra {
        if seen.insert(keccak256(&node)) {
            target.push(node);
            added += 1;
        }
    }

    added
}

#[cfg(test)]
mod tests {
    use super::{
        B256, Block, Bytes, ExecutionWitnessMode, HashedPostState, HashedStorage, Header, U256,
        block_execution_witness, block_execution_witness_with_observations, extend_unique_by_hash,
        touched_account_removal_state, touched_storage_removal_state,
    };
    use crate::observation::BlockObservation;
    use alloy_primitives::{Address, address};
    use eez_protocol::settlement::FifoObservationClaim;
    use eez_protocol::{CallMode, RollupId};
    use reth_chainspec::ChainSpecBuilder;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use std::sync::Arc;

    const MANAGER: Address = address!("4200000000000000000000000000000000000007");
    const TARGET: Address = address!("0000000000000000000000000000000000001234");

    #[test]
    fn removal_closure_keeps_read_only_slots_and_expanded_storage_resets() {
        use reth_storage_api::{HashedPostStateProvider, errors::provider::ProviderResult};
        use revm::database::{BundleState, EmptyDB, State, states::CacheAccount};
        use revm::state::AccountInfo;

        struct ExpandedState(HashedPostState);
        impl HashedPostStateProvider for ExpandedState {
            fn hashed_post_state(&self, _: &BundleState) -> ProviderResult<HashedPostState> {
                Ok(self.0.clone())
            }
        }

        let read_address = Address::with_last_byte(1);
        let destroyed_address = Address::with_last_byte(2);
        let read_slot = U256::from(3);
        let destroyed_slot = alloy_primitives::keccak256(B256::from(U256::from(4)));
        let read_hash = alloy_primitives::keccak256(read_address);
        let destroyed_hash = alloy_primitives::keccak256(destroyed_address);
        let mut executed = State::builder().with_database(EmptyDB::default()).build();
        executed.cache.accounts.insert(
            read_address,
            CacheAccount::new_loaded(
                AccountInfo::default(),
                [(read_slot, U256::from(7))].into_iter().collect(),
            ),
        );
        // A destroyed account's old slot is absent from the execution cache;
        // the parent provider expands it into an explicit zero write.
        let provider = ExpandedState(
            HashedPostState::default()
                .with_accounts([(destroyed_hash, None)])
                .with_storages([(
                    destroyed_hash,
                    HashedStorage::from_iter([(destroyed_slot, U256::ZERO)]),
                )]),
        );
        let accesses = super::witness_access_state(&executed, &provider).unwrap();
        let (accounts, count) = touched_account_removal_state(&accesses);
        assert_eq!(count, 2);
        assert_eq!(accounts.accounts.get(&read_hash), Some(&None));
        assert_eq!(accounts.accounts.get(&destroyed_hash), Some(&None));
        let (storage, count) = touched_storage_removal_state(&accesses).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            storage.storages[&read_hash].storage
                [&alloy_primitives::keccak256(B256::from(read_slot))],
            U256::ZERO,
        );
        assert_eq!(
            storage.storages[&destroyed_hash].storage[&destroyed_slot],
            U256::ZERO
        );
    }

    /// Every touched account is staged as a removal (`None`) — including one whose
    /// end-state is present (the delete-then-refund case) — so reth fetches the
    /// account-collapse siblings.
    #[test]
    fn account_removal_stages_all_as_none() {
        let present = B256::repeat_byte(0x11); // present end-state (masks mid-block delete)
        let destroyed = B256::repeat_byte(0x22); // destroyed end-state
        let mut hashed_state = HashedPostState::default();
        hashed_state
            .accounts
            .insert(present, Some(Default::default()));
        hashed_state.accounts.insert(destroyed, None);
        let (state, count) = touched_account_removal_state(&hashed_state);
        assert_eq!(count, 2);
        assert!(matches!(state.accounts.get(&present), Some(None)));
        assert!(matches!(state.accounts.get(&destroyed), Some(None)));
    }

    /// Storage removals keep the account present (so reth processes its storage
    /// trie) and stage each touched slot as zero.
    #[test]
    fn storage_removal_stages_slots_zero_account_present() {
        let addr = B256::repeat_byte(0x11);
        let (s1, s2) = (B256::repeat_byte(0xaa), B256::repeat_byte(0xbb));
        let mut hashed_state = HashedPostState::default();
        hashed_state.accounts.insert(addr, Some(Default::default()));
        let mut storage = HashedStorage::default();
        storage.storage.insert(s1, U256::from(5));
        storage.storage.insert(s2, U256::from(7));
        hashed_state.storages.insert(addr, storage);
        let (state, slots) = touched_storage_removal_state(&hashed_state).unwrap();
        assert_eq!(slots, 2);
        assert!(matches!(state.accounts.get(&addr), Some(Some(_)))); // account kept present
        let st = state.storages.get(&addr).unwrap();
        assert_eq!(st.storage.get(&s1), Some(&U256::ZERO));
        assert_eq!(st.storage.get(&s2), Some(&U256::ZERO));
    }

    /// The removal-closure augmentation dedups by node hash: only genuinely new
    /// nodes are appended, and the reported count matches.
    #[test]
    fn extend_unique_by_hash_dedups() {
        let mut target = vec![Bytes::from_static(b"a"), Bytes::from_static(b"b")];
        let added = extend_unique_by_hash(
            &mut target,
            [
                Bytes::from_static(b"b"), // dup of existing
                Bytes::from_static(b"c"), // new
                Bytes::from_static(b"c"), // dup within extra
            ],
        );
        assert_eq!(added, 1, "only 'c' is new");
        assert_eq!(target.len(), 3);
        assert_eq!(target[2], Bytes::from_static(b"c"));
    }

    #[test]
    fn observation_witness_includes_accounts_and_code_read_only_by_the_observation() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());
        let evm_config = EthEvmConfig::new(Arc::clone(&chain_spec));
        let provider: MockEthProvider = MockEthProvider::new();

        let proxy = Address::repeat_byte(0x55);
        let mut manager_code = vec![0x73]; // PUSH20 proxy
        manager_code.extend_from_slice(proxy.as_slice());
        manager_code.extend_from_slice(&[0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
        let manager_code = Bytes::from(manager_code);
        let target_code =
            Bytes::from_static(&[0x60, 0x2a, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
        provider.add_account(
            MANAGER,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(manager_code.clone()),
        );
        provider.add_account(
            TARGET,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(target_code.clone()),
        );

        let parent = Header {
            number: 1,
            timestamp: 1_000,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(0),
            ..Default::default()
        };
        let parent_hash = parent.hash_slow();
        provider.add_header(parent_hash, parent);
        let block = RecoveredBlock::new_unhashed(
            Block::new(
                Header {
                    parent_hash,
                    number: 2,
                    timestamp: 1_012,
                    gas_limit: 30_000_000,
                    base_fee_per_gas: Some(0),
                    ..Default::default()
                },
                Default::default(),
            ),
            Vec::new(),
        );
        let observation = BlockObservation {
            after_transactions: 0,
            manager_address: MANAGER,
            claim: FifoObservationClaim {
                pinned_block_hash: parent_hash,
                call_mode: CallMode::Static,
                target_address: TARGET,
                source_address: Address::repeat_byte(0x44),
                source_rollup_id: RollupId::MAINNET,
                data: Bytes::new(),
                value: U256::ZERO,
                expected_success: true,
                expected_output: U256::from(42).to_be_bytes_vec().into(),
            },
        };

        let ordinary =
            block_execution_witness(&provider, &evm_config, &block, ExecutionWitnessMode::Legacy)
                .unwrap();
        let (augmented, outcomes) = block_execution_witness_with_observations(
            &provider,
            &evm_config,
            &block,
            ExecutionWitnessMode::Legacy,
            &[observation],
        )
        .unwrap();

        assert!(!ordinary.codes.contains(&manager_code));
        assert!(!ordinary.codes.contains(&target_code));
        assert!(augmented.codes.contains(&manager_code));
        assert!(augmented.codes.contains(&target_code));
        assert_eq!(
            outcomes,
            vec![crate::observation::ObservedOutcome {
                success: true,
                output: U256::from(42).to_be_bytes_vec().into(),
            }]
        );
    }
}
