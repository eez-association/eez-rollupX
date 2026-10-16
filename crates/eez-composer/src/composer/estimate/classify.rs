//! Read-only native call observation, including frames an application catches.
use super::*;
use alloy_rpc_types_eth::state::StateOverride;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::EvmStateProviderAdapter;
use revm::{
    Inspector, Journal,
    context_interface::ContextTr,
    interpreter::{CallInputs, CallOutcome},
};

/// Dispatch recognition shared with signed-transaction ingress tracing.
pub fn is_dispatch(target: Address, input: &[u8], manager: Address) -> bool {
    target == manager
        && matches!(
            input.get(..4),
            Some([0x9a, 0xf5, 0x32, 0x59] | [0x31, 0x34, 0x4a, 0xde])
        )
}

struct Observer {
    manager: Address,
    found: bool,
}

impl<DB: Database, C: ContextTr<Db = DB, Journal = Journal<DB>>> Inspector<C> for Observer {
    fn call(&mut self, context: &mut C, inputs: &mut CallInputs) -> Option<CallOutcome> {
        self.found |= is_dispatch(
            inputs.target_address,
            &inputs.input.as_bytes(context),
            self.manager,
        );
        None
    }
}

impl GasEstimator {
    /// Inspect unsigned execution without needing a node's debug RPC namespace.
    /// `number` selects historical state; `None` selects the local current head.
    /// An unavailable context is an error, never an ordinary-call classification.
    pub fn classify(
        &self,
        direction: Direction,
        request: TransactionRequest,
        number: Option<u64>,
        overrides: Option<StateOverride>,
    ) -> Result<bool, EstimateError> {
        let client = if direction == Direction::Inbound {
            &self.l1
        } else {
            &self.l2
        };
        let chain = client.chain_provider();
        let number = match number {
            Some(n) => n,
            None => chain.headers.best_block_number().map_err(unavailable)?,
        };
        let header = chain
            .headers
            .header_by_number(number)
            .map_err(unavailable)?
            .ok_or_else(|| unavailable("classification header missing"))?;
        let parent = SealedHeader::seal_slow(header);
        let provider = chain
            .provider
            .state_by_block_hash(parent.hash())
            .map_err(unavailable)?;
        let mut state = State::builder()
            .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
                provider,
            )))
            .build();
        if let Some(overrides) = overrides {
            reth_evm::overrides::apply_state_overrides(overrides, &mut state)
                .map_err(unavailable)?;
        }
        let mut observer = Observer {
            manager: client.manager_address(),
            found: false,
        };
        macro_rules! run {
            ($config:expr, $header:expr) => {{
                let config = $config;
                let mut env = config.evm_env(&$header).map_err(unavailable)?;
                // Match eth_call validation: no signature, account-code or fee
                // affordability requirement. Estimate execution checks fees later.
                env.cfg_env.disable_nonce_check = true;
                env.cfg_env.disable_eip3607 = true;
                env.cfg_env.disable_base_fee = true;
                env.cfg_env.disable_balance_check = true;
                let mut tx: TxEnv = request
                    .try_into_tx_env(&env)
                    .map_err(|e| EstimateError::Invalid(e.to_string()))?;
                tx.gas_limit = tx.gas_limit.min(env.cfg_env.tx_gas_limit_cap());
                let mut evm = config.evm_with_env_and_inspector(&mut state, env, &mut observer);
                evm.transact(tx).map_err(unavailable)?;
            }};
        }
        if let Some(config) = &chain.gnosis_evm_config {
            run!(config, parent.header().clone().into());
        } else if direction == Direction::Inbound {
            run!(
                &reth_evm_ethereum::EthEvmConfig::new(chain.evm_config.chain_spec().clone()),
                parent
            );
        } else {
            run!(&chain.evm_config, parent);
        }
        Ok(observer.found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_evm::EezEvmConfig;
    use reth_chainspec::ChainSpecBuilder;
    use revm::{
        database::InMemoryDB,
        state::{AccountInfo, Bytecode},
    };

    #[test]
    fn native_observer_finds_dispatch_even_when_the_application_catches_revert() {
        let manager = Address::repeat_byte(1);
        let wrapper = Address::repeat_byte(2);
        let config = EezEvmConfig::for_l1_simulation(Arc::new(
            ChainSpecBuilder::mainnet().prague_activated().build(),
        ));
        let header = Header {
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(0),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ..Default::default()
        };
        for selector in [
            [0x9a, 0xf5, 0x32, 0x59],
            [0x31, 0x34, 0x4a, 0xde],
            [0, 0, 0, 0],
        ] {
            // Write the selector, STATICCALL manager, discard its failure and STOP.
            let mut code = vec![0x63];
            code.extend(selector);
            code.extend([
                0x60, 0xe0, 0x1b, 0x5f, 0x52, 0x5f, 0x5f, 0x60, 4, 0x5f, 0x73,
            ]);
            code.extend(manager.as_slice());
            code.extend([0x5a, 0xfa, 0x50, 0x00]);
            let mut db = InMemoryDB::default();
            db.insert_account_info(
                wrapper,
                AccountInfo {
                    code: Some(Bytecode::new_raw(code.into())),
                    ..Default::default()
                },
            );
            db.insert_account_info(
                manager,
                AccountInfo {
                    code: Some(Bytecode::new_raw(Bytes::from_static(&[0x5f, 0x5f, 0xfd]))),
                    ..Default::default()
                },
            );
            let env = config.evm_env(&header).unwrap();
            let mut observer = Observer {
                manager,
                found: false,
            };
            let result = config
                .evm_with_env_and_inspector(&mut db, env, &mut observer)
                .transact(TxEnv {
                    kind: alloy_primitives::TxKind::Call(wrapper),
                    gas_limit: 100_000,
                    ..Default::default()
                })
                .unwrap();
            assert!(
                result.result.is_success(),
                "application catches the protocol revert"
            );
            assert_eq!(observer.found, selector != [0, 0, 0, 0]);
        }
    }
}
