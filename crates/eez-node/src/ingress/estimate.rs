//! Wallet estimates share one worker budget across both source-chain fronts.
use super::*;
use alloy_rpc_types_eth::TransactionRequest;
use eez_composer::composer::estimate::{EstimateError, GasEstimator};
use reth_tasks::TaskExecutor;
use std::time::Instant;
use tokio::sync::Semaphore;

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct EstimationService {
    estimator: Arc<GasEstimator>,
    executor: TaskExecutor,
    permits: Arc<Semaphore>,
}

impl EstimationService {
    pub(crate) fn new(estimator: GasEstimator, executor: TaskExecutor) -> Self {
        Self {
            estimator: Arc::new(estimator),
            executor,
            permits: Arc::new(Semaphore::new(2)),
        }
    }
}

async fn run_worker<T: Send + 'static>(
    executor: &TaskExecutor,
    permit: tokio::sync::OwnedSemaphorePermit,
    deadline: Instant,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<(tokio::sync::OwnedSemaphorePermit, T), EstimateError> {
    let worker = executor.spawn_blocking(move || {
        let result = work();
        (permit, result)
    });
    tokio::time::timeout_at(deadline.into(), worker)
        .await
        .map_err(|_| EstimateError::Timeout)?
        .map_err(|_| EstimateError::Unavailable("estimation worker stopped".into()))
}

pub(super) fn contains_estimate(request: &Value) -> bool {
    match request {
        Value::Array(items) => items.iter().any(contains_estimate),
        _ => request.get("method").and_then(Value::as_str) == Some("eth_estimateGas"),
    }
}

fn error(id: Value, error: EstimateError) -> Value {
    let code = match &error {
        EstimateError::Invalid(_) => -32602,
        EstimateError::Revert(_) => 3,
        _ => -32000,
    };
    let mut response = serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":error.to_string()}});
    if let EstimateError::Revert(data) = error {
        response["error"]["data"] = serde_json::json!(data);
    }
    response
}

async fn estimate(ctx: &Ctx, request: &Value) -> Result<Option<u64>, EstimateError> {
    let params = request
        .get("params")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty() && p.len() <= 3)
        .ok_or_else(|| {
            EstimateError::Invalid("expected transaction and optional block/state overrides".into())
        })?;
    let tx: TransactionRequest = serde_json::from_value(params[0].clone())
        .map_err(|e| EstimateError::Invalid(e.to_string()))?;
    if tx.to.is_none_or(|to| to.is_create()) {
        return Ok(None);
    }
    let Some(service) = &ctx.estimator else {
        return Ok(None);
    };
    let permit =
        service.permits.clone().try_acquire_owned().map_err(|_| {
            EstimateError::Unavailable("estimation workers busy; retry shortly".into())
        })?;
    let deadline = Instant::now() + DEADLINE;
    let block = params
        .get(1)
        .filter(|block| !block.is_null())
        .cloned()
        .unwrap_or(serde_json::json!("pending"));
    let overrides = params.get(2).cloned().unwrap_or(Value::Null);
    let number = match block.as_str() {
        Some("latest" | "pending") => None,
        Some("earliest") => Some(0),
        Some(value) if value.starts_with("0x") => Some(
            u64::from_str_radix(&value[2..], 16)
                .map_err(|e| EstimateError::Invalid(e.to_string()))?,
        ),
        _ => {
            return Err(EstimateError::Unavailable(
                "classification requires latest, pending or an explicit block number".into(),
            ));
        }
    };
    let state_overrides = if overrides.is_null() {
        None
    } else {
        Some(
            serde_json::from_value(overrides.clone())
                .map_err(|e| EstimateError::Invalid(e.to_string()))?,
        )
    };
    let estimator = service.estimator.clone();
    let direction = ctx.direction;
    let classify_tx = tx.clone();
    let (permit, classification) = run_worker(&service.executor, permit, deadline, move || {
        estimator.classify(direction, classify_tx, number, state_overrides)
    })
    .await?;
    let cross_chain = classification?;
    if !cross_chain {
        return Ok(None);
    }
    if !ctx.ready.load(std::sync::atomic::Ordering::Acquire) {
        return Err(EstimateError::Unavailable(STARTING_UP_MSG.into()));
    }
    if !matches!(block.as_str(), Some("pending" | "latest")) || !overrides.is_null() {
        return Err(EstimateError::Unsupported(
            "use latest/pending without state overrides for cross-chain estimates".into(),
        ));
    }
    let estimator = service.estimator.clone();
    let direction = ctx.direction;
    // Dropping the response future cannot release this permit while execution
    // continues. Reth owns the blocking task through shutdown.
    let (_permit, result) = run_worker(&service.executor, permit, deadline, move || {
        estimator.estimate(direction, tx, deadline)
    })
    .await?;
    result.map(Some)
}

pub(super) async fn item(ctx: &Ctx, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let local = if contains_estimate(request) {
        match estimate(ctx, request).await {
            Ok(Some(gas)) => {
                Some(serde_json::json!({"jsonrpc":"2.0","id":id,"result":format!("0x{gas:x}")}))
            }
            Ok(None) => None,
            Err(e) => Some(error(id.clone(), e)),
        }
    } else {
        None
    };
    let forwarded = local.is_none();
    let mut response = match local {
        Some(response) => response,
        None => match ctx
            .client
            .post(&ctx.upstream_rpc_url)
            .json(request)
            .send()
            .await
        {
            Ok(response) if request.get("id").is_none() => {
                let _ = response;
                return None;
            }
            Ok(response) => response.json().await.unwrap_or_else(|_| {
                error(
                    id.clone(),
                    EstimateError::Unavailable("invalid upstream response".into()),
                )
            }),
            Err(_) => error(
                id,
                EstimateError::Unavailable("upstream transport failed".into()),
            ),
        },
    };
    // Recheck the upstream candidate: gas-sensitive code may reach dispatch
    // only at the smaller limit selected by the ordinary node.
    if forwarded
        && contains_estimate(request)
        && let Some(gas) = response.get("result").and_then(Value::as_str)
    {
        let mut candidate = request.clone();
        candidate["params"][0]["gas"] = serde_json::json!(gas);
        match estimate(ctx, &candidate).await {
            Ok(Some(gas)) => {
                response = serde_json::json!({"jsonrpc":"2.0","id":request.get("id"),"result":format!("0x{gas:x}")})
            }
            Ok(None) => {}
            Err(e) => response = error(request.get("id").cloned().unwrap_or(Value::Null), e),
        }
    }
    request.get("id").map(|_| response)
}

pub(super) async fn handle(ctx: &Ctx, request: &Value) -> Response<Full<HyperBytes>> {
    let response = if let Value::Array(items) = request {
        let mut responses = Vec::new();
        for request in items {
            if let Some(response) = item(ctx, request).await {
                responses.push(response);
            }
        }
        (!responses.is_empty()).then_some(Value::Array(responses))
    } else {
        item(ctx, request).await
    };
    match response {
        Some(response) => json_response(response.to_string()),
        None => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("Access-Control-Allow-Origin", "*")
            .body(Full::new(HyperBytes::new()))
            .expect("valid response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timeout_keeps_worker_permit_until_execution_exits() {
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().try_acquire_owned().unwrap();
        let (started, start) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let work = tokio::spawn(async move {
            run_worker(
                &TaskExecutor::test(),
                permit,
                Instant::now() + Duration::from_millis(100),
                move || {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                },
            )
            .await
        });
        start.await.unwrap();
        assert!(matches!(work.await.unwrap(), Err(EstimateError::Timeout)));
        assert!(permits.clone().try_acquire_owned().is_err());
        release.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    #[test]
    fn revert_payload_and_request_identity_survive_mapping() {
        let id = serde_json::json!("wallet:42");
        let response = error(
            id.clone(),
            EstimateError::Revert(Bytes::from_static(&[0xde, 0xad])),
        );
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], 3);
        assert_eq!(response["error"]["data"], "0xdead");
    }
}
