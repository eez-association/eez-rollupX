//! Opt-in gas diagnostics selected by signed source hash or an address touched
//! during composition. No opcode inspector is attached when the selector is
//! absent. Traces omit calldata, memory, and full stacks; storage keys and
//! gas/refund transitions are enough to inspect queue reuse.

use std::{
    cell::RefCell,
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    marker::PhantomData,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use alloy_consensus::{Transaction as _, TxEnvelope, transaction::SignerRecoverable as _};
use alloy_eips::eip2718::Decodable2718 as _;
use alloy_primitives::{Address, B256};
use evm2::{
    EvmTypesHost, Inspector,
    interpreter::{Interpreter, Message, MessageResult},
};
use serde_json::{Value, json};

static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static ACTIVE: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    selector: Selector,
    directory: PathBuf,
}

#[derive(Debug, Clone)]
enum Selector {
    Hashes(Vec<B256>),
    Address {
        address: Address,
        selected: Arc<Mutex<HashSet<B256>>>,
    },
}

impl Config {
    // Address mode writes a provisional admission trace before it knows whether
    // a nested source/target frame reaches the watched contract. Limit the
    // distinct retained sources; recompositions of selected sources remain visible.
    const MAX_ADDRESS_SOURCES: usize = 8;

    #[cfg(test)]
    pub(crate) fn parse(
        hash: Option<String>,
        directory: Option<String>,
    ) -> Result<Option<Self>, String> {
        Self::parse_with_address(hash, None, directory)
    }

    pub(crate) fn parse_with_address(
        hash: Option<String>,
        address: Option<String>,
        directory: Option<String>,
    ) -> Result<Option<Self>, String> {
        let hash = hash.as_deref().filter(|value| !value.is_empty());
        let address = address.as_deref().filter(|value| !value.is_empty());
        let directory = directory.as_deref().filter(|value| !value.is_empty());
        if hash.is_some() && address.is_some() {
            return Err(
                "EEZ_COMPOSER_GAS_TRACE_TX_HASH and EEZ_COMPOSER_GAS_TRACE_ADDRESS are mutually exclusive".into(),
            );
        }
        match (hash, address, directory) {
            (None, None, None) => Ok(None),
            (None, None, Some(_)) | (_, _, None) => Err(
                "exactly one of EEZ_COMPOSER_GAS_TRACE_TX_HASH or EEZ_COMPOSER_GAS_TRACE_ADDRESS requires EEZ_COMPOSER_GAS_TRACE_DIR".into(),
            ),
            (hash, address, Some(directory)) => {
                let selector = if let Some(hash) = hash {
                    let hashes = hash.split(',').collect::<Vec<_>>();
                    if hashes.len() > 2 {
                        return Err(
                            "EEZ_COMPOSER_GAS_TRACE_TX_HASH accepts at most two hashes".into(),
                        );
                    }
                    let selected_txs = hashes
                        .into_iter()
                        .map(|hash| {
                            if !hash.starts_with("0x") || hash.len() != 66 {
                                return Err("EEZ_COMPOSER_GAS_TRACE_TX_HASH must contain 0x-prefixed 32-byte hashes".to_owned());
                            }
                            hash.parse::<B256>().map_err(|_| {
                                "EEZ_COMPOSER_GAS_TRACE_TX_HASH must contain 0x-prefixed 32-byte hashes".to_owned()
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if selected_txs.len() == 2 && selected_txs[0] == selected_txs[1] {
                        return Err(
                            "EEZ_COMPOSER_GAS_TRACE_TX_HASH contains a duplicate hash".into(),
                        );
                    }
                    Selector::Hashes(selected_txs)
                } else {
                    let address = address.expect("selector presence checked by match arms");
                    if !address.starts_with("0x") || address.len() != 42 {
                        return Err("EEZ_COMPOSER_GAS_TRACE_ADDRESS must be a 0x-prefixed 20-byte address".into());
                    }
                    let address = address.parse::<Address>().map_err(|_| {
                        "EEZ_COMPOSER_GAS_TRACE_ADDRESS must be a 0x-prefixed 20-byte address".to_owned()
                    })?;
                    Selector::Address {
                        address,
                        selected: Arc::new(Mutex::new(HashSet::new())),
                    }
                };
                let directory = PathBuf::from(directory);
                if !directory.is_absolute() {
                    return Err("EEZ_COMPOSER_GAS_TRACE_DIR must be absolute".into());
                }
                Ok(Some(Self { selector, directory }))
            }
        }
    }

    pub(crate) fn selects(&self, hash: B256) -> bool {
        match &self.selector {
            Selector::Hashes(hashes) => hashes.contains(&hash),
            Selector::Address { selected, .. } => selected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&hash),
        }
    }

    /// Opens a complete source trace before internal target addresses are
    /// known. Address-mode files are removed on scope exit unless this source
    /// touches the configured address; hash mode keeps its exact selection.
    pub(crate) fn begin_admission(
        &self,
        parent: B256,
        source_hash: B256,
        raw_tx: &[u8],
    ) -> std::io::Result<Option<Scope>> {
        let retention = match &self.selector {
            Selector::Hashes(hashes) if hashes.contains(&source_hash) => Retention::Always,
            Selector::Hashes(_) => return Ok(None),
            Selector::Address { address, selected } => {
                let selected_guard = selected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if selected_guard.len() >= Self::MAX_ADDRESS_SOURCES
                    && !selected_guard.contains(&source_hash)
                {
                    return Ok(None);
                }
                Retention::Address {
                    address: *address,
                    selected: Arc::clone(selected),
                    source_hash,
                    already_selected: selected_guard.contains(&source_hash),
                    matched: false,
                }
            }
        };
        let scope = self.open("admission_execution", parent, source_hash, retention)?;
        if matches!(&self.selector, Selector::Address { .. }) {
            let mut bytes = raw_tx;
            if let Ok(tx) = TxEnvelope::decode_2718(&mut bytes) {
                if let Ok(sender) = tx.recover_signer() {
                    note_address(sender, "source_sender");
                }
                if let Some(to) = tx.to() {
                    note_address(to, "source_recipient");
                }
            }
        }
        Ok(Some(scope))
    }

    pub(crate) fn begin(
        &self,
        phase: &'static str,
        parent: B256,
        selected_source_tx: B256,
    ) -> std::io::Result<Scope> {
        if !self.selects(selected_source_tx) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "gas trace source is not selected",
            ));
        }
        self.open(phase, parent, selected_source_tx, Retention::Always)
    }

    fn open(
        &self,
        phase: &'static str,
        parent: B256,
        selected_source_tx: B256,
        retention: Retention,
    ) -> std::io::Result<Scope> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)?;
        let sequence = FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let started_unix_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let filename = format!(
            "{}-{phase}-{}-{started_unix_nanos}-{sequence}.jsonl",
            selected_source_tx,
            std::process::id()
        );
        let path = self.directory.join(filename);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let sink = Sink {
            path,
            writer: BufWriter::new(file),
            error: None,
            steps: 0,
            truncated: false,
            tx_index: 0,
            retention,
        };
        let previous = ACTIVE.with(|active| active.replace(Some(sink)));
        emit(json!({
            "kind": "header", "version": 1, "phase": phase,
            "selected_source_tx": selected_source_tx,
            "configured_source_txs": match &self.selector {
                Selector::Hashes(hashes) => hashes.clone(),
                Selector::Address { .. } => Vec::new(),
            },
            "selected_address": match &self.selector {
                Selector::Hashes(_) => None,
                Selector::Address { address, .. } => Some(*address),
            },
            "parent": parent,
            "started_unix_nanos": started_unix_nanos.to_string(),
        }));
        Ok(Scope {
            previous,
            _not_send: PhantomData,
        })
    }
}

struct Sink {
    path: PathBuf,
    writer: BufWriter<File>,
    error: Option<String>,
    steps: u64,
    truncated: bool,
    tx_index: usize,
    retention: Retention,
}

enum Retention {
    Always,
    Address {
        address: Address,
        selected: Arc<Mutex<HashSet<B256>>>,
        source_hash: B256,
        already_selected: bool,
        matched: bool,
    },
}

impl Sink {
    // A selected trace is diagnostic and must not exhaust the node's disk.
    const MAX_STEPS: u64 = 1_000_000;

    fn note_address(&mut self, candidate: Address, reason: &'static str) {
        if let Retention::Address {
            address, matched, ..
        } = &mut self.retention
            && *address == candidate
            && !*matched
        {
            *matched = true;
            self.write(&json!({
                "kind": "address_match", "address": candidate, "reason": reason,
            }));
        }
    }

    fn write(&mut self, value: &Value) {
        if self.error.is_some() {
            return;
        }
        let result = serde_json::to_writer(&mut self.writer, value)
            .map_err(|error| error.to_string())
            .and_then(|_| {
                self.writer
                    .write_all(b"\n")
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    fn write_step(&mut self, value: &Value) {
        if self.steps == Self::MAX_STEPS {
            self.truncated = true;
            self.steps += 1;
            self.write(&json!({"kind": "truncated", "max_steps": Self::MAX_STEPS}));
            return;
        }
        if self.steps > Self::MAX_STEPS {
            return;
        }
        self.steps += 1;
        self.write(value);
    }
}

/// The scope stays on the same thread as the synchronous EVM execution.
/// Nested target sessions reuse its writer; leaving the scope restores any
/// earlier diagnostic without changing the simulated state or result.
pub(crate) struct Scope {
    previous: Option<Sink>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        let Some(mut sink) = ACTIVE.with(|active| active.replace(self.previous.take())) else {
            return;
        };
        sink.write(&json!({"kind": "end", "steps": sink.steps, "truncated": sink.truncated}));
        if let Err(error) = sink.writer.flush() {
            sink.error.get_or_insert_with(|| error.to_string());
        }
        let keep = match &sink.retention {
            Retention::Always => true,
            Retention::Address {
                selected,
                source_hash,
                already_selected,
                matched,
                ..
            } => {
                if *already_selected {
                    true
                } else if *matched {
                    let mut hashes = selected
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if hashes.len() >= Config::MAX_ADDRESS_SOURCES {
                        false
                    } else {
                        hashes.insert(*source_hash);
                        if hashes.len() == Config::MAX_ADDRESS_SOURCES {
                            tracing::warn!(
                                limit = Config::MAX_ADDRESS_SOURCES,
                                "address gas trace source limit reached; later sources will not be captured"
                            );
                        }
                        true
                    }
                } else {
                    false
                }
            }
        };
        let path = sink.path.clone();
        let steps = sink.steps;
        let error = sink.error.take();
        drop(sink);
        if keep {
            if let Some(error) = error {
                tracing::warn!(path = %path.display(), %error, "gas trace is incomplete");
            } else {
                tracing::info!(path = %path.display(), steps, "gas trace written");
            }
        } else if let Err(error) = std::fs::remove_file(&path) {
            tracing::warn!(path = %path.display(), %error, "cannot discard unmatched gas trace");
        }
    }
}

pub(super) fn active() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

pub(super) fn set_tx_index(index: usize) {
    ACTIVE.with(|active| {
        if let Some(sink) = active.borrow_mut().as_mut() {
            sink.tx_index = index;
        }
    });
}

pub(super) fn emit(value: Value) {
    ACTIVE.with(|active| {
        if let Some(sink) = active.borrow_mut().as_mut() {
            sink.write(&value);
        }
    });
}

pub(super) fn note_address(address: Address, reason: &'static str) {
    ACTIVE.with(|active| {
        if let Some(sink) = active.borrow_mut().as_mut() {
            sink.note_address(address, reason);
        }
    });
}

/// Selects the original inspector without a step hook on normal executions.
pub(crate) fn maybe_trace<I>(inspector: &mut I, chain_id: u64) -> (&mut I, Option<StepTracer>) {
    (inspector, active().then(|| StepTracer::new(chain_id)))
}

pub(crate) struct StepTracer {
    chain_id: u64,
    // Native step_end follows nested execution; child steps must not overwrite CALL's row.
    pending: Vec<PendingStep>,
}
struct PendingStep {
    pc: usize,
    opcode: u8,
    gas_before: u64,
    refund_before: i64,
    storage_key: Option<String>,
    emitted: bool,
}
impl StepTracer {
    fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            pending: Vec::new(),
        }
    }
}

impl<T: EvmTypesHost> Inspector<T> for StepTracer {
    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        // CALL into an empty account or precompile has no opcode rows. For
        // DELEGATECALL the code and storage addresses also differ.
        note_address(inputs.code_address, "call_code_address");
        note_address(inputs.destination, "call_target_address");
        self.emit_suspended_step(interp);
        None
    }

    fn create(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        _input: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        self.emit_suspended_step(interp);
        None
    }

    fn step(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        let opcode = interp.opcode();
        self.pending.push(PendingStep {
            pc: interp.pc(),
            opcode,
            gas_before: interp.gas().remaining(),
            refund_before: interp.gas().refunded(),
            emitted: false,
            storage_key: matches!(opcode, 0x54 | 0x55)
                .then(|| interp.stack().peek(0).map(|key| format!("{key:#x}")))
                .flatten(),
        });
    }

    fn step_end(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        let Some(step) = self.pending.pop() else {
            return;
        };
        if !step.emitted {
            Self::emit_step(self.chain_id, interp, &step);
        }
    }
}

impl StepTracer {
    fn emit_suspended_step<T: EvmTypesHost>(&mut self, interp: &Interpreter<'_, '_, T>) {
        let Some(step) = self.pending.last_mut() else {
            return;
        };
        if !step.emitted && matches!(step.opcode, 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa) {
            // Preserve REVM's row before child entry, including forwarded gas;
            // evm2's ordinary step_end runs only after that child returns.
            step.emitted = true;
            Self::emit_step(self.chain_id, interp, step);
        }
    }
    fn emit_step<T: EvmTypesHost>(
        chain_id: u64,
        interp: &Interpreter<'_, '_, T>,
        step: &PendingStep,
    ) {
        let value = json!({
            "kind": "step", "chain_id": chain_id,
            "address": interp.message().destination, "depth": interp.message().depth.saturating_add(1),
            "pc": step.pc, "op": step.opcode, "gas_before": step.gas_before,
            "gas_cost": step.gas_before.saturating_sub(interp.gas().remaining()), "gas_after": interp.gas().remaining(),
            "refund_before": step.refund_before, "refund_after": interp.gas().refunded(),
            "storage_key": step.storage_key,
        });
        ACTIVE.with(|active| {
            if let Some(sink) = active.borrow_mut().as_mut() {
                sink.note_address(interp.message().destination, "executed_frame");
                let mut value = value;
                value["tx_index"] = json!(sink.tx_index);
                sink.write_step(&value);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_requires_both_strict_inputs() {
        assert!(Config::parse(None, None).unwrap().is_none());
        assert!(
            Config::parse(Some(String::new()), Some(String::new()))
                .unwrap()
                .is_none()
        );
        assert!(Config::parse(Some(B256::ZERO.to_string()), None).is_err());
        assert!(Config::parse(None, Some("/tmp/traces".into())).is_err());
        assert!(Config::parse(Some("bad".into()), Some("/tmp/traces".into())).is_err());
        assert!(Config::parse(Some(B256::ZERO.to_string()), Some("relative".into())).is_err());
        assert!(Config::parse(Some(B256::ZERO.to_string()), Some("/tmp/traces".into())).is_ok());
        let second = B256::with_last_byte(1);
        let both = Config::parse(
            Some(format!("{},{}", B256::ZERO, second)),
            Some("/tmp/traces".into()),
        )
        .unwrap()
        .unwrap();
        assert!(both.selects(B256::ZERO));
        assert!(both.selects(second));
        assert!(
            Config::parse(
                Some(format!("{0},{0}", B256::ZERO)),
                Some("/tmp/traces".into())
            )
            .is_err()
        );
        assert!(
            Config::parse(
                Some(format!("{0},{0},{0}", B256::ZERO)),
                Some("/tmp/traces".into())
            )
            .is_err()
        );
        assert!(
            Config::parse(
                Some(format!("{},bad", B256::ZERO)),
                Some("/tmp/traces".into())
            )
            .is_err()
        );
        let address = Address::repeat_byte(0x42).to_string();
        assert!(
            Config::parse_with_address(None, Some(address.clone()), Some("/tmp/traces".into()))
                .is_ok()
        );
        for (hash, address, directory) in [
            (None, Some(address.clone()), None),
            (None, None, Some("/tmp/traces".into())),
            (None, Some("bad".into()), Some("/tmp/traces".into())),
            (None, Some(address.clone()), Some("relative".into())),
            (
                Some(B256::ZERO.to_string()),
                Some(address),
                Some("/tmp/traces".into()),
            ),
        ] {
            assert!(Config::parse_with_address(hash, address, directory).is_err());
        }
    }

    #[test]
    fn selected_scope_writes_private_structured_trace_only_when_opened() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "eez-gas-trace-test-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let config = Config::parse(
            Some(B256::ZERO.to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        assert!(!directory.exists());
        assert!(config.selects(B256::ZERO));
        assert!(!config.selects(B256::with_last_byte(1)));
        {
            let _scope = config
                .begin("admission_budget", B256::ZERO, B256::ZERO)
                .unwrap();
            emit(json!({"kind": "admission_budget", "projected_before": 12}));
        }
        let paths = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let lines = std::fs::read_to_string(&paths[0]).unwrap();
        let values = lines
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0]["kind"], "header");
        assert_eq!(values[1]["projected_before"], 12);
        assert_eq!(values[2]["kind"], "end");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn second_selected_source_has_its_own_admission_identity() {
        let directory = std::env::temp_dir().join(format!(
            "eez-gas-trace-pair-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let second = B256::with_last_byte(1);
        let config = Config::parse(
            Some(format!("{},{}", B256::ZERO, second)),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        assert!(
            config
                .begin("admission_execution", B256::ZERO, B256::with_last_byte(2))
                .is_err()
        );
        {
            let _scope = config
                .begin("admission_execution", B256::ZERO, second)
                .unwrap();
        }
        let file = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(
            file.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&second.to_string())
        );
        let header: Value = serde_json::from_str(
            std::fs::read_to_string(&file)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header["selected_source_tx"], second.to_string());
        assert_eq!(header["configured_source_txs"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn address_selection_discards_unmatched_and_shares_matching_source_with_preflight() {
        let directory = std::env::temp_dir().join(format!(
            "eez-gas-trace-address-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let watched = Address::repeat_byte(0x42);
        let config = Config::parse_with_address(
            None,
            Some(watched.to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        let final_config = config.clone();
        let unmatched = B256::with_last_byte(1);
        {
            let _scope = config
                .begin_admission(B256::ZERO, unmatched, &[])
                .unwrap()
                .unwrap();
            note_address(Address::repeat_byte(0x43), "executed_frame");
        }
        assert!(!config.selects(unmatched));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);

        let matched = B256::with_last_byte(2);
        {
            let _scope = config
                .begin_admission(B256::ZERO, matched, &[])
                .unwrap()
                .unwrap();
            note_address(watched, "cross_chain_target");
        }
        assert!(final_config.selects(matched));
        assert!(!final_config.selects(unmatched));
        {
            let _scope = final_config
                .begin("preflight_final", B256::ZERO, matched)
                .unwrap();
        }
        let files = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 2);
        let admission = files
            .iter()
            .find(|path| path.to_string_lossy().contains("admission_execution"))
            .unwrap();
        assert!(
            std::fs::read_to_string(admission)
                .unwrap()
                .contains("cross_chain_target")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn address_selection_bounds_distinct_sources_but_keeps_recomposition() {
        let directory = std::env::temp_dir().join(format!(
            "eez-gas-trace-address-limit-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let watched = Address::repeat_byte(0x42);
        let config = Config::parse_with_address(
            None,
            Some(watched.to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        for byte in 1..=Config::MAX_ADDRESS_SOURCES {
            let hash = B256::with_last_byte(u8::try_from(byte).unwrap());
            let _scope = config
                .begin_admission(B256::ZERO, hash, &[])
                .unwrap()
                .unwrap();
            note_address(watched, "executed_frame");
        }
        let next = B256::with_last_byte(99);
        assert!(
            config
                .begin_admission(B256::ZERO, next, &[])
                .unwrap()
                .is_none()
        );
        assert!(!config.selects(next));
        assert!(
            config
                .begin_admission(B256::ZERO, B256::with_last_byte(1), &[])
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn native_nested_trace_keeps_parent_rows_and_does_not_change_execution() {
        use alloy_consensus::{TxLegacy, transaction::Recovered};
        use alloy_primitives::{Bytes, TxKind, U256};
        use evm2::{
            BaseEvmTypes, Evm, SpecId,
            bytecode::Bytecode,
            evm::{AccountInfo, InMemoryDB, inspector::NoopInspector},
        };
        for opcode in [0xf1, 0xf0, 0xf5] {
            let source = Address::repeat_byte(0x11);
            let init = alloy_primitives::hex::decode("600160005560006000f3").unwrap();
            let child = match opcode {
                0xf0 => source.create(1),
                0xf5 => source.create2_from_code(B256::ZERO, &init),
                _ => Address::repeat_byte(0x22),
            };
            let caller = Address::repeat_byte(0x33);
            let mut db = InMemoryDB::default();
            let mut code = alloy_primitives::hex::decode("5f5f5f5f5f73").unwrap();
            code.extend_from_slice(child.as_slice());
            code.extend_from_slice(&alloy_primitives::hex::decode("5af15000").unwrap());
            if opcode != 0xf1 {
                code = vec![0x69];
                code.extend_from_slice(&init);
                code.extend_from_slice(&[0x5f, 0x52]);
                if opcode == 0xf5 {
                    code.push(0x5f);
                }
                code.extend_from_slice(&[0x60, 10, 0x60, 22, 0x5f, opcode, 0x50, 0]);
            }
            db.insert_account_info(
                &source,
                AccountInfo::empty()
                    .with_nonce(1)
                    .with_code(Bytecode::new_raw(code.into())),
            );
            if opcode == 0xf1 {
                db.insert_account_info(
                    &child,
                    AccountInfo::empty()
                        .with_nonce(1)
                        .with_code(Bytecode::new_raw(Bytes::from_static(&[
                            0x60, 1, 0x60, 0, 0x55, 0,
                        ]))),
                );
            }
            db.insert_account_info(
                &caller,
                AccountInfo::empty().with_balance(U256::from(1_000_000)),
            );
            let create = |db| {
                Evm::<BaseEvmTypes>::new(
                    SpecId::OSAKA,
                    Default::default(),
                    evm2::ethereum::ethereum_tx_registry(SpecId::OSAKA),
                    db,
                    evm2::precompiles::Precompiles::base(SpecId::OSAKA),
                )
            };
            let tx = Recovered::new_unchecked(
                evm2::ethereum::TxEnvelope::Legacy(TxLegacy {
                    to: TxKind::Call(source),
                    gas_limit: 500_000,
                    ..Default::default()
                }),
                caller,
            );
            let expected = create(db.clone()).transact(&tx).unwrap().detach();
            let directory = std::env::temp_dir().join(format!(
                "eez-native-trace-{}-{}",
                std::process::id(),
                FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let config = Config::parse(
                Some(B256::ZERO.to_string()),
                Some(directory.to_string_lossy().into_owned()),
            )
            .unwrap()
            .unwrap();
            let scope = config.begin("test", B256::ZERO, B256::ZERO).unwrap();
            let quote = super::super::quote_trace::Scope::begin(json!({}), json!({}));
            let mut noop = NoopInspector::default();
            let traced = maybe_trace(&mut noop, 1);
            let mut traced = super::super::quote_trace::inspect(traced, 1, "composition", &tx);
            let actual = create(db)
                .transact_with_inspector(&tx, &mut traced)
                .unwrap()
                .detach();
            drop(traced);
            let quote = quote.finish();
            drop(scope);
            assert_eq!(actual.result, expected.result);
            assert_eq!(actual.pending_state, expected.pending_state);
            assert_eq!(quote["runs"][0]["complete"], true);
            assert_eq!(quote["runs"][0]["frames"].as_array().unwrap().len(), 2);
            let paths = std::fs::read_dir(&directory)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(paths.len(), 1);
            let rows = std::fs::read_to_string(paths[0].path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|row| row["kind"] == "step")
                .collect::<Vec<_>>();
            let calls = rows
                .iter()
                .filter(|row| row["op"] == opcode)
                .collect::<Vec<_>>();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["address"], json!(source));
            assert_eq!(calls[0]["depth"], 1);
            if opcode == 0xf1 {
                assert_eq!(calls[0]["pc"], 27);
                assert_eq!(calls[0]["gas_cost"], 471_542);
                assert_eq!(rows[7], *calls[0]);
            }
            let stores = rows
                .iter()
                .filter(|row| row["op"] == 0x55)
                .collect::<Vec<_>>();
            assert_eq!(stores.len(), 1);
            assert_eq!(stores[0]["address"], json!(child));
            assert_eq!(stores[0]["gas_cost"], 22_100);
            assert_eq!(stores[0]["depth"], 2);
            println!("nested-trace {}", serde_json::to_string(&rows).unwrap());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
