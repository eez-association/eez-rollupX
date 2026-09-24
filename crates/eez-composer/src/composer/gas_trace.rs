//! Opt-in, transaction-selected gas diagnostics. No opcode inspector is attached
//! when the selector is absent. Traces omit calldata, memory, and full stacks;
//! storage keys and gas/refund transitions are enough to inspect queue reuse.

use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    marker::PhantomData,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy_primitives::B256;
use either::Either;
use revm::{
    Inspector,
    context_interface::{ContextTr, JournalTr},
    inspector::inspectors::GasInspector,
    interpreter::{Interpreter, interpreter_types::Jumps},
};
use serde_json::{Value, json};

static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static ACTIVE: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    selected_txs: Vec<B256>,
    directory: PathBuf,
}

impl Config {
    pub(crate) fn parse(
        hash: Option<String>,
        directory: Option<String>,
    ) -> Result<Option<Self>, String> {
        let hash = hash.as_deref().filter(|value| !value.is_empty());
        let directory = directory.as_deref().filter(|value| !value.is_empty());
        match (hash, directory) {
            (None, None) => Ok(None),
            (Some(_), None) | (None, Some(_)) => Err(
                "EEZ_COMPOSER_GAS_TRACE_TX_HASH and EEZ_COMPOSER_GAS_TRACE_DIR must be set together".into(),
            ),
            (Some(hash), Some(directory)) => {
                let hashes = hash.split(',').collect::<Vec<_>>();
                if hashes.len() > 2 {
                    return Err("EEZ_COMPOSER_GAS_TRACE_TX_HASH accepts at most two hashes".into());
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
                    return Err("EEZ_COMPOSER_GAS_TRACE_TX_HASH contains a duplicate hash".into());
                }
                let directory = PathBuf::from(directory);
                if !directory.is_absolute() {
                    return Err("EEZ_COMPOSER_GAS_TRACE_DIR must be absolute".into());
                }
                Ok(Some(Self { selected_txs, directory }))
            }
        }
    }

    pub(super) fn selects(&self, hash: B256) -> bool {
        self.selected_txs.contains(&hash)
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
        };
        let previous = ACTIVE.with(|active| active.replace(Some(sink)));
        emit(json!({
            "kind": "header", "version": 1, "phase": phase,
            "selected_source_tx": selected_source_tx,
            "configured_source_txs": self.selected_txs, "parent": parent,
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
}

impl Sink {
    // A selected trace is diagnostic and must not exhaust the node's disk.
    const MAX_STEPS: u64 = 1_000_000;

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
        let mut sink = ACTIVE.with(|active| active.replace(self.previous.take()));
        if let Some(sink) = sink.as_mut() {
            sink.write(&json!({"kind": "end", "steps": sink.steps, "truncated": sink.truncated}));
            if let Err(error) = sink.writer.flush() {
                sink.error.get_or_insert_with(|| error.to_string());
            }
            if let Some(error) = &sink.error {
                tracing::warn!(path = %sink.path.display(), %error, "gas trace is incomplete");
            } else {
                tracing::info!(path = %sink.path.display(), steps = sink.steps, "gas trace written");
            }
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

/// Selects the original inspector without a step hook on normal executions.
pub(crate) fn maybe_trace<I>(
    inspector: &mut I,
    chain_id: u64,
) -> Either<&mut I, (&mut I, StepTracer)> {
    if active() {
        Either::Right((inspector, StepTracer::new(chain_id)))
    } else {
        Either::Left(inspector)
    }
}

pub(crate) struct StepTracer {
    chain_id: u64,
    gas: GasInspector,
    pc: usize,
    opcode: u8,
    gas_before: u64,
    refund_before: i64,
    storage_key: Option<String>,
}

impl StepTracer {
    fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            gas: GasInspector::new(),
            pc: 0,
            opcode: 0,
            gas_before: 0,
            refund_before: 0,
            storage_key: None,
        }
    }
}

impl<CTX: ContextTr> Inspector<CTX> for StepTracer {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        self.gas.initialize_interp(&interp.gas);
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        self.gas.step(&interp.gas);
        self.pc = interp.bytecode.pc();
        self.opcode = interp.bytecode.opcode();
        self.gas_before = interp.gas.remaining();
        self.refund_before = interp.gas.refunded();
        self.storage_key = if matches!(self.opcode, 0x54 | 0x55) {
            interp.stack.peek(0).ok().map(|key| format!("{key:#x}"))
        } else {
            None
        };
    }

    fn step_end(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        self.gas.step_end(&interp.gas);
        let value = json!({
            "kind": "step", "chain_id": self.chain_id,
            "address": interp.input.target_address, "depth": context.journal_ref().depth(),
            "pc": self.pc, "op": self.opcode, "gas_before": self.gas_before,
            "gas_cost": self.gas.last_gas_cost(), "gas_after": interp.gas.remaining(),
            "refund_before": self.refund_before, "refund_after": interp.gas.refunded(),
            "storage_key": self.storage_key,
        });
        ACTIVE.with(|active| {
            if let Some(sink) = active.borrow_mut().as_mut() {
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
}
