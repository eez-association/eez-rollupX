//! A read barrier for Reth's pure post-execution Ethereum assembler.
//!
//! This is not an empty database. Every state operation fails, and even a
//! swallowed error prevents returning a block.

use alloy_primitives::{Address, B256, Bytes, U256};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
    errors::provider::{ProviderError, ProviderResult},
};
use reth_trie::{
    AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState, HashedStorage,
    MultiProof, MultiProofTargets, MultiProofTargetsV2, StorageMultiProof, StorageProof, TrieInput,
    updates::TrieUpdates,
};
use revm::database::BundleState;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub(super) struct NoStateReads(AtomicBool);

impl NoStateReads {
    fn reject<T>(&self) -> ProviderResult<T> {
        self.0.store(true, Ordering::Relaxed);
        Err(ProviderError::UnsupportedProvider)
    }

    pub(super) fn ensure_unused(&self) -> eyre::Result<()> {
        eyre::ensure!(
            !self.0.load(Ordering::Relaxed),
            "pinned block assembler unexpectedly accessed state"
        );
        Ok(())
    }
}

impl AccountReader for NoStateReads {
    fn basic_account(&self, _: &Address) -> ProviderResult<Option<Account>> {
        self.reject()
    }
}
impl BlockHashReader for NoStateReads {
    fn block_hash(&self, _: u64) -> ProviderResult<Option<B256>> {
        self.reject()
    }
    fn canonical_hashes_range(&self, _: u64, _: u64) -> ProviderResult<Vec<B256>> {
        self.reject()
    }
}
impl BytecodeReader for NoStateReads {
    fn bytecode_by_hash(&self, _: &B256) -> ProviderResult<Option<Bytecode>> {
        self.reject()
    }
}
impl StateProvider for NoStateReads {
    fn storage(&self, _: Address, _: B256) -> ProviderResult<Option<U256>> {
        self.reject()
    }
}
impl HashedPostStateProvider for NoStateReads {
    fn hashed_post_state(&self, _: &BundleState) -> ProviderResult<HashedPostState> {
        self.reject()
    }
}
impl StateRootProvider for NoStateReads {
    fn state_root(&self, _: HashedPostState) -> ProviderResult<B256> {
        self.reject()
    }
    fn state_root_from_nodes(&self, _: TrieInput) -> ProviderResult<B256> {
        self.reject()
    }
    fn state_root_with_updates(&self, _: HashedPostState) -> ProviderResult<(B256, TrieUpdates)> {
        self.reject()
    }
    fn state_root_from_nodes_with_updates(
        &self,
        _: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.reject()
    }
}
impl StorageRootProvider for NoStateReads {
    fn storage_root(&self, _: Address, _: HashedStorage) -> ProviderResult<B256> {
        self.reject()
    }
    fn storage_proof(&self, _: Address, _: B256, _: HashedStorage) -> ProviderResult<StorageProof> {
        self.reject()
    }
    fn storage_multiproof(
        &self,
        _: Address,
        _: &[B256],
        _: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.reject()
    }
}
impl StateProofProvider for NoStateReads {
    fn proof(&self, _: TrieInput, _: Address, _: &[B256]) -> ProviderResult<AccountProof> {
        self.reject()
    }
    fn multiproof(&self, _: TrieInput, _: MultiProofTargets) -> ProviderResult<MultiProof> {
        self.reject()
    }
    fn multiproof_v2(
        &self,
        _: TrieInput,
        _: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        self.reject()
    }
    fn witness(
        &self,
        _: TrieInput,
        _: HashedPostState,
        _: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        self.reject()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_or_swallowed_state_access_invalidates_assembly() {
        let provider = NoStateReads::default();
        provider.ensure_unused().unwrap();
        assert!(provider.basic_account(&Address::ZERO).is_err());
        assert!(provider.ensure_unused().is_err());
        let provider = NoStateReads::default();
        let _ignored = provider.hashed_post_state(&BundleState::default());
        assert!(provider.ensure_unused().is_err());
    }
}
