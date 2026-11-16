//! Exact in-memory parent provider for production capture tests. Trie proofs are
//! computed by Reth; unsupported provider operations fail rather than return stubs.
use alloy_consensus::Header;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use reth_primitives_traits::Account;
use reth_primitives_traits::{Bytecode, SealedHeader};
use reth_storage_api::errors::provider::ProviderResult;
use reth_storage_api::*;
use reth_trie::bundle_state::BundleState;
use reth_trie::{
    AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState, HashedStorage,
    MultiProof, MultiProofTargets, MultiProofTargetsV2, TrieInput,
};
use reth_trie::{
    hashed_cursor::{HashedPostStateCursorFactory, noop::NoopHashedCursorFactory},
    trie_cursor::noop::NoopTrieCursorFactory,
};
use std::{collections::BTreeMap, ops::RangeBounds};
#[derive(Clone, Debug)]
pub struct Provider {
    pub accounts: BTreeMap<Address, Data>,
    pub parent: Header,
    pub headers: BTreeMap<B256, Header>,
}
#[derive(Clone, Debug)]
pub struct Data {
    pub nonce: u64,
    pub balance: U256,
    pub code: Bytes,
    pub slots: BTreeMap<U256, U256>,
}
impl Data {
    pub fn account(&self) -> Account {
        Account {
            nonce: self.nonce,
            balance: self.balance,
            bytecode_hash: (!self.code.is_empty()).then(|| keccak256(&self.code)),
        }
    }
}
impl Provider {
    pub fn hashed(&self) -> HashedPostState {
        let mut h = HashedPostState::default();
        for (a, d) in &self.accounts {
            let a = keccak256(a);
            h.accounts.insert(a, Some(d.account()));
            h.storages.insert(
                a,
                HashedStorage::from_iter(
                    d.slots
                        .iter()
                        .map(|(k, v)| (keccak256(k.to_be_bytes::<32>()), *v)),
                ),
            );
        }
        h
    }
    pub fn root(&self, delta: HashedPostState) -> B256 {
        let mut h = self.hashed();
        h.extend(delta);
        let sorted = h.into_sorted();
        reth_trie::StateRoot::new(
            NoopTrieCursorFactory::default(),
            HashedPostStateCursorFactory::new(NoopHashedCursorFactory::default(), &sorted),
        )
        .root()
        .unwrap()
    }
}
impl AccountReader for Provider {
    fn basic_account(&self, a: &Address) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.get(a).map(Data::account))
    }
}
impl HashedPostStateProvider for Provider {
    fn hashed_post_state(&self, bundle: &BundleState) -> ProviderResult<HashedPostState> {
        let mut h =
            HashedPostState::from_bundle_state::<reth_trie::KeccakKeyHasher>(bundle.state());
        // Use the same parent-storage reset expansion as LatestStateProvider, with complete
        // in-memory hashed cursors instead of MDBX cursors.
        let sorted = self.hashed().into_sorted();
        reth_trie::hashed_cursor::zero_destroyed_account_storage(
            &HashedPostStateCursorFactory::new(NoopHashedCursorFactory::default(), &sorted),
            bundle.state(),
            &mut h,
        )?;
        Ok(h)
    }
}
impl StateProofProvider for Provider {
    fn proof(&self, _: TrieInput, _: Address, _: &[B256]) -> ProviderResult<AccountProof> {
        panic!("unused proof entrypoint")
    }
    fn multiproof(&self, _: TrieInput, _: MultiProofTargets) -> ProviderResult<MultiProof> {
        panic!("unused multiproof entrypoint")
    }
    fn multiproof_v2(
        &self,
        _: MultiProofTargetsV2,
        _: bool,
    ) -> ProviderResult<DecodedMultiProofV2> {
        panic!("unused multiproof v2 entrypoint")
    }
    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        assert!(
            input.nodes.is_empty() && input.state.is_empty() && input.prefix_sets.is_empty(),
            "probe supports exact parent state only"
        );
        let sorted = self.hashed().into_sorted();
        Ok(reth_trie::witness::TrieWitness::new(
            NoopTrieCursorFactory::default(),
            HashedPostStateCursorFactory::new(NoopHashedCursorFactory::default(), &sorted),
        )
        .with_execution_witness_mode(mode)
        .compute(target)
        .unwrap()
        .into_values()
        .collect())
    }
}

impl Provider {
    pub fn new(genesis: &Genesis, ancestors: &[Vec<u8>]) -> Self {
        use alloy_rlp::Decodable as _;
        let headers: BTreeMap<_, _> = ancestors
            .iter()
            .map(|raw| {
                let mut bytes = raw.as_slice();
                let header = Header::decode(&mut bytes).unwrap();
                assert!(bytes.is_empty());
                (header.hash_slow(), header)
            })
            .collect();
        let parent = headers.values().max_by_key(|h| h.number).unwrap().clone();
        let accounts = genesis
            .alloc
            .iter()
            .map(|(address, account)| {
                (
                    *address,
                    Data {
                        nonce: account.nonce.unwrap_or(0),
                        balance: account.balance,
                        code: account.code.clone().unwrap_or_default(),
                        slots: account
                            .storage
                            .iter()
                            .flatten()
                            .map(|(key, value)| {
                                (U256::from_be_bytes(key.0), U256::from_be_bytes(value.0))
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        let provider = Self {
            accounts,
            parent,
            headers,
        };
        assert_eq!(
            provider.root(Default::default()),
            provider.parent.state_root
        );
        provider
    }
}
impl BytecodeReader for Provider {
    fn bytecode_by_hash(&self, hash: &B256) -> ProviderResult<Option<Bytecode>> {
        Ok(self
            .accounts
            .values()
            .find(|a| keccak256(&a.code) == *hash)
            .map(|a| Bytecode(evm2::bytecode::Bytecode::from_stored(a.code.clone()))))
    }
}
impl StateProvider for Provider {
    fn storage(&self, address: Address, key: B256) -> ProviderResult<Option<U256>> {
        Ok(self
            .accounts
            .get(&address)
            .and_then(|a| a.slots.get(&U256::from_be_bytes(key.0)))
            .copied())
    }
}
impl BlockHashReader for Provider {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        Ok(self
            .headers
            .values()
            .find(|h| h.number == number)
            .map(Header::hash_slow))
    }
    fn canonical_hashes_range(&self, _: u64, _: u64) -> ProviderResult<Vec<B256>> {
        panic!("unused canonical range")
    }
}
impl StateRootProvider for Provider {
    fn state_root(&self, state: HashedPostState) -> ProviderResult<B256> {
        Ok(self.root(state))
    }
    fn state_root_from_nodes(&self, _: TrieInput) -> ProviderResult<B256> {
        panic!("unused state root nodes")
    }
    fn state_root_with_updates(
        &self,
        _: HashedPostState,
    ) -> ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        panic!("unused trie updates")
    }
    fn state_root_from_nodes_with_updates(
        &self,
        _: TrieInput,
    ) -> ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        panic!("unused trie updates nodes")
    }
}
impl StorageRootProvider for Provider {
    fn storage_root(&self, _: Address, _: HashedStorage) -> ProviderResult<B256> {
        panic!("unused storage root")
    }
    fn storage_proof(
        &self,
        _: Address,
        _: B256,
        _: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        panic!("unused storage proof")
    }
    fn storage_multiproof(
        &self,
        _: Address,
        _: &[B256],
        _: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageMultiProof> {
        panic!("unused storage multiproof")
    }
}
impl HeaderProvider for Provider {
    type Header = Header;
    fn header(&self, hash: B256) -> ProviderResult<Option<Header>> {
        Ok(self.headers.get(&hash).cloned())
    }
    fn header_by_number(&self, _: u64) -> ProviderResult<Option<Header>> {
        panic!("capture must follow exact hash ancestry")
    }
    fn headers_range(&self, _: impl RangeBounds<u64>) -> ProviderResult<Vec<Header>> {
        panic!("capture must follow exact hash ancestry")
    }
    fn sealed_header(&self, _: u64) -> ProviderResult<Option<SealedHeader>> {
        panic!("unused sealed header")
    }
    fn sealed_headers_while(
        &self,
        _: impl RangeBounds<u64>,
        _: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        panic!("unused sealed headers")
    }
}
impl BlockNumReader for Provider {
    fn chain_info(&self) -> ProviderResult<reth_chainspec::ChainInfo> {
        panic!("unused chain info")
    }
    fn best_block_number(&self) -> ProviderResult<u64> {
        panic!("unused best number")
    }
    fn last_block_number(&self) -> ProviderResult<u64> {
        panic!("unused last number")
    }
    fn block_number(&self, _: B256) -> ProviderResult<Option<u64>> {
        panic!("unused block number")
    }
}
impl BlockIdReader for Provider {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        panic!("unused pending id")
    }
    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        panic!("unused safe id")
    }
    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        panic!("unused finalized id")
    }
}
impl StateProviderFactory for Provider {
    type Primitives = eez_primitives::EezPrimitives;
    fn state_by_block_hash(&self, hash: B256) -> ProviderResult<StateProviderBox> {
        assert_eq!(
            hash,
            self.parent.hash_slow(),
            "only the exact fixture parent has state"
        );
        Ok(Box::new(self.clone()))
    }
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        panic!("capture must request exact parent")
    }
    fn state_with_block_appended(
        &self,
        _: B256,
        _: reth_chain_state::ExecutedBlock<Self::Primitives>,
    ) -> ProviderResult<StateProviderBox> {
        panic!("unused appended state")
    }
    fn state_by_block_number_or_tag(
        &self,
        _: alloy_eips::BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        panic!("unused numbered state")
    }
    fn history_by_block_number(&self, _: u64) -> ProviderResult<StateProviderBox> {
        panic!("unused numbered history")
    }
    fn history_by_block_hash(&self, _: B256) -> ProviderResult<StateProviderBox> {
        panic!("unused history")
    }
    fn pending(&self) -> ProviderResult<StateProviderBox> {
        panic!("unused pending")
    }
    fn pending_state_by_hash(&self, _: B256) -> ProviderResult<Option<StateProviderBox>> {
        panic!("unused pending hash")
    }
    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
        panic!("unused maybe pending")
    }
}
