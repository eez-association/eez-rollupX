//! Inject state-read failures without replacing session construction or execution.
use crate::local::provider::StateSnapshotProvider;
use alloy_primitives::{Address, B256, Bytes, U256};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::test_utils::MockEthProvider;
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateProviderBox, StateRootProvider, StorageRootProvider,
    errors::provider::{ProviderError, ProviderResult},
};
use reth_trie_common::{
    AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState, HashedStorage,
    MultiProof, MultiProofTargets, MultiProofTargetsV2, StorageMultiProof, StorageProof, TrieInput,
    updates::TrieUpdates,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone)]
pub(super) struct FaultProvider {
    pub inner: MockEthProvider,
    pub fail_storage_at: Address,
    pub failing: Arc<AtomicBool>,
    pub target_reads: Arc<AtomicUsize>,
}
impl StateSnapshotProvider for FaultProvider {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }
    fn state_by_block_hash(&self, _: B256) -> ProviderResult<StateProviderBox> {
        self.latest()
    }
}
impl AccountReader for FaultProvider {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if *address == super::TARGET {
            self.target_reads.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.basic_account(address)
    }
}
impl BlockHashReader for FaultProvider {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }
    fn canonical_hashes_range(&self, start: u64, end: u64) -> ProviderResult<Vec<B256>> {
        self.inner.canonical_hashes_range(start, end)
    }
}
impl BytecodeReader for FaultProvider {
    fn bytecode_by_hash(&self, hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.inner.bytecode_by_hash(hash)
    }
}
impl StateProvider for FaultProvider {
    fn storage(&self, address: Address, key: B256) -> ProviderResult<Option<U256>> {
        if address == self.fail_storage_at && self.failing.load(Ordering::Relaxed) {
            return Err(ProviderError::StateForHashNotFound(B256::repeat_byte(0xfa)));
        }
        self.inner.storage(address, key)
    }
}
impl HashedPostStateProvider for FaultProvider {
    fn hashed_post_state(
        &self,
        state: &revm::database::BundleState,
    ) -> ProviderResult<HashedPostState> {
        self.inner.hashed_post_state(state)
    }
}
impl StateRootProvider for FaultProvider {
    fn state_root(&self, state: HashedPostState) -> ProviderResult<B256> {
        self.inner.state_root(state)
    }
    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.inner.state_root_from_nodes(input)
    }
    fn state_root_with_updates(
        &self,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_with_updates(state)
    }
    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_from_nodes_with_updates(input)
    }
}
impl StorageRootProvider for FaultProvider {
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        self.inner.storage_root(address, storage)
    }
    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.inner.storage_proof(address, slot, storage)
    }
    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.inner.storage_multiproof(address, slots, storage)
    }
}
impl StateProofProvider for FaultProvider {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.inner.proof(input, address, slots)
    }
    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.inner.multiproof(input, targets)
    }
    fn multiproof_v2(
        &self,
        input: TrieInput,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        self.inner.multiproof_v2(input, targets)
    }
    fn witness(
        &self,
        input: TrieInput,
        state: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        self.inner.witness(input, state, mode)
    }
}
