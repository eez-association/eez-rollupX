//! Bounded quote workers and waiting requests, with reserved signed-admission
//! capacity. Execution owns both permits through timeout or cancellation.

use alloy_consensus::Header;
use eez_composer::{
    Composer,
    composer::{CompactFeeRequest, FeeQuoteError, L2FeeRequest},
};
use hyper::StatusCode;
use reth_storage_api::{BlockReader, StateProviderFactory};
use reth_tasks::TaskExecutor;
use serde_json::{Value, json};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

pub(crate) const METHOD: &str = "eez_quoteFee";
pub(crate) const DEBUG_METHOD: &str = "eez_debugQuoteFee";
// Hex encoding alone takes 128 KiB for maximum-size calldata; leave room for
// transaction fields and options while retaining a bounded per-quote input.
pub(crate) const MAX_BODY: usize = 160 * 1024;
type Work = Box<dyn FnOnce(Arc<AtomicBool>) -> Result<Value, FeeQuoteError> + Send>;
type Preparation = Pin<Box<dyn Future<Output = Result<Work, FeeQuoteError>> + Send>>;

#[derive(Clone)]
enum Request {
    Signed(Box<L2FeeRequest>),
    Compact(CompactFeeRequest, bool),
}

pub(crate) struct FeeQuotes {
    prepare: Arc<dyn Fn(Request) -> Preparation + Send + Sync>,
    executor: TaskExecutor,
    quotes: Workers,
    admission: Workers,
    timeout: Duration,
    pub(crate) enforce_tariff: bool,
}

struct Workers {
    active: Arc<Semaphore>,
    capacity: Arc<Semaphore>,
}

impl Workers {
    fn new(active: usize, waiting: usize) -> Self {
        Self {
            active: Arc::new(Semaphore::new(active)),
            capacity: Arc::new(Semaphore::new(active + waiting)),
        }
    }
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl FeeQuotes {
    pub(crate) fn new<L2>(
        composer: Composer<L2>,
        rollup_id: u64,
        direction: eez_composer::Direction,
        executor: TaskExecutor,
    ) -> Arc<Self>
    where
        L2: BlockReader<Block = eez_primitives::Block, Header = Header>
            + StateProviderFactory
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let enforce_tariff = composer.enforces_fee_tariff(direction);
        Arc::new(Self {
            enforce_tariff,
            prepare: Arc::new(move |request| {
                let composer = composer.clone();
                Box::pin(async move {
                    match request {
                        Request::Signed(request) => {
                            let prepared = composer
                                .prepare_fee_quote(direction, rollup_id, *request)
                                .await?;
                            Ok(Box::new(move |cancelled| {
                                composer.quote_signed_fee(prepared, cancelled)
                            }) as Work)
                        }
                        Request::Compact(request, debug) => {
                            let prepared = composer
                                .prepare_compact_fee_quote(rollup_id, direction, request)
                                .await?;
                            Ok(Box::new(move |cancelled| {
                                composer.quote_fee(prepared, cancelled, debug)
                            }) as Work)
                        }
                    }
                })
            }),
            executor,
            // Each worker has independent EVM overlays and the quote profile's
            // gas/DA bounds. A small fixed pool also bounds worst-case memory.
            quotes: Workers::new(4, 16),
            admission: Workers::new(1, 8),
            timeout: Duration::from_secs(10),
        })
    }

    pub(crate) async fn compact(
        &self,
        transaction: &Value,
        options: &Value,
        debug: bool,
    ) -> (StatusCode, Value) {
        if !debug && options.get("trace").is_some() {
            return failure(FeeQuoteError::Invalid(
                "trace is available only in eez_debugQuoteFee",
            ));
        }
        if transaction.to_string().len() + options.to_string().len() > MAX_BODY {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "quote_limit",
                "quote exceeds 160 KiB",
            );
        }
        match CompactFeeRequest::parse(transaction, options) {
            Ok(request) => self.run(Request::Compact(request, debug)).await,
            Err(error) => failure(error),
        }
    }

    pub(crate) async fn check_signed(
        &self,
        envelope: &alloy_consensus::TxEnvelope,
    ) -> (StatusCode, Value) {
        match L2FeeRequest::from_signed(envelope) {
            Ok(request) => self.run(Request::Signed(Box::new(request))).await,
            Err(error) => failure(error),
        }
    }

    async fn run(&self, request: Request) -> (StatusCode, Value) {
        let workers = match &request {
            Request::Signed(_) => &self.admission,
            Request::Compact(..) => &self.quotes,
        };
        let Ok(slot) = Arc::clone(&workers.capacity).try_acquire_owned() else {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "quote_busy",
                "quote request capacity is full",
            );
        };
        let cancellation = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let cancelled = Arc::clone(&cancellation.0);
        let operation = async {
            let permit = Arc::clone(&workers.active)
                .acquire_owned()
                .await
                .map_err(|_| FeeQuoteError::Unavailable("quote service stopped"))?;
            let mut permits = (slot, permit);
            for refresh in 0..=8 {
                let work = (self.prepare)(request.clone()).await?;
                let cancelled = Arc::clone(&cancelled);
                let (result, returned_permits) = self
                    .executor
                    .spawn_blocking(move || {
                        // Cancellation cannot release capacity while synchronous
                        // execution is running, including a refreshed attempt.
                        let result = work(cancelled);
                        (result, permits)
                    })
                    .await
                    .map_err(|_| FeeQuoteError::Unavailable("quote worker failed"))?;
                permits = returned_permits;
                if refresh == 8
                    || !matches!(
                        &result,
                        Err(FeeQuoteError::SourceSnapshot {
                            before_execution: true,
                            ..
                        })
                    )
                {
                    return result;
                }
                // Refresh the complete prepared L1/L2 context, not just a nonce.
                // This cheap pre-execution condition normally clears when the
                // settlement scan catches up. Keep capacity and the original
                // request timeout; persistent lag returns its typed diagnostic.
                tracing::debug!(refresh, error = ?result.as_ref().err(), "refreshing fee quote source snapshot");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            unreachable!("bounded refresh loop returns on its last iteration")
        };
        tokio::select! {
            _=self.executor.on_shutdown_signal().clone()=>error(StatusCode::SERVICE_UNAVAILABLE,"shutting_down","quote service is shutting down"),
            result=tokio::time::timeout(self.timeout,operation)=>match result {
                Ok(Ok(value))=>(StatusCode::OK,value),
                Ok(Err(err))=>failure(err),
                Err(_)=>error(StatusCode::GATEWAY_TIMEOUT,"quote_timeout","quote work timed out"),
            }
        }
    }
}

/// Translate the shared quote result without turning operational unavailability
/// into a JSON-RPC protocol error. Admission retains its distinct -32003 code.
pub(crate) fn rpc_response(id: Value, status: StatusCode, value: Value) -> Value {
    if status.is_success() {
        json!({"jsonrpc":"2.0", "id":id, "result":value})
    } else {
        let code = if status == StatusCode::BAD_REQUEST {
            -32602
        } else {
            -32004
        };
        json!({"jsonrpc":"2.0", "id":id,
            "error":{"code":code,"message":value["error"]["message"],"data":value["error"]}})
    }
}

pub(crate) fn error(status: StatusCode, code: &str, message: &str) -> (StatusCode, Value) {
    (status, json!({"error":{"code":code,"message":message}}))
}

fn failure(err: FeeQuoteError) -> (StatusCode, Value) {
    let status = match err {
        FeeQuoteError::Invalid(_) => StatusCode::BAD_REQUEST,
        FeeQuoteError::Unsupported(_)
        | FeeQuoteError::Execution { .. }
        | FeeQuoteError::InsufficientFunds(_)
        | FeeQuoteError::Limit(_)
        | FeeQuoteError::Underfunded(_)
        | FeeQuoteError::L1Underfunded(_)
        | FeeQuoteError::SourceFeeCap { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        FeeQuoteError::Stale | FeeQuoteError::SourceSnapshot { .. } => StatusCode::CONFLICT,
        FeeQuoteError::Disabled => StatusCode::NOT_FOUND,
        FeeQuoteError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    let (status, mut response) = error(status, err.code(), &err.to_string());
    if let Some(details) = err.diagnostics() {
        response["error"]["details"] = details;
    }
    (status, response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    fn funding_shortfall() -> Box<eez_composer::FundingDiagnostic> {
        Box::new(eez_composer::FundingDiagnostic {
            required: alloy_primitives::U256::from(200),
            offered: alloy_primitives::U256::from(100),
            payment_chain_id: 1,
            source_gas_used: 21_000,
            source_base_fee: 49,
            candidate_prefix: false,
            l1_sample: eez_l1::L1BlockRef {
                number: 10,
                hash: alloy_primitives::B256::repeat_byte(1),
            },
            l2_parent: eez_l1::L1BlockRef {
                number: 20,
                hash: alloy_primitives::B256::repeat_byte(2),
            },
            l1_priority_fee: 1_000_000_000,
            l1_execution_price: 1_000_000_008,
            l1_blob_price: 1,
        })
    }

    #[test]
    fn funding_errors_preserve_measured_amounts_and_distinguish_source_base_fee() {
        for error in [
            FeeQuoteError::Underfunded(funding_shortfall()),
            FeeQuoteError::L1Underfunded(funding_shortfall()),
        ] {
            let code = error.code();
            let (status, response) = failure(error);
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(response["error"]["code"], code);
            let details = &response["error"]["details"];
            assert_eq!(details["reason"], "settlement_fee");
            assert_eq!(details["requiredPayment"], "0xc8");
            assert_eq!(details["offeredPayment"], "0x64");
            assert_eq!(details["shortfall"], "0x64");
            assert_eq!(details["l1Sample"]["number"], "0xa");
            assert_eq!(details["sourceBaseFeePerGas"], "0x31");
        }
        let (_, response) = failure(FeeQuoteError::SourceFeeCap {
            cap: 48,
            base: 49,
            chain_id: 1,
        });
        assert_eq!(response["error"]["code"], "source_fee_cap_below_base_fee");
        assert_eq!(response["error"]["details"]["reason"], "source_base_fee");
        assert!(
            response["error"]["details"]
                .get("requiredPayment")
                .is_none()
        );
    }

    #[test]
    fn source_balance_failure_keeps_amounts_in_rpc_error_data() {
        let (status, value) = failure(FeeQuoteError::InsufficientFunds(
            eez_composer::SourceBalanceError {
                available: alloy_primitives::U256::from(100),
                required: alloy_primitives::U256::from(200),
                chain_id: 906969,
            },
        ));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let response = rpc_response(json!("balance"), status, value);
        assert_eq!(response["id"], "balance");
        assert_eq!(response["error"]["code"], -32004);
        assert_eq!(response["error"]["data"]["code"], "insufficient_funds");
        assert_eq!(
            response["error"]["data"]["details"],
            json!({
                "reason":"source_balance", "sourceChainId":"0xdd6d9",
                "availableBalance":"0x64", "requiredBalance":"0xc8", "shortfall":"0x64"
            })
        );
    }

    fn transaction() -> Value {
        json!({"type":"0x2","chainId":"0x1","from":Address::repeat_byte(1),"to":Address::repeat_byte(2),"gas":"0x186a0","nonce":"0x0","value":"0x0","data":"0x"})
    }

    #[tokio::test]
    async fn compact_trace_option_is_rejected_before_preparation() {
        let service = FeeQuotes {
            prepare: Arc::new(|_| panic!("compact trace must not acquire a worker")),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(1),
            enforce_tariff: false,
        };
        let (status, response) = service
            .compact(&transaction(), &json!({"trace":"calls"}), false)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["error"]["code"], "invalid_transaction");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("eez_debugQuoteFee")
        );
    }
    #[tokio::test]
    async fn queued_predecessor_requires_stateful_admission_before_ordinary_forwarding() {
        use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
        use alloy_network::{TxSignerSync, eip2718::Encodable2718};
        use alloy_primitives::{B256, Bytes, TxKind};
        use alloy_provider::{ProviderBuilder, mock::Asserter};
        use alloy_signer_local::PrivateKeySigner;
        use eez_composer::{Direction, HeldPool, HeldTx};
        use jsonrpsee::{RpcModule, server::ServerBuilder};
        use std::sync::atomic::AtomicUsize;

        // Canonical state cannot see the predecessor's effects: the isolated
        // trace reverts before dispatch, but stateful tariff admission must run.
        let forwarded = Arc::new(AtomicUsize::new(0));
        let upstream = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}", upstream.local_addr().unwrap());
        let mut module = RpcModule::new(Arc::clone(&forwarded));
        module
            .register_method(
                "debug_traceCall",
                |_, _, _| json!({"type":"CALL", "error":"execution reverted", "calls":[]}),
            )
            .unwrap();
        module
            .register_method("eth_sendRawTransaction", |_, count, _| {
                count.fetch_add(1, Ordering::Relaxed);
                json!(B256::repeat_byte(7))
            })
            .unwrap();
        let upstream_handle = upstream.start(module);
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(1)).unwrap();
        let pool = Arc::new(HeldPool::new(Default::default()));
        pool.push_contiguous(
            HeldTx {
                raw_tx: Bytes::new(),
                hash: B256::repeat_byte(1),
                attempts: 0,
                excluded_slots: 0,
                sender: signer.address(),
                nonce: 0,
                direction: Direction::Inbound,
                max_fee_per_gas: 10,
                priority_fee_per_gas: 1,
            },
            0,
        )
        .unwrap();
        let service = Arc::new(FeeQuotes {
            enforce_tariff: true,
            prepare: Arc::new(|request| {
                assert!(matches!(request, Request::Signed(_)));
                Box::pin(async { Err(FeeQuoteError::L1Underfunded(funding_shortfall())) })
            }),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(10),
        });
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let front = tokio::spawn(crate::ingress::run_cross_chain_front(
            port,
            upstream_url,
            Direction::Inbound,
            Arc::clone(&pool),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            1,
            Address::repeat_byte(9),
            Arc::new(AtomicBool::new(true)),
            Arc::new(json!({})),
            Some(service),
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            while tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut tx = TxEip1559 {
            chain_id: 1,
            nonce: 1,
            gas_limit: 100_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::repeat_byte(2)),
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut tx).unwrap();
        let raw = Bytes::from(TxEnvelope::from(tx.into_signed(sig)).encoded_2718());
        let response: Value = reqwest::Client::new().post(format!("http://127.0.0.1:{port}"))
            .json(&json!({"jsonrpc":"2.0", "id":1, "method":"eth_sendRawTransaction", "params":[raw]}))
            .send().await.unwrap().json().await.unwrap();
        front.abort();
        upstream_handle.stop().unwrap();
        assert_eq!(response["error"]["code"], -32003, "{response}");
        assert_eq!(response["error"]["data"]["code"], "l1_tip_underfunded");
        assert_eq!(forwarded.load(Ordering::Relaxed), 0);
        assert_eq!(pool.len(), 1);
    }

    fn quote_service() -> Arc<FeeQuotes> {
        Arc::new(FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(|_| {
                Box::pin(async { Ok(Box::new(|_| Ok(json!({"quote":"shared"}))) as Work) })
            }),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(10),
        })
    }

    #[tokio::test]
    async fn front_rejects_trace_batches_before_executing_any_member() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let service = Arc::new(FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(move |_| {
                observed.fetch_add(1, Ordering::Relaxed);
                Box::pin(async { Ok(Box::new(|_| Ok(json!({"quote":"single"}))) as Work) })
            }),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(10),
        });
        let (url, task) = start_quote_front(service, Arc::new(AtomicBool::new(true))).await;
        let client = reqwest::Client::new();
        let traced = json!({"jsonrpc":"2.0","id":2,"method":DEBUG_METHOD,
            "params":[transaction(),{"trace":"calls"}]});
        for batch in [
            json!([traced]),
            json!([{"jsonrpc":"2.0","id":1,"method":METHOD,"params":[transaction()]},traced]),
        ] {
            let response: Value = client
                .post(&url)
                .json(&batch)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(response["error"]["code"], -32600, "{response}");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("individually")
            );
            assert_eq!(calls.load(Ordering::Relaxed), 0);
        }
        let response: Value = client
            .post(&url)
            .json(&traced)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["result"]["quote"], "single");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        task.abort();
    }

    async fn start_quote_front(
        service: Arc<FeeQuotes>,
        ready: Arc<AtomicBool>,
    ) -> (String, tokio::task::JoinHandle<eyre::Result<()>>) {
        use alloy_provider::{ProviderBuilder, mock::Asserter};
        use eez_composer::{Direction, HeldPool};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let task = tokio::spawn(crate::ingress::run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Outbound,
            Arc::new(HeldPool::new(Default::default())),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            1,
            Address::ZERO,
            ready,
            Arc::new(json!({})),
            Some(service),
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            while tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        (format!("http://127.0.0.1:{port}"), task)
    }

    #[tokio::test]
    async fn rpc_front_enforces_64_kib_calldata_boundary() {
        let (url, task) = start_quote_front(quote_service(), Arc::new(AtomicBool::new(true))).await;
        let client = reqwest::Client::new();
        for method in [METHOD, DEBUG_METHOD] {
            for length in [65_535, 65_536, 65_537] {
                let mut input = transaction();
                input["data"] = json!(format!("0x{}", "ff".repeat(length)));
                let response: Value = client
                    .post(&url)
                    .json(&json!({
                        "jsonrpc":"2.0", "id":"boundary", "method":method, "params":[input]
                    }))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(response["id"], "boundary");
                if length <= 65_536 {
                    assert_eq!(response["result"], json!({"quote":"shared"}), "{response}");
                } else {
                    assert_eq!(response["error"]["code"], -32602, "{response}");
                    assert_eq!(response["error"]["data"]["code"], "invalid_transaction");
                    assert_eq!(
                        response["error"]["data"]["message"],
                        "invalid transaction: calldata must be hex and at most 64 KiB"
                    );
                }
            }
        }
        task.abort();
    }

    #[tokio::test]
    async fn compact_quotes_enforce_160_kib_serialized_input_boundary() {
        let service = quote_service();
        for debug in [false, true] {
            // At the serialized-input limit the strict field parser still runs;
            // one extra byte must be rejected by the resource bound first.
            for length in [160 * 1024 - 1, 160 * 1024, 160 * 1024 + 1] {
                let mut input = transaction();
                input["padding"] = json!("");
                let base = input.to_string().len() + json!({}).to_string().len();
                input["padding"] = json!("x".repeat(length - base));
                let (status, response) = service.compact(&input, &json!({}), debug).await;
                assert_eq!(
                    status,
                    if length <= 160 * 1024 {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::PAYLOAD_TOO_LARGE
                    },
                    "{response}"
                );
                assert_eq!(
                    response["error"]["code"],
                    if length <= 160 * 1024 {
                        "invalid_transaction"
                    } else {
                        "quote_limit"
                    },
                    "{response}"
                );
            }
        }
    }

    #[tokio::test]
    async fn rpc_front_preserves_ids_errors_readiness_and_notifications() {
        let service = quote_service();
        let ready = Arc::new(AtomicBool::new(false));
        let (url, task) = start_quote_front(Arc::clone(&service), Arc::clone(&ready)).await;
        let client = reqwest::Client::new();
        let call = json!({"jsonrpc":"2.0","id":-2,"method":METHOD,"params":[transaction()]});
        let response: Value = client
            .post(&url)
            .json(&call)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["id"], -2);
        assert_eq!(response["error"]["code"], -32004);
        assert_eq!(response["error"]["data"]["code"], "starting_up");
        ready.store(true, Ordering::Release);
        for id in [Value::Null, json!(-2), json!("quote")] {
            let mut request = call.clone();
            request["id"] = id.clone();
            let response: Value = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                response,
                json!({"jsonrpc":"2.0","id":id,"result":{"quote":"shared"}})
            );
        }
        let expected = json!({"quote":"shared"});
        let batch: Value = client
            .post(&url)
            .json(&json!([
                call,
                {"jsonrpc":"2.0", "id":"info", "method":"eez_composerInfo"}
            ]))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(batch[0]["id"], -2);
        assert_eq!(batch[0]["result"], expected);
        assert_eq!(batch[1]["id"], "info");
        let mut oversized = transaction();
        oversized["data"] = json!("x".repeat(MAX_BODY));
        let response: Value = client
            .post(&url)
            .json(&json!({
                "jsonrpc":"2.0", "id":1, "method":METHOD, "params":[oversized]
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["error"]["code"], -32004);
        assert_eq!(response["error"]["data"]["code"], "quote_limit");
        for params in [
            Value::Null,
            json!([]),
            json!([transaction(), transaction()]),
            json!([{}]),
        ] {
            let mut request = call.clone();
            request["params"] = params;
            let response: Value = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(response["error"]["code"], -32602, "{response}");
        }
        for method in [METHOD, DEBUG_METHOD] {
            for params in [
                json!([{"from":transaction()["from"],"to":transaction()["to"]}]),
                json!([transaction(), {"transactionTypes":["0x2"]}]),
            ] {
                let response: Value = client
                    .post(&url)
                    .json(&json!({
                        "jsonrpc":"2.0", "id":"compact", "method":method,"params":params
                    }))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(response["result"], expected, "{response}");
            }
            let response: Value = client.post(&url).json(&json!({
                "jsonrpc":"2.0", "id":2, "method":method, "params":[transaction(), {"transactionTypes":["0x0"]}]
            })).send().await.unwrap().json().await.unwrap();
            assert_eq!(response["error"]["code"], -32602);
        }
        // Both diagnostic and compact requests share the quote capacity.
        let permit = service.quotes.capacity.acquire().await.unwrap();
        let response: Value = client
            .post(&url)
            .json(&call)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "quote_busy");
        drop(permit);
        let mut notification = call.clone();
        notification.as_object_mut().unwrap().remove("id");
        assert_eq!(
            client
                .post(&url)
                .json(&notification)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        task.abort();
    }

    async fn run_test_request(service: &FeeQuotes, compact: bool) -> (StatusCode, Value) {
        if compact {
            service.compact(&transaction(), &json!({}), false).await
        } else {
            service
                .run(Request::Signed(Box::new(
                    L2FeeRequest::parse(&transaction()).unwrap(),
                )))
                .await
        }
    }

    fn source_snapshot_error() -> FeeQuoteError {
        FeeQuoteError::SourceSnapshot {
            sampled: eez_l1::L1BlockRef {
                number: 39,
                hash: alloy_primitives::B256::repeat_byte(39),
            },
            current: eez_l1::L1BlockRef {
                number: 40,
                hash: alloy_primitives::B256::repeat_byte(40),
            },
            sampled_nonce: 9,
            current_nonce: 10,
            before_execution: true,
        }
    }

    #[tokio::test]
    async fn snapshot_refresh_is_bounded_and_does_not_retry_real_nonce_gaps() {
        use std::sync::atomic::AtomicUsize;
        for compact in [true, false] {
            for failures in [0, 1, 2, 9] {
                let preparations = Arc::new(AtomicUsize::new(0));
                let counter = Arc::clone(&preparations);
                let service = FeeQuotes {
                    enforce_tariff: true,
                    prepare: Arc::new(move |_| {
                        let attempt = counter.fetch_add(1, Ordering::Relaxed);
                        Box::pin(async move {
                            Ok(Box::new(move |_| {
                                if failures == 0 {
                                    Err(FeeQuoteError::Invalid("queued nonce prefix has a gap"))
                                } else if attempt < failures {
                                    let mut error = source_snapshot_error();
                                    if failures == 2 {
                                        // State changed after execution; do not
                                        // reset its frame budget in this request.
                                        if let FeeQuoteError::SourceSnapshot {
                                            before_execution,
                                            ..
                                        } = &mut error
                                        {
                                            *before_execution = false;
                                        }
                                    }
                                    Err(error)
                                } else {
                                    Ok(json!({"nonce":"0xa"}))
                                }
                            }) as Work)
                        })
                    }),
                    executor: TaskExecutor::test(),
                    quotes: Workers::new(1, 0),
                    admission: Workers::new(1, 0),
                    timeout: Duration::from_secs(10),
                };
                let (status, body) = run_test_request(&service, compact).await;
                let expected = match failures {
                    0 => {
                        assert_eq!(body["error"]["code"], "invalid_transaction");
                        StatusCode::BAD_REQUEST
                    }
                    1 => {
                        assert_eq!(body["nonce"], "0xa");
                        StatusCode::OK
                    }
                    _ => {
                        assert_eq!(body["error"]["details"]["currentNonce"], "0xa");
                        StatusCode::CONFLICT
                    }
                };
                assert_eq!(status, expected);
                assert_eq!(
                    preparations.load(Ordering::Relaxed),
                    match failures {
                        1 => 2,
                        9 => 9,
                        _ => 1,
                    }
                );
                assert_eq!(service.quotes.active.available_permits(), 1);
                assert_eq!(service.admission.active.available_permits(), 1);
            }
        }
    }

    #[tokio::test]
    async fn bounded_queue_waits_without_blocking_signed_admission() {
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let (finish, finished) = std::sync::mpsc::channel();
        let finished = std::sync::Mutex::new(Some(finished));
        let service = Arc::new(FeeQuotes {
            enforce_tariff: true,
            prepare: Arc::new(move |request| {
                let finish = if matches!(request, Request::Compact(..)) {
                    finished.lock().unwrap().take()
                } else {
                    None
                };
                let started = started.clone();
                Box::pin(async move {
                    Ok(Box::new(move |_| {
                        if let Some(finish) = finish {
                            started.send(()).unwrap();
                            finish.recv_timeout(Duration::from_secs(10)).unwrap();
                        }
                        Ok(json!({"ok":true}))
                    }) as Work)
                })
            }),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 1),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(5),
        });
        let first = Arc::clone(&service);
        let first = tokio::spawn(async move { run_test_request(&first, true).await });
        starts.recv().await.unwrap();
        let second = Arc::clone(&service);
        let second =
            tokio::spawn(async move { second.compact(&transaction(), &json!({}), true).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.quotes.capacity.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!second.is_finished(), "request waits for its worker");
        assert_eq!(
            run_test_request(&service, true).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(run_test_request(&service, false).await.0, StatusCode::OK);
        finish.send(()).unwrap();
        assert_eq!(first.await.unwrap().0, StatusCode::OK);
        assert_eq!(second.await.unwrap().0, StatusCode::OK);
        assert_eq!(service.quotes.capacity.available_permits(), 2);
    }

    #[tokio::test]
    async fn cancelling_a_waiting_request_releases_capacity_without_starting_work() {
        let service = Arc::new(FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(|_| panic!("cancelled queued work must not start")),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 1),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(5),
        });
        let active = service.quotes.active.acquire().await.unwrap();
        let cloned = Arc::clone(&service);
        let waiting = tokio::spawn(async move { run_test_request(&cloned, true).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.quotes.capacity.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert_eq!(service.quotes.capacity.available_permits(), 2);
        drop(active);
    }

    // The worker is held by a channel, not timing: an aborted HTTP task must
    // not make its slot available until this worker is explicitly released.
    #[tokio::test]
    async fn cancelled_or_timed_out_request_keeps_worker_permit_until_exit() {
        for (timeout, compact, refresh) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
            (true, false, true),
            (false, true, true),
            (true, true, true),
        ] {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (finish_tx, finish_rx) = std::sync::mpsc::channel();
            let channels = std::sync::Mutex::new(Some((started_tx, finish_rx)));
            let refresh_pending = AtomicBool::new(refresh);
            let service = Arc::new(FeeQuotes {
                enforce_tariff: false,
                prepare: Arc::new(move |_| {
                    if refresh_pending.swap(false, Ordering::Relaxed) {
                        return Box::pin(async {
                            Ok(Box::new(|_| Err(source_snapshot_error())) as Work)
                        });
                    }
                    let (started, finish) = channels.lock().unwrap().take().unwrap();
                    Box::pin(async move {
                        Ok(Box::new(move |cancelled: Arc<AtomicBool>| {
                            started.send(Arc::clone(&cancelled)).unwrap();
                            finish.recv_timeout(Duration::from_secs(10)).unwrap();
                            assert!(cancelled.load(Ordering::Relaxed));
                            Ok(json!({"finished":true}))
                        }) as Work)
                    })
                }),
                executor: TaskExecutor::test(),
                quotes: Workers::new(1, 0),
                admission: Workers::new(1, 0),
                timeout: Duration::from_millis(if timeout {
                    if refresh { 500 } else { 200 }
                } else {
                    30_000
                }),
            });
            let workers = if compact {
                &service.quotes
            } else {
                &service.admission
            };
            let cloned = Arc::clone(&service);
            let request = tokio::spawn(async move { run_test_request(&cloned, compact).await });
            let cancelled = started_rx.await.unwrap();
            assert_eq!(
                run_test_request(&service, compact).await.0,
                StatusCode::TOO_MANY_REQUESTS
            );
            if timeout {
                assert_eq!(request.await.unwrap().0, StatusCode::GATEWAY_TIMEOUT);
            } else {
                request.abort();
                assert!(request.await.unwrap_err().is_cancelled());
            }
            assert!(cancelled.load(Ordering::Relaxed));
            assert_eq!(
                run_test_request(&service, compact).await.0,
                StatusCode::TOO_MANY_REQUESTS
            );
            finish_tx.send(()).unwrap();
            let permit = tokio::time::timeout(Duration::from_secs(5), workers.active.acquire())
                .await
                .unwrap()
                .unwrap();
            drop(permit);
            assert_eq!(workers.active.available_permits(), 1);
            assert_eq!(workers.capacity.available_permits(), 1);
        }
    }

    #[tokio::test]
    async fn timeout_cancels_preparation_and_invalid_input_never_starts_it() {
        let service = FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(|_| Box::pin(std::future::pending())),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::ZERO,
        };
        assert_eq!(
            service.compact(&json!({}), &json!({}), false).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            service.compact(&transaction(), &json!({}), false).await.0,
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(service.quotes.active.available_permits(), 1);
    }
}
