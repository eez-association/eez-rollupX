//! Local reth-backed implementation of [`ChainClient`].
//!
//! [`Role`] selects source-simulation capability and the chain-local dispatch
//! address. [`eez_protocol::ChainDialect`] selects the proxy-mapping layout.
//! Each client owns an overlay channel used to propagate in-flight state
//! through same-rollup re-entry.

use std::sync::Arc;

use alloy_primitives::{Address, Bytes};
use eez_evm::EezEvmConfig;
use eez_primitives::EezTxEnvelope as TransactionSigned;
use reth_evm::{ConfigureEvm, Evm as _};
use reth_primitives_traits::SignerRecoverable;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::{
    BlockNumReader, EvmStateProviderAdapter, HeaderProvider, StateProviderBox, StateProviderFactory,
};
use revm::DatabaseCommit;
use revm::context::result::EVMError;

use crate::composer::gas_trace;
use eez_evm_inspector::{OverlayChannelHandle, SessionInspectorFactory, new_overlay_channel};
use eez_protocol::{
    ChainClient, CompositionBuilder, ExecutorError, ExecutorErrorKind, ExecutorResult,
    ProxyLookupConfig, RollupId, TargetExecutionSession,
};

use super::provider::{ChainProvider, HeaderReader};
use super::session::LocalExecutionSession;

/// Composition simulation bypasses proxy consumption on the source chain.
/// Its gas is a planning estimate; final ordered L1 execution remains required.
#[derive(Debug)]
pub(crate) struct SourceSimulation {
    pub success: bool,
    pub gas_used: u64,
    pub output: Bytes,
}

/// Discriminates how this client operates within the composition.
///
/// Both variants carry the contract holding this chain's
/// `authorizedProxies` mapping. The client's dialect supplies the mapping slot.
#[derive(Debug, Clone)]
pub enum Role {
    /// May initiate source simulation and participate through target sessions.
    Entry {
        /// Contract holding `authorizedProxies` on this chain: `EEZ`
        /// on L1 or `EEZL2` on L2.
        dispatch_address: Address,
    },
    /// Cannot initiate source simulation; participates through target sessions.
    Follower {
        /// Contract holding `authorizedProxies` on this chain.
        dispatch_address: Address,
    },
}

impl Role {
    /// Address of the dispatch contract on this chain.
    fn dispatch_address(&self) -> Address {
        match self {
            Role::Entry { dispatch_address } | Role::Follower { dispatch_address } => {
                *dispatch_address
            }
        }
    }
}

/// Unified local chain client.
///
/// Implements [`ChainClient`] for every role. Entry-only behavior checks
/// `Role::Entry` at runtime and returns `Unavailable` for follower clients.
pub struct LocalChainClient {
    /// Type-erased chain provider used by every execution path.
    provider: ChainProvider,
    rollup_id: RollupId,
    role: Role,
    /// Contract dialect used to select the proxy-mapping layout.
    dialect: eez_protocol::ChainDialect,
    /// Bidirectional cache channel used to propagate in-flight state through
    /// nested dispatches that re-enter this rollup.
    overlay_channel: OverlayChannelHandle,
    session_context: Option<(alloy_primitives::B256, reth_evm::EvmEnv)>,
}

impl std::fmt::Debug for LocalChainClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalChainClient")
            .field("rollup_id", &self.rollup_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl LocalChainClient {
    fn build_chain_provider<P>(provider: &P, evm_config: EezEvmConfig) -> ChainProvider
    where
        P: StateProviderFactory
            + HeaderProvider<Header = alloy_consensus::Header>
            + BlockNumReader
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let headers: Arc<dyn HeaderReader> = Arc::new(provider.clone());
        let state_provider: Arc<dyn super::provider::StateSnapshotProvider> =
            Arc::new(provider.clone());
        ChainProvider {
            provider: state_provider,
            headers,
            evm_config,
            gnosis_evm_config: None,
        }
    }

    /// Build an entry-role client.
    pub fn new_entry<P>(
        provider: P,
        evm_config: EezEvmConfig,
        rollup_id: RollupId,
        dispatch_address: Address,
        dialect: eez_protocol::ChainDialect,
    ) -> Arc<Self>
    where
        P: StateProviderFactory
            + HeaderProvider<Header = alloy_consensus::Header>
            + BlockNumReader
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let cp = Self::build_chain_provider(&provider, evm_config);
        Arc::new(Self {
            provider: cp,
            rollup_id,
            role: Role::Entry { dispatch_address },
            dialect,
            overlay_channel: new_overlay_channel(),
            session_context: None,
        })
    }

    /// Retain the node's actual Gnosis executor alongside the header adapter.
    /// Strict posting simulation must include its fee collector and fork
    /// transitions, which an Ethereum executor over Gnosis timestamps omits.
    pub fn new_gnosis_entry<P>(
        provider: P,
        config: reth_gnosis::evm_config::GnosisEvmConfig,
        rollup_id: RollupId,
        dispatch_address: Address,
        dialect: eez_protocol::ChainDialect,
    ) -> Arc<Self>
    where
        P: StateProviderFactory
            + HeaderProvider<Header = alloy_consensus::Header>
            + BlockNumReader
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let evm_config =
            EezEvmConfig::for_l1_simulation(Arc::new(config.chain_spec().inner.clone()));
        let mut cp = Self::build_chain_provider(&provider, evm_config);
        cp.gnosis_evm_config = Some(config);
        Arc::new(Self {
            provider: cp,
            rollup_id,
            role: Role::Entry { dispatch_address },
            dialect,
            overlay_channel: new_overlay_channel(),
            session_context: None,
        })
    }

    /// Build an entry-role view that shares its re-entry overlay with another
    /// client for the same chain.
    ///
    /// Slot composition keeps separate entry/follower roles over one L2
    /// provider. They must share this channel so a target session can return to
    /// the suspended source simulation without reopening from stale chain
    /// state.
    pub fn new_entry_sharing_overlay<P>(
        provider: P,
        evm_config: EezEvmConfig,
        rollup_id: RollupId,
        dispatch_address: Address,
        dialect: eez_protocol::ChainDialect,
        shared_with: &Self,
    ) -> Arc<Self>
    where
        P: StateProviderFactory
            + HeaderProvider<Header = alloy_consensus::Header>
            + BlockNumReader
            + Clone
            + Send
            + Sync
            + 'static,
    {
        assert_eq!(
            rollup_id, shared_with.rollup_id,
            "overlay channels may only be shared by views of the same rollup"
        );
        let cp = Self::build_chain_provider(&provider, evm_config);
        Arc::new(Self {
            provider: cp,
            rollup_id,
            role: Role::Entry { dispatch_address },
            dialect,
            overlay_channel: Arc::clone(&shared_with.overlay_channel),
            session_context: None,
        })
    }

    /// Build a follower-role client.
    pub fn new_follower<P>(
        provider: P,
        evm_config: EezEvmConfig,
        rollup_id: RollupId,
        dispatch_address: Address,
        dialect: eez_protocol::ChainDialect,
    ) -> Arc<Self>
    where
        P: StateProviderFactory
            + HeaderProvider<Header = alloy_consensus::Header>
            + BlockNumReader
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let cp = Self::build_chain_provider(&provider, evm_config);
        Arc::new(Self {
            provider: cp,
            rollup_id,
            role: Role::Follower { dispatch_address },
            dialect,
            overlay_channel: new_overlay_channel(),
            session_context: None,
        })
    }

    /// Build the proxy lookup used by source and target-session inspectors.
    /// The role supplies the chain-local contract address and the dialect
    /// supplies its storage layout.
    fn proxy_lookup_config(&self) -> ProxyLookupConfig {
        ProxyLookupConfig {
            contract_address: self.role.dispatch_address(),
            authorized_proxies_slot: self.dialect.proxy_lookup_slot(),
        }
    }

    /// Reth handles (state factory, headers, `EvmConfig`) backing this chain.
    #[must_use]
    pub fn chain_provider(&self) -> &ChainProvider {
        &self.provider
    }

    /// Contract holding this chain's `authorizedProxies` mapping and driving
    /// cross-chain execution: `EEZ` on L1, `EEZL2` on L2.
    #[must_use]
    pub fn manager_address(&self) -> Address {
        self.role.dispatch_address()
    }

    /// The inspector factory both `begin_execution_session` and
    /// [`Self::simulate_source_tx_on`] build: this chain's proxy lookup, rollup
    /// id, and overlay channel.
    #[must_use]
    pub fn inspector_factory(&self) -> SessionInspectorFactory {
        SessionInspectorFactory::new(
            self.proxy_lookup_config(),
            self.rollup_id,
            Arc::clone(&self.overlay_channel),
        )
    }

    /// Source-simulate a raw tx over a caller-provided live state + env — a
    /// fork of the slot's execution context — COMMITTING the result state into
    /// that fork so later simulations see this tx's writes.
    ///
    /// Entry-role only; the caller owns the env, which must already be derived
    /// from the fork's own header.
    ///
    /// # Errors
    ///
    /// [`ExecutorErrorKind::Unavailable`] on a follower client,
    /// [`ExecutorErrorKind::Decode`] when the raw tx cannot be decoded or its
    /// signer recovered, [`ExecutorErrorKind::Provider`] when the backing store
    /// fails mid-execution, plus any error a nested dispatch raises.
    ///
    /// Returns the source transaction's receipt-level status. A normal EVM
    /// revert is `Ok(false)`, not an executor error.
    pub fn simulate_source_tx_on(
        &self,
        raw_tx: Vec<u8>,
        dispatcher: &mut CompositionBuilder,
        state: &mut State<StateProviderDatabase<EvmStateProviderAdapter<StateProviderBox>>>,
        evm_env: reth_evm::EvmEnvFor<EezEvmConfig>,
    ) -> ExecutorResult<bool> {
        self.simulate_source_tx_on_with_retries(raw_tx, dispatcher, state, evm_env, false)
            .map(|result| result.success)
    }

    /// Independent channels are required for concurrent wallet estimates: the
    /// provider is shared, but in-flight composition overlays must never be.
    pub(crate) fn isolated(
        &self,
        hash: alloy_primitives::B256,
        env: reth_evm::EvmEnv,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider: self.provider.clone(),
            rollup_id: self.rollup_id,
            role: match &self.role {
                Role::Entry { dispatch_address } => Role::Entry {
                    dispatch_address: *dispatch_address,
                },
                Role::Follower { dispatch_address } => Role::Follower {
                    dispatch_address: *dispatch_address,
                },
            },
            dialect: self.dialect,
            overlay_channel: new_overlay_channel(),
            session_context: Some((hash, env)),
        })
    }

    /// Shared source-simulation body: decode, build the tx env, run under the
    /// session inspector, commit.
    pub(crate) fn simulate_source_tx_on_with_retries(
        &self,
        raw_tx: Vec<u8>,
        dispatcher: &mut CompositionBuilder,
        state: &mut State<StateProviderDatabase<EvmStateProviderAdapter<StateProviderBox>>>,
        evm_env: reth_evm::EvmEnvFor<EezEvmConfig>,
        retry_unavailable_conditions: bool,
    ) -> ExecutorResult<SourceSimulation> {
        use alloy_eips::eip2718::Decodable2718;

        // Only entry-role clients are authorized to simulate source
        // transactions. Keep the check here because callers use the uniform
        // `ChainClient` interface for both roles.
        let Role::Entry { .. } = &self.role else {
            return Err(ExecutorError::from(ExecutorErrorKind::Unavailable(
                "simulate_source_tx_on called on follower LocalChainClient".into(),
            )));
        };

        let mut raw: &[u8] = &raw_tx;
        let tx = TransactionSigned::decode_2718(&mut raw)
            .map_err(|e| ExecutorError::from(ExecutorErrorKind::Decode(e.to_string())))?;
        let signer = tx
            .recover_signer()
            .map_err(|e| ExecutorError::from(ExecutorErrorKind::Decode(e.to_string())))?;

        tracing::info!(
            ?signer,
            to = ?alloy_consensus::Transaction::to(&tx),
            "simulating source tx for cross-chain call detection"
        );

        let recovered = reth_primitives_traits::Recovered::new_unchecked(tx, signer);
        let tx_env = self.provider.evm_config.tx_env(&recovered);

        self.simulate_source_env_on(
            tx_env,
            dispatcher,
            state,
            evm_env,
            retry_unavailable_conditions,
        )
    }

    /// Execute a normalized unsigned call using the same composition inspector
    /// as signed admission. The caller owns a disposable, pinned state.
    pub(crate) fn simulate_source_env_on(
        &self,
        tx_env: revm::context::TxEnv,
        dispatcher: &mut CompositionBuilder,
        state: &mut State<StateProviderDatabase<EvmStateProviderAdapter<StateProviderBox>>>,
        evm_env: reth_evm::EvmEnvFor<EezEvmConfig>,
        retry_unavailable_conditions: bool,
    ) -> ExecutorResult<SourceSimulation> {
        if !matches!(self.role, Role::Entry { .. }) {
            return Err(ExecutorErrorKind::Unavailable(
                "source simulation requires entry role".into(),
            )
            .into());
        }
        // The source-simulation inspector dispatches every detected proxy CALL
        // through the composition builder, which records calls in preorder.
        // Attach the cache channel around each downstream dispatch.
        let mut inspector = self.inspector_factory().build(dispatcher);
        let traced = gas_trace::maybe_trace(&mut inspector, evm_env.cfg_env.chain_id);
        let mut evm =
            self.provider
                .evm_config
                .evm_with_env_and_inspector(&mut *state, evm_env, traced);
        let (gas_used, success, output, changes) = match evm.transact(tx_env) {
            Ok(r) => (
                r.result.tx_gas_used(),
                r.result.is_success(),
                r.result.output().cloned().unwrap_or_default(),
                Some(r.state),
            ),
            // The backing store is unreachable — that is the slot's problem, not
            // the tx's, so it must not degrade into an empty composition (which
            // the drain reads as poison and evicts on).
            Err(EVMError::Database(e)) => return Err(ExecutorError::provider(e)),
            Err(EVMError::Transaction(e))
                if retry_unavailable_conditions
                    && matches!(
                        e,
                        revm::context::result::InvalidTransaction::GasPriceLessThanBasefee
                            | revm::context::result::InvalidTransaction::LackOfFundForMaxFee { .. }
                            | revm::context::result::InvalidTransaction::NonceTooHigh { .. }
                    ) =>
            {
                // Fees, funding, or an unexecuted/orphaned predecessor can
                // repair this exact signed source. Keep ownership and nonce
                // ordering until the source becomes executable in this slot.
                use eez_protocol::SourceCondition;
                use revm::context::result::InvalidTransaction;
                let condition = match e {
                    InvalidTransaction::NonceTooHigh { tx, state } => SourceCondition::Nonce {
                        transaction_nonce: tx,
                        required_nonce: state,
                    },
                    InvalidTransaction::LackOfFundForMaxFee { fee, balance } => {
                        SourceCondition::Balance {
                            required: *fee,
                            available: *balance,
                        }
                    }
                    InvalidTransaction::GasPriceLessThanBasefee => SourceCondition::Fee,
                    _ => unreachable!("matched source condition"),
                };
                return Err(ExecutorErrorKind::SourceBlocked(condition).into());
            }
            // Rejected before execution. Same outcome as a
            // revert — no calls, so the drain evicts — but named for what it is.
            Err(EVMError::Transaction(e)) => {
                tracing::warn!(%e, "source tx rejected at validation; it records no cross-chain call");
                (0, false, Bytes::new(), None)
            }
            Err(e) => {
                tracing::warn!(%e, "source sim reverted");
                (0, false, Bytes::new(), None)
            }
        };
        drop(evm);
        let inspector_error = inspector.take_error();

        if let Some(err) = inspector_error {
            return Err(err);
        }
        // The trait-method caller's `State` is function-local, so committing
        // into it is unobservable there; the fork caller needs the writes.
        if let Some(changes) = changes {
            state.commit(changes);
        }

        tracing::info!(gas_used, success, "source simulation complete");
        Ok(SourceSimulation {
            success,
            gas_used,
            output,
        })
    }
}

impl ChainClient for LocalChainClient {
    fn reset_composition_state(&self) {
        self.overlay_channel.reset();
    }

    fn begin_execution_session(&self) -> ExecutorResult<Box<dyn TargetExecutionSession + Send>> {
        tracing::debug!(
            rollup_id = %self.rollup_id,
            manager = %self.role.dispatch_address(),
            role = ?self.role,
            "opening execution session"
        );
        // Inspect every target session because nested proxy calls may dispatch
        // again. The inspector exchanges cache snapshots through this client's
        // configured channel.
        let inspector_factory = Some(self.inspector_factory());
        // Preload the top cache snapshot when one is available.
        let preloaded_cache = self.overlay_channel.peek_pre_snapshot();
        let manager_address = self.role.dispatch_address();
        let session = LocalExecutionSession::new(
            &self.provider,
            manager_address,
            inspector_factory,
            preloaded_cache,
            self.overlay_channel.clone(),
            self.session_context.clone(),
        )?;
        Ok(Box::new(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, TxLegacy};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_network::TxSignerSync;
    use alloy_primitives::{B256, TxKind, U256, address, bytes};
    use alloy_signer_local::PrivateKeySigner;
    use reth_chainspec::ChainSpecBuilder;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    use crate::local::L1SlotState;

    const RECIPIENT: Address = address!("000000000000000000000000000000000000b0b0");
    const REVERTER: Address = address!("000000000000000000000000000000000000bad0");
    const NESTED_CALLER: Address = address!("000000000000000000000000000000000000ca11");
    const NESTED_TARGET: Address = address!("000000000000000000000000000000000000cafe");

    fn signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::with_last_byte(7)).expect("test key")
    }

    fn signed_tx(to: Address) -> Vec<u8> {
        signed_tx_with_nonce(to, 0)
    }

    fn signed_tx_with_nonce(to: Address, nonce: u64) -> Vec<u8> {
        let mut tx = TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 1,
            gas_limit: 100_000,
            to: TxKind::Call(to),
            value: U256::ZERO,
            input: Bytes::new(),
        };
        let signature = signer()
            .sign_transaction_sync(&mut tx)
            .expect("sign test transaction");
        let signed = reth_ethereum_primitives::TransactionSigned::new_unhashed(
            reth_ethereum_primitives::Transaction::Legacy(tx),
            signature,
        );
        let mut raw = Vec::new();
        signed.encode_2718(&mut raw);
        raw
    }

    fn fixture() -> Arc<LocalChainClient> {
        fixture_with_balance(U256::from(1_000_000_000_000_000_000_u128))
    }

    fn fixture_with_balance(balance: U256) -> Arc<LocalChainClient> {
        let provider: MockEthProvider = MockEthProvider::new().with_genesis_block();
        let header = Header {
            number: 1,
            gas_limit: 30_000_000,
            ..Default::default()
        };
        provider.add_header(header.hash_slow(), header);
        provider.add_account(signer().address(), ExtendedAccount::new(0, balance));
        provider.add_account(
            REVERTER,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("63deadbeef6000526004601cfd")),
        );
        let evm_config =
            EezEvmConfig::for_l1_simulation(Arc::new(ChainSpecBuilder::mainnet().build()));
        LocalChainClient::new_entry(
            provider,
            evm_config,
            RollupId::MAINNET,
            Address::ZERO,
            eez_protocol::ChainDialect::EvmL1Style,
        )
    }

    fn source_status(client: &LocalChainClient, raw_tx: Vec<u8>) -> bool {
        let slot = L1SlotState::open(client).expect("open source slot");
        let (mut state, env) = slot.fork_state(client).expect("fork source state");
        let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
        client
            .simulate_source_tx_on(raw_tx, &mut dispatcher, &mut state, env)
            .expect("simulate source transaction")
    }

    #[test]
    fn source_simulation_reports_receipt_status() {
        let client = fixture();
        assert!(source_status(&client, signed_tx(RECIPIENT)));
        assert!(!source_status(&client, signed_tx(REVERTER)));
    }

    #[test]
    fn source_planning_gas_reports_execution_instead_of_the_signed_limit() {
        let client = fixture();
        let slot = L1SlotState::open(&client).unwrap();
        let (mut state, env) = slot.fork_state(&client).unwrap();
        let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
        let result = client
            .simulate_source_tx_on_with_retries(
                signed_tx(RECIPIENT),
                &mut dispatcher,
                &mut state,
                env,
                true,
            )
            .unwrap();
        assert!(result.success);
        assert_eq!(result.gas_used, 21_000);
    }

    #[test]
    fn selected_admission_source_trace_keeps_the_same_gas_result() {
        let directory = std::env::temp_dir().join(format!(
            "eez-admission-source-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let raw = signed_tx(REVERTER);
        let config = gas_trace::Config::parse(
            Some(alloy_primitives::keccak256(&raw).to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        let run = |record: bool| {
            let client = fixture();
            let slot = L1SlotState::open(&client).unwrap();
            let (mut state, env) = slot.fork_state(&client).unwrap();
            let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
            let _scope = record.then(|| {
                config
                    .begin(
                        "admission_execution",
                        B256::ZERO,
                        alloy_primitives::keccak256(&raw),
                    )
                    .unwrap()
            });
            client
                .simulate_source_tx_on_with_retries(
                    raw.clone(),
                    &mut dispatcher,
                    &mut state,
                    env,
                    false,
                )
                .unwrap()
                .gas_used
        };
        let ordinary = run(false);
        assert!(!directory.exists());
        assert_eq!(run(true), ordinary);
        let file = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let lines = std::fs::read_to_string(file).unwrap();
        assert!(lines.lines().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["kind"] == "step" && value["chain_id"] == 1
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn address_trace_selects_an_internal_call_without_changing_gas() {
        let directory = std::env::temp_dir().join(format!(
            "eez-internal-address-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = gas_trace::Config::parse_with_address(
            None,
            Some(NESTED_TARGET.to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        let raw = signed_tx(NESTED_CALLER);
        let hash = alloy_primitives::keccak256(&raw);
        let run = |record: bool| {
            let provider: MockEthProvider = MockEthProvider::new().with_genesis_block();
            let header = Header {
                number: 1,
                gas_limit: 30_000_000,
                ..Default::default()
            };
            provider.add_header(header.hash_slow(), header);
            provider.add_account(
                signer().address(),
                ExtendedAccount::new(0, U256::from(10_u64.pow(12))),
            );
            let mut caller_code = vec![0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x73];
            caller_code.extend_from_slice(NESTED_TARGET.as_slice());
            caller_code.extend_from_slice(&[0x61, 0xff, 0xff, 0xf1, 0x00]);
            provider.add_account(
                NESTED_CALLER,
                ExtendedAccount::new(0, U256::ZERO).with_bytecode(Bytes::from(caller_code)),
            );
            provider.add_account(
                NESTED_TARGET,
                ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("00")),
            );
            let client = LocalChainClient::new_entry(
                provider,
                EezEvmConfig::for_l1_simulation(Arc::new(ChainSpecBuilder::mainnet().build())),
                RollupId::MAINNET,
                Address::ZERO,
                eez_protocol::ChainDialect::EvmL1Style,
            );
            let slot = L1SlotState::open(&client).unwrap();
            let (mut state, env) = slot.fork_state(&client).unwrap();
            let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
            let _scope = record.then(|| {
                config
                    .begin_admission(B256::ZERO, hash, &raw)
                    .unwrap()
                    .unwrap()
            });
            client
                .simulate_source_tx_on_with_retries(
                    raw.clone(),
                    &mut dispatcher,
                    &mut state,
                    env,
                    false,
                )
                .unwrap()
                .gas_used
        };
        let ordinary = run(false);
        assert!(!directory.exists());
        assert_eq!(run(true), ordinary);
        assert!(config.selects(hash));
        let file = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let lines = std::fs::read_to_string(file).unwrap();
        assert!(lines.lines().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["kind"] == "address_match"
                && value["address"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap()
                    == NESTED_TARGET
                && value["reason"] == "call_code_address"
        }));
        assert!(lines.lines().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["kind"] == "step"
                && value["address"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap()
                    == NESTED_TARGET
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn address_trace_selects_signed_sender_and_recipient() {
        let raw = signed_tx(RECIPIENT);
        let hash = alloy_primitives::keccak256(&raw);
        for (address, reason) in [
            (signer().address(), "source_sender"),
            (RECIPIENT, "source_recipient"),
        ] {
            let directory = std::env::temp_dir().join(format!(
                "eez-source-address-trace-{reason}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let config = gas_trace::Config::parse_with_address(
                None,
                Some(address.to_string()),
                Some(directory.to_str().unwrap().into()),
            )
            .unwrap()
            .unwrap();
            {
                let _scope = config
                    .begin_admission(B256::ZERO, hash, &raw)
                    .unwrap()
                    .unwrap();
            }
            assert!(config.selects(hash));
            let file = std::fs::read_dir(&directory)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let lines = std::fs::read_to_string(file).unwrap();
            assert!(lines.lines().any(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                value["kind"] == "address_match" && value["reason"] == reason
            }));
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn enabled_future_nonce_waits_for_its_predecessor_without_changing_legacy() {
        let client = fixture();
        let slot = L1SlotState::open(&client).unwrap();
        let (mut state, env) = slot.fork_state(&client).unwrap();
        let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
        let future = signed_tx_with_nonce(RECIPIENT, 1);
        assert!(
            !client
                .simulate_source_tx_on_with_retries(
                    future.clone(),
                    &mut dispatcher,
                    &mut state,
                    env.clone(),
                    false,
                )
                .unwrap()
                .success
        );
        let error = client
            .simulate_source_tx_on_with_retries(
                future.clone(),
                &mut dispatcher,
                &mut state,
                env.clone(),
                true,
            )
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            ExecutorErrorKind::SourceBlocked(eez_protocol::SourceCondition::Nonce {
                transaction_nonce: 1,
                required_nonce: 0
            })
        ));
        assert!(
            client
                .simulate_source_tx_on_with_retries(
                    signed_tx(RECIPIENT),
                    &mut dispatcher,
                    &mut state,
                    env.clone(),
                    true,
                )
                .unwrap()
                .success
        );
        assert!(
            client
                .simulate_source_tx_on_with_retries(
                    future,
                    &mut dispatcher,
                    &mut state,
                    env.clone(),
                    true,
                )
                .unwrap()
                .success
        );
        // A spent nonce cannot be repaired by waiting for a predecessor.
        assert!(
            !client
                .simulate_source_tx_on_with_retries(
                    signed_tx(RECIPIENT),
                    &mut dispatcher,
                    &mut state,
                    env,
                    true,
                )
                .unwrap()
                .success
        );
    }

    #[test]
    fn enabled_funding_and_fee_failures_are_retryable_without_changing_legacy_or_reverts() {
        let simulate = |balance, basefee, retry, to| {
            let client = fixture_with_balance(balance);
            let slot = L1SlotState::open(&client).unwrap();
            let (mut state, mut env) = slot.fork_state(&client).unwrap();
            env.cfg_env = revm::context::CfgEnv::new()
                .with_spec_and_mainnet_gas_params(revm::primitives::hardfork::SpecId::LONDON);
            env.block_env.basefee = basefee;
            let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
            client.simulate_source_tx_on_with_retries(
                signed_tx(to),
                &mut dispatcher,
                &mut state,
                env,
                retry,
            )
        };
        let funded = U256::from(1_000_000_000u64);
        for (balance, fee) in [(funded, 2), (U256::ZERO, 1)] {
            assert!(!simulate(balance, fee, false, RECIPIENT).unwrap().success);
            let error = simulate(balance, fee, true, RECIPIENT).unwrap_err();
            assert!(matches!(error.kind(), ExecutorErrorKind::SourceBlocked(_)));
        }
        assert!(simulate(funded, 1, true, RECIPIENT).unwrap().success);
        assert!(!simulate(funded, 1, true, REVERTER).unwrap().success);
    }
}
