//! Application HTTP quotes. One worker, no waiting queue; its permit remains
//! owned by blocking execution after an HTTP timeout or caller cancellation.

use alloy_consensus::Header;
use eez_composer::{
    Composer,
    composer::{FeeQuoteError, L2FeeRequest},
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

pub(crate) const PATH: &str = "/eez/fees/l2-to-l1";
pub(crate) const MAX_BODY: usize = 32 * 1024;
type Work = Box<dyn FnOnce(Arc<AtomicBool>) -> Result<Value, FeeQuoteError> + Send>;
type Preparation = Pin<Box<dyn Future<Output = Result<Work, FeeQuoteError>> + Send>>;

pub(crate) struct FeeQuotes {
    prepare: Arc<dyn Fn(L2FeeRequest) -> Preparation + Send + Sync>,
    executor: TaskExecutor,
    permits: Arc<Semaphore>,
    timeout: Duration,
    pub(crate) enforce_tariff: bool,
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
                    let prepared = composer.prepare_l2_fee_quote(rollup_id, request).await?;
                    Ok(
                        Box::new(move |cancelled| composer.quote_l2_fee(prepared, cancelled))
                            as Work,
                    )
                })
            }),
            executor,
            permits: Arc::new(Semaphore::new(1)),
            timeout: Duration::from_secs(10),
        })
    }

    pub(crate) async fn quote(&self, input: &Value) -> (StatusCode, Value) {
        let request = match L2FeeRequest::parse(input) {
            Ok(request) => request,
            Err(error) => return failure(error),
        };
        self.run(request).await
    }

    pub(crate) async fn check_signed(
        &self,
        envelope: &alloy_consensus::TxEnvelope,
    ) -> (StatusCode, Value) {
        match L2FeeRequest::from_signed(envelope) {
            Ok(request) => self.run(request).await,
            Err(error) => failure(error),
        }
    }

    async fn run(&self, request: L2FeeRequest) -> (StatusCode, Value) {
        let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "quote_busy",
                "one quote is already running",
            );
        };
        let cancellation = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let cancelled = Arc::clone(&cancellation.0);
        let operation = async {
            let work = (self.prepare)(request).await?;
            self.executor
                .spawn_blocking(move || {
                    // Dropping the JoinHandle does not stop synchronous EVM work.
                    // Keep the permit inside that work until all its frames exit.
                    let _permit = permit;
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

    fn intent() -> Value {
        json!({"type":"0x2","chainId":"0x1","from":Address::repeat_byte(1),"to":Address::repeat_byte(2),"gas":"0x186a0","nonce":"0x0","value":"0x0","data":"0x"})
    }

    // The worker is held by a channel, not timing: an aborted HTTP task must
    // not make its slot available until this worker is explicitly released.
    #[tokio::test]
    async fn cancelled_or_timed_out_request_keeps_worker_permit_until_exit() {
        for timeout in [false, true] {
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
                permits: Arc::new(Semaphore::new(1)),
                timeout: Duration::from_millis(if timeout { 200 } else { 30_000 }),
            });
            let cloned = Arc::clone(&service);
            let request = tokio::spawn(async move { cloned.quote(&intent()).await });
            let cancelled = started_rx.await.unwrap();
            assert_eq!(
                service.quote(&intent()).await.0,
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
                service.quote(&intent()).await.0,
                StatusCode::TOO_MANY_REQUESTS
            );
            finish_tx.send(()).unwrap();
            let permit = tokio::time::timeout(Duration::from_secs(5), service.permits.acquire())
                .await
                .unwrap()
                .unwrap();
            drop(permit);
            assert_eq!(service.permits.available_permits(), 1);
        }
    }

    #[tokio::test]
    async fn timeout_cancels_preparation_and_invalid_input_never_starts_it() {
        let service = FeeQuotes {
            enforce_tariff: false,
            prepare: Arc::new(|_| Box::pin(std::future::pending())),
            executor: TaskExecutor::test(),
            permits: Arc::new(Semaphore::new(1)),
            timeout: Duration::ZERO,
        };
        assert_eq!(service.quote(&json!({})).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            service.quote(&intent()).await.0,
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(service.permits.available_permits(), 1);
    }
}
