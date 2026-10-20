//! Request-local call traces. The blocking quote worker and all synchronous
//! cross-chain callbacks share one collector; nothing is written to disk.
use alloy_primitives::{Address, Log, U256, hex};
use revm::{
    Inspector,
    context::TxEnv,
    context_interface::ContextTr,
    handler::FrameResult,
    interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome, FrameInput, Interpreter},
};
use serde_json::{Value, json};
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

const MAX_RUNS: usize = 128;
const MAX_FRAMES: usize = 4096;
const MAX_DATA_BYTES: usize = 1024 * 1024;
const MAX_FIELD_BYTES: usize = 4096;

thread_local! {
    static ACTIVE: RefCell<Option<Collector>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Collector {
    context: Value,
    request: Value,
    phase: &'static str,
    transaction_type: u8,
    pass: usize,
    runs: Vec<Value>,
    current_run: Option<usize>,
    // A shared stack, rather than EVM-local depth, links reentrant executions
    // on different chains to the source frame suspended inside its inspector.
    stack: Vec<usize>,
    omitted_depth: usize,
    frame_count: usize,
    data_bytes: usize,
    omitted_frames: usize,
    omitted_runs: usize,
    truncated: bool,
}

/// This guard must stay on the blocking worker's thread. Restoration on unwind
/// prevents a failed request from leaking a collector into a reused worker.
pub(crate) struct Scope {
    previous: Option<Collector>,
    _thread: PhantomData<Rc<()>>,
}

impl Scope {
    pub(crate) fn begin(request: Value, context: Value) -> Self {
        let previous = ACTIVE.with(|active| {
            active.replace(Some(Collector {
                context,
                request,
                ..Default::default()
            }))
        });
        Self {
            previous,
            _thread: PhantomData,
        }
    }

    pub(crate) fn finish(self) -> Value {
        let collector = ACTIVE.with(RefCell::take);
        drop(self);
        let Some(c) = collector else {
            return Value::Null;
        };
        json!({
            "format":"eez-quote-call-trace", "version":1,
            "request":c.request, "context":c.context, "runs":c.runs,
            "truncated":c.truncated, "omittedFrames":c.omitted_frames,
            "omittedRuns":c.omitted_runs,
            "limits":{"runs":MAX_RUNS,"frames":MAX_FRAMES,
                "dataBytes":MAX_DATA_BYTES,"fieldBytes":MAX_FIELD_BYTES},
            "gasAccounting":"inclusive_frame_execution_gas",
        })
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

pub(crate) fn phase(transaction_type: u8, phase: &'static str) {
    ACTIVE.with(|active| {
        if let Some(c) = active.borrow_mut().as_mut() {
            c.transaction_type = transaction_type;
            c.phase = phase;
            c.pass = 0;
        }
    });
}

impl Collector {
    fn bytes(&mut self, data: &[u8]) -> Value {
        let count = data
            .len()
            .min(MAX_FIELD_BYTES)
            .min(MAX_DATA_BYTES.saturating_sub(self.data_bytes));
        self.data_bytes += count;
        self.truncated |= count < data.len();
        json!({"hex":hex::encode_prefixed(&data[..count]),"length":data.len(),
            "truncated":count < data.len()})
    }

    fn begin_run(&mut self, chain: u64, purpose: &'static str, tx: &TxEnv) -> bool {
        if !self.stack.is_empty() || self.omitted_depth != 0 {
            return false;
        }
        if purpose == "composition" {
            self.pass += 1;
        }
        if self.runs.len() >= MAX_RUNS || self.frame_count >= MAX_FRAMES {
            self.omitted_runs += 1;
            self.truncated = true;
            self.current_run = None;
            return true;
        }
        let input = self.bytes(&tx.data);
        let mut transaction = json!({
            "type":format!("{:#x}",tx.tx_type),"chainId":format!("{chain:#x}"),
            "from":tx.caller,"to":tx.kind.to(),"nonce":format!("{:#x}",tx.nonce),
            "gas":format!("{:#x}",tx.gas_limit),"value":format!("{:#x}",tx.value),"input":input
        });
        if let Some(tip) = tx.gas_priority_fee {
            transaction["maxFeePerGas"] = json!(format!("{:#x}", tx.gas_price));
            transaction["maxPriorityFeePerGas"] = json!(format!("{tip:#x}"));
        } else {
            transaction["gasPrice"] = json!(format!("{:#x}", tx.gas_price));
        }
        self.current_run = Some(self.runs.len());
        self.runs.push(json!({
            "id":self.runs.len(),"phase":self.phase,"type":format!("{:#x}",self.transaction_type),
            "pass":self.pass,
            "purpose":purpose,"chainId":format!("{chain:#x}"),
            "transaction":transaction,
            "frames":[],"complete":false
        }));
        true
    }

    fn enter<C: ContextTr>(&mut self, chain: u64, context: &C, input: &FrameInput, root: bool) {
        let Some(run) = self.current_run else {
            self.omitted_depth += 1;
            self.omitted_frames += 1;
            return;
        };
        if self.omitted_depth != 0 || self.frame_count >= MAX_FRAMES {
            self.omitted_depth += 1;
            self.omitted_frames += 1;
            self.truncated = true;
            return;
        }
        let mut frame = match input {
            FrameInput::Call(call) => {
                let data = self.bytes(&call.input.as_bytes(context));
                json!({
                    "kind":format!("{:?}",call.scheme).to_uppercase(),
                    "from":call.caller,"to":call.target_address,
                    "codeAddress":call.bytecode_address,"value":format!("{:#x}",call.call_value()),
                    "gas":format!("{:#x}",call.gas_limit),"isStatic":call.is_static,"input":data
                })
            }
            FrameInput::Create(create) => {
                let data = self.bytes(create.init_code());
                let kind = match create.scheme() {
                    revm::context_interface::CreateScheme::Create => "CREATE",
                    revm::context_interface::CreateScheme::Create2 { .. } => "CREATE2",
                    revm::context_interface::CreateScheme::Custom { .. } => "CREATE_CUSTOM",
                };
                json!({"kind":kind,"from":create.caller(),"to":null,"codeAddress":null,
                    "value":format!("{:#x}",create.value()),"gas":format!("{:#x}",create.gas_limit()),
                    "isStatic":false,"input":data})
            }
            FrameInput::Empty => json!({"kind":"EMPTY","input":self.bytes(&[]),
                "from":null,"to":null,"codeAddress":null,"value":"0x0","gas":"0x0","isStatic":false}),
        };
        let frames = self.runs[run]["frames"]
            .as_array_mut()
            .expect("collector constructs frames as an array");
        let parent = self.stack.last().copied();
        let chain = format!("{chain:#x}");
        frame["id"] = json!(frames.len());
        frame["parentId"] = json!(parent);
        frame["chainId"] = json!(chain);
        frame["executionRoot"] = json!(root);
        frame["crossChain"] = json!(parent.is_some_and(|p| frames[p]["chainId"] != chain));
        frame["status"] = json!("incomplete");
        self.stack.push(frames.len());
        frames.push(frame);
        self.frame_count += 1;
    }

    fn end(&mut self, result: &FrameResult) {
        if self.omitted_depth > 0 {
            self.omitted_depth -= 1;
            return;
        }
        let (Some(run), Some(frame)) = (self.current_run, self.stack.pop()) else {
            self.truncated = true;
            return;
        };
        let outcome = result.interpreter_result();
        let output = self.bytes(&outcome.output);
        let frame = &mut self.runs[run]["frames"][frame];
        frame["output"] = output;
        frame["status"] = json!(if outcome.result.is_ok() {
            "success"
        } else if outcome.result.is_revert() {
            "revert"
        } else {
            "halt"
        });
        frame["result"] = json!(format!("{:?}", outcome.result));
        frame["gasUsed"] = json!(format!("{:#x}", outcome.gas.total_gas_spent()));
        if let FrameResult::Create(create) = result {
            frame["to"] = json!(create.address);
            frame["codeAddress"] = json!(create.address);
        }
    }
}

/// With no collector, this only forwards the existing inspector's hooks. Use
/// our wrapper rather than REVM 43's `Either` inspector: that implementation
/// does not forward frame_start/frame_end. Nested EVMs join the active tree.
pub(crate) fn inspect<I>(inner: I, chain: u64, purpose: &'static str, tx: &TxEnv) -> CallTracer<I> {
    let capture = ACTIVE.with(|active| {
        active.borrow_mut().as_mut().map(|c| {
            let owner = c.begin_run(chain, purpose, tx);
            Capture {
                owner,
                stack_depth: c.stack.len(),
                omitted_depth: c.omitted_depth,
            }
        })
    });
    CallTracer {
        inner,
        chain,
        capture,
        first: true,
    }
}

pub(crate) struct CallTracer<I> {
    inner: I,
    chain: u64,
    first: bool,
    capture: Option<Capture>,
}

impl<I> CallTracer<I> {
    pub(crate) const fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }
}

struct Capture {
    owner: bool,
    stack_depth: usize,
    omitted_depth: usize,
}

impl<I> Drop for CallTracer<I> {
    fn drop(&mut self) {
        let Some(capture) = &self.capture else { return };
        ACTIVE.with(|active| {
            if let Some(c) = active.borrow_mut().as_mut() {
                let balanced = c.stack.len() == capture.stack_depth
                    && c.omitted_depth == capture.omitted_depth;
                if !balanced {
                    c.truncated = true;
                }
                if capture.owner
                    && let Some(run) = c.current_run.take()
                {
                    let has_frames = c.runs[run]["frames"].as_array().is_some_and(|frames| {
                        !frames.is_empty()
                            && frames.iter().all(|frame| frame["status"] != "incomplete")
                    });
                    c.runs[run]["complete"] =
                        json!(balanced && has_frames && !std::thread::panicking());
                }
                // Nested provider failures can also skip end hooks. Unwind
                // only this EVM's frames, preserving its suspended parent.
                c.stack.truncate(capture.stack_depth);
                c.omitted_depth = capture.omitted_depth;
            }
        });
    }
}

impl<C: ContextTr, I: Inspector<C>> Inspector<C> for CallTracer<I> {
    fn frame_start(&mut self, context: &mut C, input: &mut FrameInput) -> Option<FrameResult> {
        if self.capture.is_some() {
            ACTIVE.with(|active| {
                if let Some(c) = active.borrow_mut().as_mut() {
                    c.enter(self.chain, context, input, self.first);
                }
            });
        }
        self.first = false;
        self.inner.frame_start(context, input)
    }

    fn frame_end(&mut self, context: &mut C, input: &FrameInput, result: &mut FrameResult) {
        self.inner.frame_end(context, input, result);
        if self.capture.is_some() {
            ACTIVE.with(|active| {
                if let Some(c) = active.borrow_mut().as_mut() {
                    c.end(result);
                }
            });
        }
    }

    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut C) {
        self.inner.initialize_interp(interp, context);
        if self.capture.is_none() {
            return;
        }
        ACTIVE.with(|active| {
            if let Some(c) = active.borrow_mut().as_mut()
                && c.omitted_depth == 0
                && let (Some(run), Some(frame)) = (c.current_run, c.stack.last().copied())
            {
                // Static synthetic roots and manager forwarding can adjust the
                // interpreter context after frame_start.
                c.runs[run]["frames"][frame]["isStatic"] = json!(interp.runtime_flag.is_static);
                c.runs[run]["frames"][frame]["from"] = json!(interp.input.caller_address);
            }
        });
    }

    fn step(&mut self, interp: &mut Interpreter, context: &mut C) {
        self.inner.step(interp, context);
    }
    fn step_end(&mut self, interp: &mut Interpreter, context: &mut C) {
        self.inner.step_end(interp, context);
    }
    fn log(&mut self, context: &mut C, log: Log) {
        self.inner.log(context, log);
    }
    fn log_full(&mut self, interp: &mut Interpreter, context: &mut C, log: Log) {
        self.inner.log_full(interp, context, log);
    }
    fn call(&mut self, context: &mut C, inputs: &mut CallInputs) -> Option<CallOutcome> {
        self.inner.call(context, inputs)
    }
    fn call_end(&mut self, context: &mut C, inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.inner.call_end(context, inputs, outcome);
    }
    fn create(&mut self, context: &mut C, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.inner.create(context, inputs)
    }
    fn create_end(&mut self, context: &mut C, inputs: &CreateInputs, outcome: &mut CreateOutcome) {
        self.inner.create_end(context, inputs, outcome);
    }
    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        self.inner.selfdestruct(contract, target, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use revm::{
        MainContext,
        context::Context,
        inspector::NoOpInspector,
        interpreter::{Gas, InstructionResult, InterpreterResult},
    };

    fn input() -> FrameInput {
        FrameInput::Call(Box::new(CallInputs {
            caller: Address::repeat_byte(1),
            target_address: Address::repeat_byte(2),
            bytecode_address: Address::repeat_byte(3),
            gas_limit: 100_000,
            input: Default::default(),
            return_memory_offset: 0..0,
            reservoir: 0,
            known_bytecode: Default::default(),
            value: revm::interpreter::CallValue::Transfer(U256::ZERO),
            scheme: revm::interpreter::CallScheme::Call,
            is_static: false,
            charged_new_account_state_gas: false,
        }))
    }

    fn result(success: bool) -> FrameResult {
        FrameResult::Call(CallOutcome::new(
            InterpreterResult::new(
                if success {
                    InstructionResult::Return
                } else {
                    InstructionResult::Revert
                },
                Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
                Gas::new(100_000),
            ),
            0..0,
        ))
    }

    #[test]
    fn links_nested_chains_and_preserves_caught_revert_evidence() {
        let scope = Scope::begin(json!({}), json!({}));
        phase(2, "fee_simulation");
        let mut context = Context::mainnet();
        let mut source = inspect(NoOpInspector, 1, "composition", &TxEnv::default());
        let mut source_input = input();
        source.frame_start(&mut context, &mut source_input);
        {
            let mut remote = inspect(NoOpInspector, 42, "composition", &TxEnv::default());
            let mut remote_input = input();
            remote.frame_start(&mut context, &mut remote_input);
            {
                let mut callback = inspect(NoOpInspector, 1, "composition", &TxEnv::default());
                callback.frame_start(&mut context, &mut input());
                callback.frame_end(&mut context, &input(), &mut result(true));
            }
            remote.frame_end(&mut context, &remote_input, &mut result(false));
        }
        source.frame_end(&mut context, &source_input, &mut result(true));
        drop(source);
        let trace = scope.finish();
        let run = &trace["runs"][0];
        let frames = run["frames"].as_array().unwrap();
        assert_eq!(trace["runs"].as_array().unwrap().len(), 1);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["status"], "success");
        assert_eq!(frames[1]["status"], "revert");
        assert_eq!(frames[2]["status"], "success");
        assert_eq!(frames[1]["parentId"], 0);
        assert_eq!(frames[2]["parentId"], 1);
        assert_eq!(frames[1]["crossChain"], true);
        assert_eq!(frames[2]["crossChain"], true);
        assert_eq!(frames[1]["output"]["hex"], "0xdeadbeef");
        assert_eq!(frames[0]["codeAddress"], json!(Address::repeat_byte(3)));
        assert_eq!(run["complete"], true);
        assert_eq!(trace["truncated"], false);
    }

    #[test]
    fn interrupted_nested_evm_does_not_corrupt_parent_or_next_run() {
        let scope = Scope::begin(json!({}), json!({}));
        let mut context = Context::mainnet();
        let mut source = inspect(NoOpInspector, 1, "composition", &TxEnv::default());
        source.frame_start(&mut context, &mut input());
        {
            let mut remote = inspect(NoOpInspector, 42, "composition", &TxEnv::default());
            remote.frame_start(&mut context, &mut input());
            // A provider error need not call frame_end.
        }
        source.frame_end(&mut context, &input(), &mut result(false));
        drop(source);
        phase(0, "final_validation");
        {
            let mut next = inspect(NoOpInspector, 42, "native_source", &TxEnv::default());
            next.frame_start(&mut context, &mut input());
            next.frame_end(&mut context, &input(), &mut result(true));
        }
        let trace = scope.finish();
        assert_eq!(trace["runs"][0]["frames"][0]["status"], "revert");
        assert_eq!(trace["runs"][0]["frames"][1]["status"], "incomplete");
        assert_eq!(trace["runs"][1]["frames"][0]["parentId"], Value::Null);
        assert_eq!(trace["runs"][1]["type"], "0x0");
        assert_eq!(trace["runs"][1]["phase"], "final_validation");
        assert_eq!(trace["truncated"], true);
    }

    #[test]
    fn bounds_bytes_frames_and_runs_without_interfering_with_outcomes() {
        let scope = Scope::begin(json!({}), json!({}));
        let mut context = Context::mainnet();
        let mut source = inspect(NoOpInspector, 1, "composition", &TxEnv::default());
        source.frame_start(&mut context, &mut input());
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let c = active.as_mut().unwrap();
            let data = c.bytes(&vec![0xaa; MAX_FIELD_BYTES + 1]);
            assert_eq!(data["length"], MAX_FIELD_BYTES + 1);
            assert_eq!(data["hex"].as_str().unwrap().len(), 2 + MAX_FIELD_BYTES * 2);
            assert_eq!(data["truncated"], true);
            c.data_bytes = MAX_DATA_BYTES;
            assert_eq!(c.bytes(&[1, 2])["hex"], "0x");
            c.frame_count = MAX_FRAMES;
        });
        for _ in 0..10 {
            source.frame_start(&mut context, &mut input());
        }
        for _ in 0..10 {
            source.frame_end(&mut context, &input(), &mut result(false));
        }
        let mut final_result = result(true);
        source.frame_end(&mut context, &input(), &mut final_result);
        drop(source);
        assert_eq!(final_result.instruction_result(), InstructionResult::Return);
        drop(inspect(
            NoOpInspector,
            1,
            "native_source",
            &TxEnv::default(),
        ));
        let trace = scope.finish();
        assert_eq!(trace["runs"][0]["frames"].as_array().unwrap().len(), 1);
        assert_eq!(trace["omittedFrames"], 10);
        assert_eq!(trace["omittedRuns"], 1);
        assert_eq!(trace["truncated"], true);

        let scope = Scope::begin(json!({}), json!({}));
        for _ in 0..=MAX_RUNS {
            drop(inspect(NoOpInspector, 1, "composition", &TxEnv::default()));
        }
        let trace = scope.finish();
        assert_eq!(trace["runs"].as_array().unwrap().len(), MAX_RUNS);
        assert_eq!(trace["omittedRuns"], 1);
        assert_eq!(trace["runs"][0]["complete"], false);
    }

    #[test]
    fn restores_scope_on_unwind_and_isolates_worker_threads() {
        let outer = Scope::begin(json!({"name":"outer"}), json!({}));
        let panic = std::panic::catch_unwind(|| {
            let _scope = Scope::begin(json!({"name":"inner"}), json!({}));
            panic!("fixture");
        });
        assert!(panic.is_err());
        std::thread::spawn(|| {
            assert!(
                inspect(NoOpInspector, 1, "composition", &TxEnv::default())
                    .capture
                    .is_none()
            );
        })
        .join()
        .unwrap();
        assert_eq!(outer.finish()["request"]["name"], "outer");
        assert!(
            inspect(NoOpInspector, 1, "composition", &TxEnv::default())
                .capture
                .is_none()
        );
    }
}
