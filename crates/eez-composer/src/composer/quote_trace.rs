//! Request-local call traces. The blocking quote worker and all synchronous
//! cross-chain callbacks share one collector; nothing is written to disk.
use alloy_consensus::{Transaction, Typed2718, transaction::Recovered};
use alloy_primitives::{Address, Log, U256, hex};
use evm2::{
    EvmTypesHost, Inspector,
    evm::inspector::InspectorConfig,
    interpreter::{InstrStop, Interpreter, Message, MessageResult},
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

    fn begin_run<T: Transaction + Typed2718>(
        &mut self,
        chain: u64,
        purpose: &'static str,
        tx: &Recovered<T>,
    ) -> bool {
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
        let input = self.bytes(tx.input());
        let mut transaction = json!({
            "type":format!("{:#x}",tx.ty()),"chainId":format!("{chain:#x}"),
            "from":tx.signer(),"to":tx.to(),"nonce":format!("{:#x}",tx.nonce()),
            "gas":format!("{:#x}",tx.gas_limit()),"value":format!("{:#x}",tx.value()),"input":input
        });
        if let Some(tip) = tx.max_priority_fee_per_gas() {
            transaction["maxFeePerGas"] = json!(format!("{:#x}", tx.max_fee_per_gas()));
            transaction["maxPriorityFeePerGas"] = json!(format!("{tip:#x}"));
        } else {
            transaction["gasPrice"] = json!(format!("{:#x}", tx.max_fee_per_gas()));
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

    fn enter<T: EvmTypesHost>(
        &mut self,
        chain: u64,
        interp: &Interpreter<'_, '_, T>,
        input: &Message<T>,
        root: bool,
    ) {
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
        let data = self.bytes(&input.input.to_bytes(interp.memory()));
        let creates = input.kind.is_create();
        let mut frame = json!({
            "kind":format!("{:?}",input.kind).to_uppercase(),
            "from":input.caller,"to":if creates {None} else {Some(input.destination)},
            "codeAddress":if creates {None} else {Some(input.code_address)},
            "value":format!("{:#x}",input.value),"gas":format!("{:#x}",input.gas_limit),
            "isStatic":input.is_static(),"input":data
        });
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

    fn end<T: EvmTypesHost>(&mut self, message: &Message<T>, outcome: &MessageResult<T>) {
        if self.omitted_depth > 0 {
            self.omitted_depth -= 1;
            return;
        }
        let (Some(run), Some(frame)) = (self.current_run, self.stack.pop()) else {
            self.truncated = true;
            return;
        };
        let output = self.bytes(&outcome.output);
        let frame = &mut self.runs[run]["frames"][frame];
        frame["output"] = output;
        frame["status"] = json!(if outcome.stop.is_success() {
            "success"
        } else if outcome.stop == InstrStop::Revert {
            "revert"
        } else {
            "halt"
        });
        frame["result"] = json!(format!("{:?}", outcome.stop));
        frame["gasUsed"] = json!(format!("{:#x}", outcome.gas.spent()));
        if message.kind.is_create() {
            frame["to"] = json!(outcome.created_address);
            frame["codeAddress"] = json!(outcome.created_address);
        }
    }
}

/// With no collector, this only forwards the existing inspector's hooks. Use
/// this wrapper to bracket before synchronous dispatch in call/create hooks.
/// Nested EVMs join the active tree, including intercepted calls with no bytecode.
pub(crate) fn inspect<I, T: Transaction + Typed2718>(
    inner: I,
    chain: u64,
    purpose: &'static str,
    tx: &Recovered<T>,
) -> CallTracer<I> {
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

impl<I> CallTracer<I> {
    fn begin_frame<T: EvmTypesHost>(
        &mut self,
        interp: &Interpreter<'_, '_, T>,
        input: &Message<T>,
    ) {
        if self.capture.is_some() {
            ACTIVE.with(|active| {
                if let Some(c) = active.borrow_mut().as_mut() {
                    c.enter(self.chain, interp, input, self.first);
                }
            });
        }
        self.first = false;
    }
    fn end_frame<T: EvmTypesHost>(&self, input: &Message<T>, result: &MessageResult<T>) {
        if self.capture.is_some() {
            ACTIVE.with(|active| {
                if let Some(c) = active.borrow_mut().as_mut() {
                    c.end::<T>(input, result);
                }
            });
        }
    }
}

impl<T: EvmTypesHost, I: Inspector<T>> Inspector<T> for CallTracer<I> {
    fn inspector_config(&self) -> InspectorConfig {
        self.inner.inspector_config()
    }
    fn initialize_interp(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        self.inner.initialize_interp(interp);
        if self.capture.is_none() {
            return;
        }
        ACTIVE.with(|active| {
            if let Some(c) = active.borrow_mut().as_mut()
                && c.omitted_depth == 0
                && let (Some(run), Some(frame)) = (c.current_run, c.stack.last().copied())
            {
                c.runs[run]["frames"][frame]["isStatic"] = json!(interp.is_static());
                c.runs[run]["frames"][frame]["from"] = json!(interp.message().caller);
            }
        });
    }
    fn step(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        self.inner.step(interp);
    }
    fn step_end(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        self.inner.step_end(interp);
    }
    fn log(&mut self, log: &Log, host: &mut T::Host<'_>) {
        self.inner.log(log, host);
    }
    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        input: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        self.begin_frame(interp, input);
        self.inner.call(interp, input)
    }
    fn call_end(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        input: &Message<T>,
        result: &mut MessageResult<T>,
    ) {
        self.inner.call_end(interp, input, result);
        self.end_frame::<T>(input, result);
    }
    fn create(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        input: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        self.begin_frame(interp, input);
        self.inner.create(interp, input)
    }
    fn create_end(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        input: &Message<T>,
        result: &mut MessageResult<T>,
    ) {
        self.inner.create_end(interp, input, result);
        self.end_frame::<T>(input, result);
    }
    fn selfdestruct(
        &mut self,
        contract: &Address,
        target: &Address,
        value: &U256,
        host: &mut T::Host<'_>,
    ) {
        self.inner.selfdestruct(contract, target, value, host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use evm2::{
        BaseEvmTypes, Evm, SpecId,
        evm::{InMemoryDB, inspector::NoopInspector},
        interpreter::{GasTracker, MessageExt},
    };
    type NoOpInspector = NoopInspector;
    fn input() -> Message<BaseEvmTypes> {
        MessageExt {
            caller: Address::repeat_byte(1),
            destination: Address::repeat_byte(2),
            code_address: Address::repeat_byte(3),
            gas_limit: 100_000,
            ..Default::default()
        }
    }
    fn transaction() -> Recovered<alloy_consensus::TxLegacy> {
        Recovered::new_unchecked(Default::default(), Address::ZERO)
    }
    fn context() -> Evm<'static, BaseEvmTypes> {
        Evm::new(
            SpecId::OSAKA,
            Default::default(),
            evm2::ethereum::ethereum_tx_registry(SpecId::OSAKA),
            InMemoryDB::default(),
            evm2::precompiles::Precompiles::base(SpecId::OSAKA),
        )
    }
    fn result(success: bool) -> MessageResult<BaseEvmTypes> {
        evm2::interpreter::MessageResultExt {
            stop: if success {
                InstrStop::Return
            } else {
                InstrStop::Revert
            },
            output: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            gas: GasTracker::new(100_000),
            ..Default::default()
        }
    }
    impl CallTracer<NoOpInspector> {
        fn frame_start(
            &mut self,
            context: &mut Evm<'_, BaseEvmTypes>,
            input: &mut Message<BaseEvmTypes>,
        ) {
            let frame = input.clone();
            let mut interp = Interpreter::<BaseEvmTypes>::new(&frame);
            interp.prepare_run(context);
            let _ = self.call(&mut interp, input);
        }
        fn frame_end(
            &mut self,
            context: &mut Evm<'_, BaseEvmTypes>,
            input: &Message<BaseEvmTypes>,
            result: &mut MessageResult<BaseEvmTypes>,
        ) {
            let mut interp = Interpreter::<BaseEvmTypes>::new(input);
            interp.prepare_run(context);
            self.call_end(&mut interp, input, result);
        }
    }

    #[test]
    fn links_nested_chains_and_preserves_caught_revert_evidence() {
        let scope = Scope::begin(json!({}), json!({}));
        phase(2, "fee_simulation");
        let mut context = context();
        let mut source = inspect(NoopInspector::default(), 1, "composition", &transaction());
        let mut source_input = input();
        source.frame_start(&mut context, &mut source_input);
        {
            let mut remote = inspect(NoopInspector::default(), 42, "composition", &transaction());
            let mut remote_input = input();
            remote.frame_start(&mut context, &mut remote_input);
            {
                let mut callback =
                    inspect(NoopInspector::default(), 1, "composition", &transaction());
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
        let mut context = context();
        let mut source = inspect(NoopInspector::default(), 1, "composition", &transaction());
        source.frame_start(&mut context, &mut input());
        {
            let mut remote = inspect(NoopInspector::default(), 42, "composition", &transaction());
            remote.frame_start(&mut context, &mut input());
            // A provider error need not call frame_end.
        }
        source.frame_end(&mut context, &input(), &mut result(false));
        drop(source);
        phase(0, "final_validation");
        {
            let mut next = inspect(
                NoopInspector::default(),
                42,
                "native_source",
                &transaction(),
            );
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
        let mut context = context();
        let mut source = inspect(NoopInspector::default(), 1, "composition", &transaction());
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
        assert_eq!(final_result.stop, InstrStop::Return);
        drop(inspect(
            NoopInspector::default(),
            1,
            "native_source",
            &transaction(),
        ));
        let trace = scope.finish();
        assert_eq!(trace["runs"][0]["frames"].as_array().unwrap().len(), 1);
        assert_eq!(trace["omittedFrames"], 10);
        assert_eq!(trace["omittedRuns"], 1);
        assert_eq!(trace["truncated"], true);

        let scope = Scope::begin(json!({}), json!({}));
        for _ in 0..=MAX_RUNS {
            drop(inspect(
                NoopInspector::default(),
                1,
                "composition",
                &transaction(),
            ));
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
                inspect(NoopInspector::default(), 1, "composition", &transaction())
                    .capture
                    .is_none()
            );
        })
        .join()
        .unwrap();
        assert_eq!(outer.finish()["request"]["name"], "outer");
        assert!(
            inspect(NoopInspector::default(), 1, "composition", &transaction())
                .capture
                .is_none()
        );
    }
}
