//! Native deposit minting around the unmodified Ethereum EVM.
//!
//! Delegates execution and inspector dispatch to Alloy 0.39.0's [`EthEvm`]:
//! <https://docs.rs/alloy-evm/0.39.0/src/alloy_evm/eth/mod.rs.html>.

use alloy_evm::{
    Database, EthEvm, EthEvmFactory, Evm, EvmEnv, EvmFactory, eth::EthEvmContext,
    precompiles::PrecompilesMap,
};
use alloy_primitives::{Address, Bytes, TxKind};
use eez_primitives::{EEZL2_ADDRESS, SYSTEM_ADDRESS, SYSTEM_TX_GAS_CAP, SYSTEM_TX_TYPE};
use revm::{
    Context, ExecuteEvm as _, Inspector, MainBuilder as _, MainContext as _,
    context::{BlockEnv, CfgEnv, TxEnv},
    context_interface::{
        JournalTr,
        journaled_state::account::JournaledAccountTr,
        result::{EVMError, HaltReason, ResultAndState},
    },
    handler::{Handler as _, MainnetHandler},
    inspector::NoOpInspector,
    primitives::hardfork::SpecId,
};

pub struct EezEvm<DB: Database, I> {
    ethereum: EthEvm<DB, I, PrecompilesMap>,
}

impl<DB: Database, I> std::fmt::Debug for EezEvm<DB, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EezEvm").finish_non_exhaustive()
    }
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> Evm for EezEvm<DB, I> {
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        self.ethereum.block()
    }

    fn cfg_env(&self) -> &CfgEnv {
        self.ethereum.cfg_env()
    }

    fn chain_id(&self) -> u64 {
        self.ethereum.chain_id()
    }

    /// Mint native value before Ethereum's validation and transfer, preserving
    /// existing system funds. Revm errors discard the mint; reverted or halted
    /// calls need the explicit rollback below. The native TxEnv supplies zero fees.
    /// `EthEvm::transact_raw` dispatches to `inspect_tx` when its inspector is enabled,
    /// so all paths below preserve the inspector installed by our factory.
    fn transact_raw(&mut self, tx: TxEnv) -> Result<ResultAndState, Self::Error> {
        if tx.tx_type != SYSTEM_TX_TYPE {
            if tx.caller == SYSTEM_ADDRESS {
                return Err(EVMError::Custom(
                    "reserved system sender requires the native transaction type".into(),
                ));
            }
            return self.ethereum.transact_raw(tx);
        }
        // Direct TxEnv callers bypass envelope conversion; enforce native sender,
        // target, and gas rules here before executing or minting value. The
        // budget is position-dependent (`system_tx_gas_limit`), so only its cap
        // is checkable here; the block executor supplies the exact value.
        if tx.caller != SYSTEM_ADDRESS
            || tx.kind != TxKind::Call(EEZL2_ADDRESS)
            || tx.gas_limit > SYSTEM_TX_GAS_CAP
            || tx.gas_price != 0
            || tx.chain_id.is_none()
            || tx.gas_priority_fee.is_some()
            || !tx.access_list.0.is_empty()
            || !tx.blob_hashes.is_empty()
            || tx.max_fee_per_blob_gas != 0
            || !tx.authorization_list.is_empty()
        {
            return Err(EVMError::Custom(
                "invalid native system transaction environment".into(),
            ));
        }
        if tx.value.is_zero() {
            return self.ethereum.transact_raw(tx);
        }

        let journal = &mut self.ethereum.ctx_mut().journaled_state;
        let funding = journal
            .load_account_with_code_mut(SYSTEM_ADDRESS)
            .map_err(EVMError::Database)
            .and_then(|mut account| {
                let balance = *account.balance();
                let funded = balance
                    .checked_add(tx.value)
                    .ok_or_else(|| EVMError::Custom("native deposit balance overflow".into()))?;
                account.set_balance(funded);
                Ok(balance)
            });
        let previous_balance = match funding {
            Ok(balance) => balance,
            Err(error) => {
                // These errors occur before Ethereum's transact/finalize path.
                // Discard the loaded state so the next transaction starts clean.
                journal.finalize();
                return Err(error);
            }
        };
        let mut output = self.ethereum.transact_raw(tx)?;
        if !output.result.is_success() {
            // The outer call already rolled back all transfers, but the mint
            // precedes that call's checkpoint. Remove only that mint; retain
            // the normal nonce increment and the receipt's metered gas usage.
            output
                .state
                .get_mut(&SYSTEM_ADDRESS)
                .expect("executed native transaction has a caller account")
                .info
                .balance = previous_balance;
        }
        Ok(output)
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        if contract == eez_protocol::l1_history::HISTORY_ADDRESS {
            if caller != SYSTEM_ADDRESS {
                return Err(EVMError::Custom("invalid L1 history system caller".into()));
            }
            // The upstream convenience method replaces the gas limit with its
            // 30M system budget. Run the same handler with the fixed profile
            // budget instead. The temporary journal shares only the state DB:
            // failure cannot commit a prefix of the import, and no transaction
            // validation, caller nonce increment or fee settlement runs.
            return bounded_history_call(
                self.ethereum.block().clone(),
                self.ethereum.cfg_env().clone(),
                self.ethereum.db_mut(),
                data,
                eez_protocol::l1_history::IMPORT_GAS_LIMIT,
            );
        }
        self.ethereum.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (DB, EvmEnv) {
        self.ethereum.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.ethereum.set_inspector_enabled(enabled);
    }

    fn components(&self) -> (&DB, &I, &PrecompilesMap) {
        self.ethereum.components()
    }

    fn components_mut(&mut self) -> (&mut DB, &mut I, &mut PrecompilesMap) {
        self.ethereum.components_mut()
    }
}

fn bounded_history_call<DB: Database>(
    block: BlockEnv,
    cfg: CfgEnv,
    db: &mut DB,
    data: Bytes,
    gas_limit: u64,
) -> Result<ResultAndState, EVMError<DB::Error>> {
    let mut evm = Context::mainnet()
        .with_block(block)
        .with_cfg(cfg)
        .with_db(db)
        .with_tx(TxEnv {
            caller: SYSTEM_ADDRESS,
            kind: TxKind::Call(eez_protocol::l1_history::HISTORY_ADDRESS),
            data,
            gas_limit,
            ..Default::default()
        })
        .build_mainnet();
    let result =
        MainnetHandler::<_, EVMError<DB::Error>, _>::default().run_system_call(&mut evm)?;
    let state = evm.finalize();
    Ok(ResultAndState { result, state })
}

/// Installs the minting wrapper for both ordinary and inspected execution.
#[derive(Debug, Default, Clone, Copy)]
pub struct EezEvmFactory;

impl EvmFactory for EezEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = EezEvm<DB, I>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<DBError: revm::database_interface::DBErrorMarker> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> EezEvm<DB, NoOpInspector> {
        EezEvm {
            ethereum: EthEvmFactory::default().create_evm(db, input),
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> EezEvm<DB, I> {
        EezEvm {
            ethereum: EthEvmFactory::default().create_evm_with_inspector(db, input, inspector),
        }
    }
}

#[cfg(test)]
mod history_budget_tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use alloy_sol_types::SolCall as _;
    use eez_protocol::l1_history::{HISTORY_ADDRESS, IL1BlockhashContract, IMPORT_GAS_LIMIT};
    use revm::Database as _;

    #[test]
    fn maximum_import_fits_and_a_small_budget_halts_without_charging_the_caller() {
        let (_, mut db) = crate::history::tests::fixture();
        db.insert_account_info(
            SYSTEM_ADDRESS,
            revm::state::AccountInfo {
                nonce: 91,
                balance: U256::from(7),
                ..Default::default()
            },
        );
        let data: Bytes = IL1BlockhashContract::importHistoryCall {
            cutoff: 4,
            hashes: (1..=64).map(B256::with_last_byte).collect(),
        }
        .abi_encode()
        .into();
        let cfg = CfgEnv::new_with_spec(SpecId::OSAKA);
        let block = BlockEnv {
            number: U256::from(3),
            timestamp: U256::from(7),
            prevrandao: Some(B256::ZERO),
            ..Default::default()
        };
        let result = bounded_history_call(
            block.clone(),
            cfg.clone(),
            &mut db,
            data.clone(),
            IMPORT_GAS_LIMIT,
        )
        .unwrap();
        assert!(result.result.is_success(), "{:?}", result.result);
        assert!(result.result.tx_gas_used() <= IMPORT_GAS_LIMIT);
        assert_eq!(
            result.state[&HISTORY_ADDRESS].storage[&U256::from(2)].present_value(),
            U256::from(64)
        );
        assert_eq!(db.basic(SYSTEM_ADDRESS).unwrap().unwrap().nonce, 91);
        assert_eq!(
            db.basic(SYSTEM_ADDRESS).unwrap().unwrap().balance,
            U256::from(7)
        );

        let insufficient = bounded_history_call(block, cfg, &mut db, data, 10_000).unwrap();
        assert!(insufficient.result.is_halt());
        assert_eq!(insufficient.result.tx_gas_used(), 10_000);
        assert_eq!(
            db.storage(HISTORY_ADDRESS, U256::from(2)).unwrap(),
            U256::ZERO
        );
        assert_eq!(db.basic(SYSTEM_ADDRESS).unwrap().unwrap().nonce, 91);
    }

    #[test]
    fn history_system_entry_rejects_any_other_caller() {
        let (_, db) = crate::history::tests::fixture();
        let mut evm = EezEvmFactory.create_evm(db, EvmEnv::default());
        let error = evm
            .transact_system_call(Address::ZERO, HISTORY_ADDRESS, Bytes::new())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid L1 history system caller")
        );
    }
}
