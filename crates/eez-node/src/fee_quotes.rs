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
pub(crate) const MAX_BODY: usize = 32 * 1024;
type Work = Box<dyn FnOnce(Arc<AtomicBool>) -> Result<Value, FeeQuoteError> + Send>;
type Preparation = Pin<Box<dyn Future<Output = Result<Work, FeeQuoteError>> + Send>>;

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
        let enforce_tariff = composer.enforces_l2_fee_tariff();
        Arc::new(Self {
            enforce_tariff,
            prepare: Arc::new(move |request| {
                let composer = composer.clone();
                Box::pin(async move {
                    match request {
                        Request::Signed(request) => {
                            let prepared =
                                composer.prepare_l2_fee_quote(rollup_id, *request).await?;
                            Ok(
                                Box::new(move |cancelled| {
                                    composer.quote_l2_fee(prepared, cancelled)
                                }) as Work,
                            )
                        }
                        Request::Compact(request, debug) => {
                            let prepared = composer
                                .prepare_compact_fee_quote(rollup_id, request)
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
        if transaction.to_string().len() + options.to_string().len() > MAX_BODY {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "quote_limit",
                "quote exceeds 32 KiB",
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
            let work = (self.prepare)(request).await?;
            self.executor
                .spawn_blocking(move || {
                    // Dropping the JoinHandle does not stop synchronous EVM work.
                    // Keep the permit inside that work until all its frames exit.
                    let _permits = (slot, permit);
                    work(cancelled)
                })
                .await
                .map_err(|_| FeeQuoteError::Unavailable("quote worker failed"))?
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
        FeeQuoteError::Unsupported(_) | FeeQuoteError::Limit(_) | FeeQuoteError::Underfunded => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        FeeQuoteError::Stale => StatusCode::CONFLICT,
        FeeQuoteError::Disabled => StatusCode::NOT_FOUND,
        FeeQuoteError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    error(status, err.code(), &err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    fn transaction() -> Value {
        json!({"type":"0x2","chainId":"0x1","from":Address::repeat_byte(1),"to":Address::repeat_byte(2),"gas":"0x186a0","nonce":"0x0","value":"0x0","data":"0x"})
    }

    #[tokio::test]
    async fn rpc_front_preserves_ids_errors_readiness_and_notifications() {
        use alloy_provider::{ProviderBuilder, mock::Asserter};
        use eez_composer::{Direction, HeldPool};
        let service = Arc::new(FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(|_| {
                Box::pin(async { Ok(Box::new(|_| Ok(json!({"quote":"shared"}))) as Work) })
            }),
            executor: TaskExecutor::test(),
            quotes: Workers::new(1, 0),
            admission: Workers::new(1, 0),
            timeout: Duration::from_secs(10),
        });
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let ready = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(crate::ingress::run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Outbound,
            Arc::new(HeldPool::new(Default::default())),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            1,
            Address::ZERO,
            Arc::clone(&ready),
            Arc::new(json!({})),
            Some(Arc::clone(&service)),
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
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}");
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
        for (timeout, compact) in [(false, false), (true, false), (false, true), (true, true)] {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (finish_tx, finish_rx) = std::sync::mpsc::channel();
            let channels = std::sync::Mutex::new(Some((started_tx, finish_rx)));
            let service = Arc::new(FeeQuotes {
                enforce_tariff: false,
                prepare: Arc::new(move |_| {
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
                timeout: Duration::from_millis(if timeout { 200 } else { 30_000 }),
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
