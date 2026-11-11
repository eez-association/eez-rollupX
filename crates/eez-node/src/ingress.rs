//! Transparent cross-chain RPC fronts — one per SOURCE chain.
//!
//! A cross-chain tx is signed and tracked against its source chain, so wallets
//! need a real RPC for that chain. Each front is a transparent proxy: it
//! forwards ordinary `eth_*` requests to the source chain's node, prepares
//! cross-chain `eth_estimateGas` locally, and classifies
//! `eth_sendRawTransaction` with a read-only execution trace. Transactions that
//! reach the chain's EEZ dispatcher are held in the per-rollup [`HeldPool`] with
//! the front's fixed [`Direction`] (L1 front → `Inbound`, L2 front → `Outbound`);
//! ordinary transactions are forwarded to the source-chain mempool unchanged.
//! The receipt resolves when the held tx lands — an outbound in the L2 Sync
//! block, an inbound in the L1 postBatch bundle.
//! Composer-only `eez_*` observations and the virtual discovery `eth_call`
//! are answered locally; discovery does not assert submission readiness.
//!
//! Classification fails closed: if tracing is temporarily unavailable, the tx
//! is held rather than risking a cross-chain transaction bypassing composition.
//! One front per source chain (invariant 8).
//!
//! A creation has no target and is forwarded directly to its source chain. A
//! codeless target that reaches cross-chain admission is rejected with why.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy_consensus::transaction::SignerRecoverable as _;
use alloy_consensus::{Transaction as _, TxEnvelope};
use alloy_eips::eip2718::Decodable2718 as _;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_provider::{Provider as _, RootProvider};
use eez_composer::{AdmissionError, Direction, HeldPool, HeldTx};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes as HyperBytes;
use hyper::header::CONTENT_LENGTH;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpListener;
use tracing::{Level, event};

mod estimate;
pub(crate) use estimate::EstimationService;

const MAX_BODY: usize = 10 * 1024 * 1024;

/// Outcome of the admission gate.
#[derive(Debug)]
pub enum Admission {
    /// Accepted into the HeldPool; carries the canonical tx hash.
    Held(B256),
    /// Rejected at the door (loud — invariant 7); carries the reason.
    Rejected(String),
}

/// Checks the target's code, the nonce and the balance on the SOURCE chain
/// (`validation_provider`), then holds the tx under `direction`.
///
/// A new nonce must be the first unreserved nonce at or above the source-chain
/// nonce for this direction. An existing queued nonce may be replaced in
/// place. A gapped held tx would poison the all-or-nothing bundle and break
/// bundle-mates' chains.
pub async fn gate_and_hold(
    envelope: &TxEnvelope,
    raw_tx: &Bytes,
    direction: Direction,
    held_pool: &HeldPool,
    expected_source_chain_id: u64,
    validation_provider: &RootProvider,
) -> Admission {
    if let Err(msg) = validate_front_envelope(envelope, direction, expected_source_chain_id) {
        return Admission::Rejected(msg);
    }
    let cost = match upfront_cost(envelope) {
        Ok(cost) => cost,
        Err(msg) => return Admission::Rejected(msg),
    };
    let Ok(sender) = envelope.recover_signer() else {
        return Admission::Rejected("signature recovery failed".into());
    };
    // The front forwards creations, so HTTP never gets here. This guards direct
    // callers of a public fn.
    let Some(to) = envelope.to() else {
        return Admission::Rejected(
            "creations are not cross-chain: a contract creation has no target; submit deploys to the chain's own RPC".into(),
        );
    };
    if let Err(msg) = check_block_gas_limit(envelope, validation_provider).await {
        return Admission::Rejected(msg);
    }
    // No code at `to` means no manager frame can ever run, so holding this tx
    // would only end in an evict.
    match validation_provider.get_code_at(to).await {
        Ok(code) if code.is_empty() => {
            return Admission::Rejected(format!(
                "target {to} has no code on the source chain — a cross-chain call must target a deployed contract or proxy; create the proxy first (createCrossChainProxy on the manager) or use the bridge, then resubmit"
            ));
        }
        Ok(_) => {}
        Err(e) => {
            return Admission::Rejected(format!(
                "source-chain code lookup failed for target {to}: {e}"
            ));
        }
    }
    let nonce = envelope.nonce();
    // This checks one transaction; simulation enforces cumulative affordability.
    let balance = match validation_provider.get_balance(sender).await {
        Ok(b) => b,
        Err(e) => {
            return Admission::Rejected(format!("source-chain balance lookup failed: {e}"));
        }
    };
    if balance < cost {
        return Admission::Rejected(format!(
            "insufficient balance for {sender}: have {balance}, need {cost} (value + gas_limit * max_fee)"
        ));
    }
    // keccak256 of the EIP-2718 envelope — the canonical tx hash for legacy and typed txs alike.
    let hash: B256 = keccak256(raw_tx.as_ref());
    let tx = HeldTx {
        raw_tx: raw_tx.clone(),
        hash,
        attempts: 0,
        excluded_slots: 0,
        max_fee_per_gas: envelope.max_fee_per_gas(),
        // Legacy/2930 have no tip field; gas_price is both cap and tip.
        priority_fee_per_gas: envelope
            .max_priority_fee_per_gas()
            .unwrap_or_else(|| envelope.max_fee_per_gas()),
        sender,
        nonce,
        direction,
    };
    // Pending state prevents collisions with source-chain mempool transactions.
    let on_chain = match validation_provider
        .get_transaction_count(sender)
        .pending()
        .await
    {
        Ok(n) => n,
        Err(e) => return Admission::Rejected(format!("source-chain nonce lookup failed: {e}")),
    };
    if let Err(err) = held_pool.push_contiguous(tx, on_chain) {
        return Admission::Rejected(match err {
            AdmissionError::ReconciliationPending => format!(
                "source ownership for {hash} awaits canonical reconciliation; retry after settlement reconciliation"
            ),
            AdmissionError::Nonce { expected, on_chain } => format!(
                "invalid nonce {nonce} for {sender}: expected next unreserved nonce {expected} (source-chain nonce {on_chain})"
            ),
            AdmissionError::UnderpricedReplacement {
                offered_max_fee,
                required_max_fee,
                offered_priority_fee,
                required_priority_fee,
            } => format!(
                "replacement underpriced for {sender} nonce {nonce}: max_fee_per_gas {offered_max_fee} (need {required_max_fee}), priority fee {offered_priority_fee} (need {required_priority_fee})"
            ),
        });
    }
    event!(
        name: "eez.ingress.cross_chain.push",
        Level::INFO,
        tx_hash = %hash,
        sender = %sender,
        nonce,
        direction = ?direction,
        "cross-chain tx held for next Sync slot",
    );
    Admission::Held(hash)
}

/// Admission policy: reject envelopes cross-chain fronts won't hold.
fn validate_front_envelope(
    envelope: &TxEnvelope,
    direction: Direction,
    expected_source_chain_id: u64,
) -> Result<(), String> {
    // Allowlist: only types the pipeline is known to handle. Notably out:
    // EIP-4844 (blobs) and EIP-7702 (authorizations bump OTHER accounts'
    // nonces, breaking the pool's contiguity model).
    match envelope {
        TxEnvelope::Legacy(_) | TxEnvelope::Eip2930(_) | TxEnvelope::Eip1559(_) => {}
        other => {
            return Err(format!(
                "unsupported transaction type {:?}: cross-chain fronts accept legacy (EIP-155), EIP-2930, and EIP-1559 only",
                other.tx_type()
            ));
        }
    }
    let Some(chain_id) = envelope.chain_id() else {
        return Err(
            "missing source-chain id; replay-unprotected legacy transactions are not accepted by cross-chain fronts"
                .into(),
        );
    };
    if chain_id != expected_source_chain_id {
        return Err(format!(
            "wrong source-chain id {chain_id} for {direction:?} front: expected {expected_source_chain_id}"
        ));
    }
    // The checks below reject transactions no block can ever include, which
    // would otherwise be held and then evicted without a client-visible reason.
    // A fee cap below the base fee is not among them: it can become valid later.
    if let Some(priority_fee) = envelope.max_priority_fee_per_gas()
        && priority_fee > envelope.max_fee_per_gas()
    {
        return Err(format!(
            "max_priority_fee_per_gas {priority_fee} exceeds max_fee_per_gas {}",
            envelope.max_fee_per_gas()
        ));
    }
    if envelope.gas_limit() > TX_GAS_LIMIT_CAP {
        return Err(format!(
            "gas limit {} exceeds the per-transaction cap {TX_GAS_LIMIT_CAP} (EIP-7825)",
            envelope.gas_limit()
        ));
    }
    let intrinsic = intrinsic_gas(envelope);
    if envelope.gas_limit() < intrinsic {
        return Err(format!(
            "gas limit {} is below the transaction's intrinsic gas {intrinsic}",
            envelope.gas_limit()
        ));
    }
    Ok(())
}

/// Osaka's per-transaction gas cap (EIP-7825). The L2 runs Osaka from genesis;
/// both L1 fronts apply the same cap by policy.
const TX_GAS_LIMIT_CAP: u64 = 1 << 24;

/// Minimum gas any inclusion charges: the base cost, calldata and access list,
/// or the EIP-7623 calldata floor when that is higher. Both source chains run
/// at least Prague rules.
fn intrinsic_gas(envelope: &TxEnvelope) -> u64 {
    let (accounts, slots) = envelope.access_list().map_or((0, 0), |list| {
        list.iter().fold((0u64, 0u64), |(accounts, slots), item| {
            (accounts + 1, slots + item.storage_keys.len() as u64)
        })
    });
    let gas = revm::context_interface::cfg::gas::calculate_initial_tx_gas(
        revm::primitives::hardfork::SpecId::PRAGUE,
        envelope.input(),
        envelope.is_create(),
        accounts,
        slots,
        0,
        None,
    );
    (gas.initial_regular_gas + gas.initial_state_gas).max(gas.floor_gas)
}

/// Reject a gas limit above the source chain's current block gas limit. The L2
/// limit is fixed; an L1 limit can change, so this checks the latest block.
async fn check_block_gas_limit(
    envelope: &TxEnvelope,
    validation_provider: &RootProvider,
) -> Result<(), String> {
    let block = validation_provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
        .map_err(|e| format!("source-chain block lookup failed: {e}"))?
        .ok_or_else(|| "source-chain block lookup returned no block".to_string())?;
    let block_gas_limit = block.header.gas_limit;
    if envelope.gas_limit() > block_gas_limit {
        return Err(format!(
            "gas limit {} exceeds the source chain's block gas limit {block_gas_limit}",
            envelope.gas_limit()
        ));
    }
    Ok(())
}

/// Upfront cost the sender must cover: value + gas_limit * max_fee_per_gas.
fn upfront_cost(envelope: &TxEnvelope) -> Result<U256, String> {
    let gas_cost = U256::from(envelope.gas_limit())
        .checked_mul(U256::from(envelope.max_fee_per_gas()))
        .ok_or_else(|| "upfront cost overflow: gas_limit * max_fee_per_gas".to_string())?;
    envelope
        .value()
        .checked_add(gas_cost)
        .ok_or_else(|| "upfront cost overflow: value + gas_limit * max_fee_per_gas".to_string())
}

/// Refusal sent while the node is still reconciling with L1.
/// WIRE CONTRACT: the e2e helper and the shell harnesses grep "starting up".
pub const STARTING_UP_MSG: &str =
    "eez node is starting up; cross-chain submissions are not accepted yet";

/// Shared, cheaply-clonable per-connection context.
#[derive(Clone)]
struct Ctx {
    /// False until the deriver has reconciled with L1. Submissions are refused
    /// until then rather than queued into a pool nothing is draining yet.
    ready: Arc<std::sync::atomic::AtomicBool>,
    client: reqwest::Client,
    upstream_rpc_url: String,
    direction: Direction,
    held_pool: Arc<HeldPool>,
    validation_provider: RootProvider,
    expected_source_chain_id: u64,
    /// EEZ on L1 or EEZL2 on L2. A trace that calls one of its cross-chain
    /// entrypoints must be composed instead of entering the ordinary mempool.
    dispatch_address: Address,
    /// Serialized `eez_composerInfo` result, shared by both fronts.
    composer_info: Arc<Value>,
    /// Local Composer-only methods. Never installed on an upstream node RPC.
    composer_methods: jsonrpsee::RpcModule<()>,
    estimator: Option<EstimationService>,
    fee_quotes: Option<Arc<crate::fee_quotes::FeeQuotes>>,
}

/// Verify that a configured cross-chain front points at its expected source chain.
///
/// # Errors
///
/// Returns an error when the upstream chain ID cannot be read or does not match.
pub async fn validate_cross_chain_front(
    validation_provider: &RootProvider,
    expected_source_chain_id: u64,
) -> eyre::Result<()> {
    let upstream = validation_provider.get_chain_id().await.map_err(|e| {
        eyre::eyre!("cross-chain front: upstream chain id lookup failed at startup: {e}")
    })?;
    if upstream != expected_source_chain_id {
        return Err(eyre::eyre!(
            "cross-chain front misconfigured: upstream RPC reports chain id {upstream}, expected {expected_source_chain_id}"
        ));
    }
    Ok(())
}

/// Run a transparent cross-chain front on `port`, forwarding every `eth_*` to
/// `upstream_rpc_url` (the source chain's node) and holding intercepted
/// `eth_sendRawTransaction`s with `direction`. Never returns under normal op.
///
/// # Errors
/// Fails if the listener can't bind or the reqwest client can't be built.
#[cfg(test)]
pub(crate) async fn run_cross_chain_front(
    port: u16,
    upstream_rpc_url: String,
    direction: Direction,
    held_pool: Arc<HeldPool>,
    validation_provider: RootProvider,
    expected_source_chain_id: u64,
    dispatch_address: Address,
    ready: Arc<std::sync::atomic::AtomicBool>,
    composer_info: Arc<Value>,
    fee_quotes: Option<Arc<crate::fee_quotes::FeeQuotes>>,
) -> eyre::Result<()> {
    let mut composer_methods = jsonrpsee::RpcModule::new(());
    composer_methods.merge(crate::queue_rpc::module(
        Arc::clone(&held_pool),
        expected_source_chain_id,
        expected_source_chain_id,
    )?)?;
    run_cross_chain_front_with_estimator(
        port,
        upstream_rpc_url,
        direction,
        held_pool,
        validation_provider,
        expected_source_chain_id,
        dispatch_address,
        ready,
        composer_info,
        composer_methods,
        None,
        fee_quotes,
    )
    .await
}

pub(crate) async fn run_cross_chain_front_with_estimator(
    port: u16,
    upstream_rpc_url: String,
    direction: Direction,
    held_pool: Arc<HeldPool>,
    validation_provider: RootProvider,
    expected_source_chain_id: u64,
    dispatch_address: Address,
    ready: Arc<std::sync::atomic::AtomicBool>,
    composer_info: Arc<Value>,
    composer_methods: jsonrpsee::RpcModule<()>,
    estimator: Option<EstimationService>,
    fee_quotes: Option<Arc<crate::fee_quotes::FeeQuotes>>,
) -> eyre::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    event!(
        name: "eez.xchain_front.listening",
        Level::INFO,
        %port,
        %upstream_rpc_url,
        direction = ?direction,
        expected_source_chain_id,
        %dispatch_address,
        "cross-chain front listening (classify sendRawTransaction; hold cross-chain, forward ordinary)",
    );
    let ctx = Ctx {
        ready,
        client,
        upstream_rpc_url,
        direction,
        held_pool,
        validation_provider,
        expected_source_chain_id,
        dispatch_address,
        composer_info,
        composer_methods,
        estimator,
        fee_quotes,
    };
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                event!(name: "eez.xchain_front.accept_failed", Level::WARN, error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| handle(req, ctx.clone()));
            let io = TokioIo::new(stream);
            if let Err(e) = http1::Builder::new()
                .keep_alive(true)
                .serve_connection(io, service)
                .await
                && !e.is_incomplete_message()
            {
                event!(name: "eez.xchain_front.conn_error", Level::DEBUG, error = %e, "connection error");
            }
        });
    }
}

fn json_response(body: String) -> Response<Full<HyperBytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .body(Full::new(HyperBytes::from(body)))
        .expect("valid response")
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response<Full<HyperBytes>> {
    json_response(
        serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": code, "message": message },
            "id": id,
        })
        .to_string(),
    )
}

fn body_too_large() -> Response<Full<HyperBytes>> {
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .body(Full::new(HyperBytes::from("request body too large")))
        .expect("valid response")
}

async fn collect_body_limited(
    mut body: hyper::body::Incoming,
    max: usize,
) -> Result<Result<HyperBytes, ()>, hyper::Error> {
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if data.len() > max.saturating_sub(out.len()) {
            return Ok(Err(()));
        }
        out.extend_from_slice(&data);
    }
    Ok(Ok(HyperBytes::from(out)))
}

fn content_length_exceeds(req: &Request<hyper::body::Incoming>, max: usize) -> bool {
    req.headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|len| len > max)
}

/// Prefix, not equality: `eth_sendRawTransactionConditional` must not slip
/// past the readiness gate or the intercept just by having a longer name.
fn is_submission_method(method: &str) -> bool {
    method.starts_with("eth_sendRawTransaction")
}

/// The one submission method this front knows how to route cross-chain.
const SEND_RAW: &str = "eth_sendRawTransaction";

fn batch_has_send_raw(json: &Value) -> bool {
    matches!(json, Value::Array(items) if items.iter().any(|it| {
        it.get("method").and_then(Value::as_str).is_some_and(is_submission_method)
    }))
}

fn all_forwarded_methods_are_eth(json: &Value) -> bool {
    let method_allowed = |value: &Value| {
        value
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| method.starts_with("eth_"))
    };
    match json {
        Value::Object(_) => method_allowed(json),
        Value::Array(items) if !items.is_empty() => items.iter().all(|item| {
            method_allowed(item)
                && !item
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(is_submission_method)
        }),
        _ => false,
    }
}

fn contains_composer_method(request: &Value) -> bool {
    match request {
        Value::Array(items) => items.iter().any(contains_composer_method),
        _ => {
            request
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|method| method.starts_with("eez_"))
                || crate::composer_rpc::is_discovery_call(request)
        }
    }
}

async fn composer_item(ctx: &Ctx, request: &Value) -> Option<Value> {
    let error = |id: Value, code: i64, message: &str| serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}});
    // Invalid batch members are errors, not notifications. Explicit null IDs
    // are calls; only an absent ID on a valid envelope suppresses the response.
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str).is_none()
        || request
            .get("id")
            .is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
    {
        return Some(error(Value::Null, -32600, "invalid request"));
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request["method"].as_str().unwrap_or_default();
    let response = if method == crate::composer_rpc::METHOD {
        crate::composer_rpc::respond(request, &ctx.composer_info)
    } else if crate::composer_rpc::is_discovery_call(request) {
        crate::composer_rpc::respond_eth_call(
            request,
            &ctx.composer_info,
            ctx.expected_source_chain_id,
        )
    } else if matches!(
        method,
        crate::fee_quotes::METHOD | crate::fee_quotes::DEBUG_METHOD
    ) {
        match request.get("params").and_then(Value::as_array) {
            Some(params) if (1..=2).contains(&params.len()) => {
                let (status, value) = if !ctx.ready.load(std::sync::atomic::Ordering::Acquire) {
                    crate::fee_quotes::error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "starting_up",
                        STARTING_UP_MSG,
                    )
                } else if let Some(service) = &ctx.fee_quotes {
                    service
                        .compact(
                            &params[0],
                            params.get(1).unwrap_or(&serde_json::json!({})),
                            method == crate::fee_quotes::DEBUG_METHOD,
                        )
                        .await
                } else {
                    crate::fee_quotes::error(
                        StatusCode::NOT_FOUND,
                        "quotes_disabled",
                        "fee quotes are not configured",
                    )
                };
                crate::fee_quotes::rpc_response(id, status, value)
            }
            _ => error(
                id,
                -32602,
                "expected transaction and optional quote options",
            ),
        }
    } else if method.starts_with("eez_") {
        let mut call = request.clone();
        // RpcModule's numeric IDs are u64-only. Use a private ID for dispatch
        // and restore the client's JSON ID, including negative numbers/null.
        // Suppress notifications only after executing the handler.
        call["id"] = Value::from(0);
        match ctx
            .composer_methods
            .raw_json_request(&call.to_string(), 1)
            .await
        {
            Ok((raw, _)) => match serde_json::from_str::<Value>(raw.get()) {
                Ok(mut response) => {
                    response["id"] = id;
                    response
                }
                Err(_) => error(id, -32603, "invalid local RPC response"),
            },
            Err(_) => error(id, -32602, "invalid Composer RPC parameters"),
        }
    } else if method.starts_with("eth_") {
        return estimate::item(ctx, request).await;
    } else {
        error(
            id,
            -32601,
            "cross-chain front serves only eth_* and Composer eez_* methods",
        )
    };
    request.get("id").map(|_| response)
}

async fn handle_composer_methods(ctx: &Ctx, request: &Value) -> Response<Full<HyperBytes>> {
    let response = if let Value::Array(items) = request {
        let mut responses = Vec::new();
        for item in items {
            if let Some(response) = composer_item(ctx, item).await {
                responses.push(response);
            }
        }
        (!responses.is_empty()).then_some(Value::Array(responses))
    } else {
        composer_item(ctx, request).await
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

async fn handle(
    req: Request<hyper::body::Incoming>,
    ctx: Ctx,
) -> Result<Response<Full<HyperBytes>>, hyper::Error> {
    if req.method() == Method::OPTIONS {
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type")
            .body(Full::new(HyperBytes::new()))
            .expect("valid response"));
    }
    if req.method() != Method::POST {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(HyperBytes::from("method not allowed")))
            .expect("valid response"));
    }

    if content_length_exceeds(&req, MAX_BODY) {
        return Ok(body_too_large());
    }

    let body_bytes = match collect_body_limited(req.into_body(), MAX_BODY).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(())) => return Ok(body_too_large()),
        Err(e) => {
            event!(name: "eez.xchain_front.body_read_failed", Level::DEBUG, error = %e, "read body failed");
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(HyperBytes::from("read body failed")))
                .expect("valid response"));
        }
    };

    let Ok(json) = serde_json::from_slice::<Value>(&body_bytes) else {
        return Ok(rpc_error(Value::Null, -32700, "parse error"));
    };

    // A JSON-RPC batch array would bypass the single-tx intercept and forward a
    // bundled cross-chain tx straight to the mempool; reject any batch carrying
    // `eth_sendRawTransaction`.
    if batch_has_send_raw(&json) {
        return Ok(rpc_error(
            Value::Null,
            -32600,
            "send cross-chain eth_sendRawTransaction singly, not in a JSON-RPC batch",
        ));
    }

    // The batch handler retains all replies until the entire batch finishes.
    // Per-quote capture limits must not multiply into an unbounded trace reply.
    if let Value::Array(items) = &json
        && items.iter().any(|item| {
            item["method"] == crate::fee_quotes::DEBUG_METHOD
                && item["params"][1].get("trace").is_some()
        })
    {
        return Ok(rpc_error(
            Value::Null,
            -32600,
            "traced eez_debugQuoteFee requests must be sent individually",
        ));
    }

    let method = json.get("method").and_then(Value::as_str).unwrap_or("");

    // Split only requests containing front-local work. Pure Ethereum requests
    // keep their existing transparent forwarding and estimation behavior.
    if contains_composer_method(&json) {
        return Ok(handle_composer_methods(&ctx, &json).await);
    }

    // An unrouted variant would be forwarded to the source chain, skipping the
    // held pool entirely.
    if is_submission_method(method) && method != SEND_RAW {
        return Ok(rpc_error(
            json.get("id").cloned().unwrap_or(Value::Null),
            -32601,
            "only eth_sendRawTransaction is routed by the cross-chain front",
        ));
    }

    // Intercept a single `eth_sendRawTransaction`. Undecodable tx bytes fall
    // through so the upstream RPC produces the standard JSON-RPC error.
    if method == SEND_RAW
        && let Some(raw_hex) = json
            .get("params")
            .and_then(|p| p.get(0))
            .and_then(Value::as_str)
        && let Some(resp) = intercept_send_raw(&ctx, raw_hex, &json).await
    {
        return Ok(resp);
    }

    if !all_forwarded_methods_are_eth(&json) {
        let id = json.get("id").cloned().unwrap_or(Value::Null);
        return Ok(rpc_error(
            id,
            -32601,
            "cross-chain front forwards only eth_* JSON-RPC methods",
        ));
    }
    if estimate::contains_estimate(&json) {
        return Ok(estimate::handle(&ctx, &json).await);
    }
    Ok(forward(&ctx, body_bytes.to_vec()).await)
}

fn trace_contains_cross_chain_call(trace: &Value, dispatch_address: Address) -> bool {
    let target = trace
        .get("to")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Address>().ok());
    let input = trace
        .get("input")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Bytes>().ok());
    let dispatch = target.zip(input).is_some_and(|(target, input)| {
        eez_composer::composer::estimate::is_dispatch(target, &input, dispatch_address)
    });
    if dispatch {
        return true;
    }
    trace
        .get("calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| {
            calls
                .iter()
                .any(|call| trace_contains_cross_chain_call(call, dispatch_address))
        })
}

async fn transaction_is_cross_chain(ctx: &Ctx, envelope: &TxEnvelope) -> Result<bool, String> {
    let sender = envelope
        .recover_signer()
        .map_err(|e| format!("signature recovery failed before trace: {e}"))?;
    let mut call = serde_json::Map::new();
    call.insert("from".into(), Value::String(sender.to_string()));
    call.insert(
        "gas".into(),
        Value::String(format!("0x{:x}", envelope.gas_limit())),
    );
    call.insert(
        "value".into(),
        Value::String(format!("{:#x}", envelope.value())),
    );
    call.insert(
        "data".into(),
        Value::String(format!("{:#x}", envelope.input())),
    );
    if let Some(to) = envelope.to() {
        call.insert("to".into(), Value::String(to.to_string()));
    }

    // L2 routing observes an executed state/header pair, as the local gas
    // classifier does. Reth's derived pending state cannot apply a scheduled
    // history import; treating that predictable error as cross-chain ownership
    // strands ordinary calls. L1 retains its pending-state classification.
    let block = match ctx.direction {
        Direction::Outbound => "latest",
        Direction::Inbound => "pending",
    };
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "debug_traceCall",
        "params": [
            Value::Object(call),
            block,
            { "tracer": "callTracer" }
        ],
    });
    let response = ctx
        .client
        .post(&ctx.upstream_rpc_url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("debug_traceCall transport failed: {e}"))?;
    let response: Value = response
        .json()
        .await
        .map_err(|e| format!("debug_traceCall response decode failed: {e}"))?;
    let trace = response.get("result").ok_or_else(|| {
        let detail = response
            .get("error")
            .cloned()
            .unwrap_or_else(|| Value::String("missing result".into()));
        format!("debug_traceCall failed: {detail}")
    })?;
    Ok(trace_contains_cross_chain_call(trace, ctx.dispatch_address))
}

/// `Some(response)` when held or rejected here; `None` to forward an ordinary
/// transaction (or an undecodable tx, so the node returns its standard error).
async fn intercept_send_raw(
    ctx: &Ctx,
    raw_hex: &str,
    json: &Value,
) -> Option<Response<Full<HyperBytes>>> {
    let raw: Bytes = raw_hex.parse().ok()?;
    let envelope = TxEnvelope::decode_2718(&mut raw.as_ref()).ok()?;
    // A creation has no target, so it can never compose. Forwarding deploys it
    // on the source chain, which is what the sender wanted.
    if envelope.is_create() {
        event!(
            name: "eez.xchain_front.create_forwarded",
            Level::INFO,
            direction = ?ctx.direction,
            "contract creation forwarded to the source chain's RPC",
        );
        return None;
    }
    let id = json.get("id").cloned().unwrap_or(Value::Null);
    let hash: B256 = keccak256(raw.as_ref());
    // A predecessor may change whether execution reaches dispatch. Only the
    // stateful fee path can classify such a call; an isolated trace could revert
    // early and incorrectly release it into the ordinary mempool.
    let requires_owned_state = ctx.fee_quotes.as_ref().is_some_and(|s| s.enforce_tariff)
        && (ctx.held_pool.owns(hash)
            || envelope.recover_signer().is_ok_and(|sender| {
                ctx.held_pool
                    .owns_predecessor(sender, ctx.direction, envelope.nonce())
            }));
    let classification = if requires_owned_state {
        Ok(true)
    } else {
        transaction_is_cross_chain(ctx, &envelope).await
    };
    match classification {
        Ok(false) => {
            event!(
                name: "eez.xchain_front.ordinary_forwarded",
                Level::INFO,
                tx_hash = %hash,
                direction = ?ctx.direction,
                "ordinary transaction forwarded to source-chain mempool",
            );
            return None;
        }
        Ok(true) => {}
        Err(error) => {
            event!(
                name: "eez.xchain_front.classification_failed_closed",
                Level::WARN,
                tx_hash = %hash,
                direction = ?ctx.direction,
                %error,
                "transaction trace classification failed; holding as cross-chain",
            );
        }
    }

    // Holding before the node is live would strand the transaction in a pool
    // nothing drains. Ordinary transactions took the forwarding branch above.
    if !ctx.ready.load(std::sync::atomic::Ordering::Relaxed) {
        return Some(rpc_error(id, -32000, STARTING_UP_MSG));
    }
    if let Some(service) = &ctx.fee_quotes
        && service.enforce_tariff
    {
        if ctx.held_pool.owns(hash) {
            return Some(json_response(
                serde_json::json!({"jsonrpc":"2.0","id":id,"result":hash}).to_string(),
            ));
        }
        let (status, result) = service.check_signed(&envelope).await;
        if !status.is_success() {
            tracing::info!(event_name = "eez.xchain_front.funding_rejected",
                tx_hash = %hash, direction = ?ctx.direction,
                error = %result["error"], "signed source failed settlement admission");
            return Some(json_response(serde_json::json!({
                "jsonrpc":"2.0", "id":id,
                "error":{"code":-32003,"message":"settlement tariff admission failed", "data":result["error"]}
            }).to_string()));
        }
    }
    let resp_body = match gate_and_hold(
        &envelope,
        &raw,
        ctx.direction,
        &ctx.held_pool,
        ctx.expected_source_chain_id,
        &ctx.validation_provider,
    )
    .await
    {
        Admission::Held(hash) => serde_json::json!({ "jsonrpc": "2.0", "result": hash, "id": id }),
        Admission::Rejected(msg) => {
            event!(
                name: "eez.xchain_front.rejected",
                Level::WARN,
                reason = %msg,
                direction = ?ctx.direction,
                "cross-chain tx rejected at front",
            );
            serde_json::json!({ "jsonrpc": "2.0", "error": { "code": -32000, "message": msg }, "id": id })
        }
    };
    Some(json_response(resp_body.to_string()))
}

/// Forward the request body verbatim to the source chain's RPC and relay back.
async fn forward(ctx: &Ctx, body: Vec<u8>) -> Response<Full<HyperBytes>> {
    match ctx
        .client
        .post(&ctx.upstream_rpc_url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status();
            let bytes = r.bytes().await.unwrap_or_default();
            Response::builder()
                .status(status.as_u16())
                .header("Content-Type", "application/json")
                .header("Access-Control-Allow-Origin", "*")
                .body(Full::new(HyperBytes::from(bytes.to_vec())))
                .expect("valid response")
        }
        Err(e) => {
            event!(name: "eez.xchain_front.upstream_error", Level::WARN, error = %e, "upstream error");
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Full::new(HyperBytes::from(format!("upstream error: {e}"))))
                .expect("valid response")
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEip4844, TxLegacy};
    use alloy_network::{TxSignerSync, eip2718::Encodable2718};
    use alloy_primitives::TxKind;
    use alloy_provider::ProviderBuilder;
    use alloy_provider::mock::Asserter;
    use alloy_rpc_types_eth::AccessList;
    use alloy_signer_local::PrivateKeySigner;
    use serde_json::json;

    use super::*;

    fn test_signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::with_last_byte(1)).expect("valid signer")
    }

    fn signed_eip1559(
        chain_id: u64,
        value: U256,
        gas_limit: u64,
        max_fee_per_gas: u128,
    ) -> (PrivateKeySigner, TxEnvelope, Bytes) {
        let signer = test_signer();
        let mut tx = TxEip1559 {
            chain_id,
            nonce: 0,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::ZERO),
            value,
            access_list: alloy_eips::eip2930::AccessList::default(),
            input: Bytes::default(),
        };
        let sig = signer.sign_transaction_sync(&mut tx).expect("tx signs");
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        (signer, envelope, raw)
    }

    fn signed_legacy(
        chain_id: Option<u64>,
        gas_price: u128,
    ) -> (PrivateKeySigner, TxEnvelope, Bytes) {
        let signer = test_signer();
        let mut tx = TxLegacy {
            chain_id,
            nonce: 0,
            gas_price,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::default(),
        };
        let sig = signer.sign_transaction_sync(&mut tx).expect("tx signs");
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        (signer, envelope, raw)
    }

    fn signed_blob(chain_id: u64) -> (PrivateKeySigner, TxEnvelope, Bytes) {
        let signer = test_signer();
        let mut tx = TxEip4844 {
            chain_id,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
            to: Address::ZERO,
            value: U256::ZERO,
            access_list: alloy_eips::eip2930::AccessList::default(),
            blob_versioned_hashes: vec![B256::with_last_byte(2)],
            max_fee_per_blob_gas: 1,
            input: Bytes::default(),
        };
        let sig = signer.sign_transaction_sync(&mut tx).expect("tx signs");
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        (signer, envelope, raw)
    }

    fn rejection(admission: Admission) -> String {
        match admission {
            Admission::Rejected(msg) => msg,
            Admission::Held(hash) => panic!("expected rejection, got held tx {hash}"),
        }
    }

    const SOURCE_BLOCK_GAS_LIMIT: u64 = 30_000_000;

    /// The source chain's latest block, as `eth_getBlockByNumber` returns it.
    fn push_latest_block(asserter: &Asserter, gas_limit: u64) {
        let block = alloy_rpc_types_eth::Block::<alloy_rpc_types_eth::Transaction> {
            header: alloy_rpc_types_eth::Header {
                inner: alloy_consensus::Header {
                    gas_limit,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        asserter.push_success(&block);
    }

    /// One admitted tx makes four lookups: latest block, code, balance, nonce.
    /// The mock answers in push order, not by method, so the order matters.
    fn push_admission_lookups(asserter: &Asserter, balance: u64, nonce: u64) {
        push_latest_block(asserter, SOURCE_BLOCK_GAS_LIMIT);
        asserter.push_success(&Bytes::from_static(&[0x60, 0x00]));
        asserter.push_success(&U256::from(balance));
        asserter.push_success(&nonce);
    }

    #[tokio::test]
    async fn matching_source_chain_id_is_accepted() {
        let (_, envelope, raw) = signed_eip1559(31337, U256::ZERO, 21_000, 1);
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        push_admission_lookups(&asserter, 1_000_000, 0);

        let admission =
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await;

        assert!(matches!(admission, Admission::Held(_)));
        assert_eq!(pool.len(), 1);
    }

    #[tokio::test]
    async fn wrong_source_chain_id_is_rejected_without_pool_mutation() {
        let (signer, envelope, raw) = signed_eip1559(1, U256::ZERO, 21_000, 1);
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Outbound, &pool, 2, &provider).await,
        );

        assert!(msg.contains("wrong source-chain id 1"));
        assert_eq!(pool.len(), 0, "rejected tx from {signer:?} mutated pool");
    }

    #[tokio::test]
    async fn missing_source_chain_id_is_rejected() {
        let (signer, envelope, raw) = signed_legacy(None, 1);
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );

        assert!(msg.contains("missing source-chain id"));
        assert_eq!(pool.len(), 0, "rejected tx from {signer:?} mutated pool");
    }

    fn signed_eip1559_with(
        gas_limit: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        input: Bytes,
    ) -> (TxEnvelope, Bytes) {
        let signer = test_signer();
        let mut tx = TxEip1559 {
            chain_id: 31337,
            nonce: 0,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            access_list: alloy_eips::eip2930::AccessList::default(),
            input,
        };
        let sig = signer.sign_transaction_sync(&mut tx).expect("tx signs");
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        (envelope, raw)
    }

    /// No base fee can make a tip above the fee cap valid (EIP-1559).
    #[tokio::test]
    async fn priority_fee_above_fee_cap_is_rejected_at_submission() {
        let (envelope, raw) = signed_eip1559_with(21_000, 10, 11, Bytes::new());
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );

        assert_eq!(
            msg,
            "max_priority_fee_per_gas 11 exceeds max_fee_per_gas 10"
        );
        assert_eq!(pool.len(), 0);
    }

    /// A gas limit below intrinsic gas can never execute. Calldata raises the
    /// intrinsic cost, including the EIP-7623 floor.
    #[tokio::test]
    async fn gas_limit_below_intrinsic_gas_is_rejected_at_submission() {
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let (envelope, raw) = signed_eip1559_with(20_999, 10, 1, Bytes::new());
        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );
        assert_eq!(
            msg,
            "gas limit 20999 is below the transaction's intrinsic gas 21000"
        );

        // 100 nonzero bytes: 21000 + 100 * 16 = 22600, floor 21000 + 400 * 10 = 25000.
        let (envelope, raw) = signed_eip1559_with(22_600, 10, 1, Bytes::from(vec![1u8; 100]));
        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );
        assert_eq!(
            msg,
            "gas limit 22600 is below the transaction's intrinsic gas 25000"
        );
        assert_eq!(pool.len(), 0);
    }

    /// Osaka caps one transaction at 2^24 gas on either source chain, before
    /// any chain lookup.
    #[tokio::test]
    async fn gas_limit_above_the_per_transaction_cap_is_rejected_at_submission() {
        let (envelope, raw) = signed_eip1559_with(16_777_217, 10, 1, Bytes::new());
        for direction in [Direction::Inbound, Direction::Outbound] {
            let pool = HeldPool::new(Default::default());
            let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());
            let msg =
                rejection(gate_and_hold(&envelope, &raw, direction, &pool, 31337, &provider).await);
            assert_eq!(
                msg,
                "gas limit 16777217 exceeds the per-transaction cap 16777216 (EIP-7825)"
            );
            assert_eq!(pool.len(), 0);
        }
    }

    /// No block on the source chain can include more gas than its current limit.
    #[tokio::test]
    async fn gas_limit_above_source_block_gas_limit_is_rejected_at_submission() {
        let (envelope, raw) = signed_eip1559_with(10_000_001, 10, 1, Bytes::new());
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        push_latest_block(&asserter, 10_000_000);

        let msg = rejection(
            gate_and_hold(
                &envelope,
                &raw,
                Direction::Outbound,
                &pool,
                31337,
                &provider,
            )
            .await,
        );

        assert_eq!(
            msg,
            "gas limit 10000001 exceeds the source chain's block gas limit 10000000"
        );
        assert_eq!(pool.len(), 0);
    }

    /// The boundaries themselves (intrinsic gas, the per-transaction cap and an
    /// equal tip) are valid, and a fee cap below the current
    /// base fee is still held: it can become includable later.
    #[tokio::test]
    async fn boundary_gas_limits_and_equal_tip_are_accepted() {
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        push_admission_lookups(&asserter, 1_000_000_000, 0);
        let (envelope, raw) = signed_eip1559_with(21_000, 10, 10, Bytes::new());
        assert!(matches!(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
            Admission::Held(_)
        ));

        let pool = HeldPool::new(Default::default());
        push_admission_lookups(&asserter, u64::MAX, 0);
        let (envelope, raw) = signed_eip1559_with(TX_GAS_LIMIT_CAP, 1, 1, Bytes::new());
        assert!(matches!(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
            Admission::Held(_)
        ));
    }

    #[tokio::test]
    async fn blob_envelope_is_rejected() {
        let (signer, envelope, raw) = signed_blob(31337);
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );

        assert!(msg.contains("unsupported transaction type"));
        assert_eq!(pool.len(), 0, "rejected tx from {signer:?} mutated pool");
    }

    #[tokio::test]
    async fn legacy_gas_price_is_both_cap_and_tip_for_replacement() {
        let (_, original, original_raw) = signed_legacy(Some(31337), 100);
        let (_, underpriced, underpriced_raw) = signed_legacy(Some(31337), 105);
        let (_, replacement, replacement_raw) = signed_legacy(Some(31337), 110);
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        for _ in 0..3 {
            push_admission_lookups(&asserter, 10_000_000, 0);
        }

        assert!(matches!(
            gate_and_hold(
                &original,
                &original_raw,
                Direction::Inbound,
                &pool,
                31337,
                &provider
            )
            .await,
            Admission::Held(_)
        ));
        // 5% gas_price bump < 10% → rejected on both fee dimensions.
        let msg = rejection(
            gate_and_hold(
                &underpriced,
                &underpriced_raw,
                Direction::Inbound,
                &pool,
                31337,
                &provider,
            )
            .await,
        );
        assert!(msg.contains("replacement underpriced"), "{msg}");
        // 10% bump → replaces.
        assert!(matches!(
            gate_and_hold(
                &replacement,
                &replacement_raw,
                Direction::Inbound,
                &pool,
                31337,
                &provider,
            )
            .await,
            Admission::Held(_)
        ));
        assert_eq!(pool.len(), 1);
    }

    #[tokio::test]
    async fn eip7702_envelope_is_rejected() {
        let signer = test_signer();
        let mut tx = alloy_consensus::TxEip7702 {
            chain_id: 31337,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
            to: Address::ZERO,
            value: U256::ZERO,
            access_list: alloy_eips::eip2930::AccessList::default(),
            authorization_list: vec![],
            input: Bytes::default(),
        };
        let sig = signer.sign_transaction_sync(&mut tx).expect("tx signs");
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );

        assert!(msg.contains("unsupported transaction type"), "{msg}");
        assert_eq!(pool.len(), 0);
    }

    #[tokio::test]
    async fn upfront_cost_overflow_is_rejected() {
        let (signer, envelope, raw) = signed_eip1559(31337, U256::MAX, 21_000, 1);
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Inbound, &pool, 31337, &provider).await,
        );

        assert!(msg.contains("upfront cost overflow"));
        assert_eq!(pool.len(), 0, "rejected tx from {signer:?} mutated pool");
    }

    #[test]
    fn batch_send_raw_is_not_forwardable() {
        let body = json!([
            {"jsonrpc": "2.0", "method": "eth_chainId", "params": [], "id": 1},
            {"jsonrpc": "2.0", "method": "eth_sendRawTransaction", "params": ["0xdead"], "id": 2}
        ]);
        assert!(batch_has_send_raw(&body));
        assert!(
            !all_forwarded_methods_are_eth(&body),
            "sendRaw batches must be rejected before forwarding"
        );
    }

    /// A longer submission method must not slip past the gate.
    #[test]
    fn submission_variants_are_recognised_not_just_the_exact_method() {
        assert!(is_submission_method("eth_sendRawTransaction"));
        assert!(is_submission_method("eth_sendRawTransactionConditional"));
        assert!(!is_submission_method("eth_getBalance"));
        assert!(!is_submission_method("eth_call"));
        // Batches carrying any variant are refused, not forwarded.
        assert!(batch_has_send_raw(&json!([
            {"jsonrpc": "2.0", "method": "eth_sendRawTransactionConditional", "params": [], "id": 1}
        ])));
    }

    #[test]
    fn only_eth_methods_are_forwardable() {
        assert!(all_forwarded_methods_are_eth(&json!({
            "jsonrpc": "2.0",
            "method": "eth_chainId",
            "params": [],
            "id": 1
        })));
        assert!(all_forwarded_methods_are_eth(&json!([
            {"jsonrpc": "2.0", "method": "eth_chainId", "params": [], "id": 1},
            {"jsonrpc": "2.0", "method": "eth_getBalance", "params": ["0x0000000000000000000000000000000000000000", "latest"], "id": 2}
        ])));
        assert!(!all_forwarded_methods_are_eth(&json!({
            "jsonrpc": "2.0",
            "method": "web3_clientVersion",
            "params": [],
            "id": 1
        })));
        assert!(!all_forwarded_methods_are_eth(&json!([
            {"jsonrpc": "2.0", "method": "eth_chainId", "params": [], "id": 1},
            {"jsonrpc": "2.0", "method": "admin_peers", "params": [], "id": 2}
        ])));
    }

    #[test]
    fn call_trace_classifier_finds_nested_cross_chain_entrypoint() {
        let dispatch = Address::repeat_byte(0x42);
        let trace = json!({
            "to": Address::repeat_byte(0x11),
            "input": "0x12345678",
            "calls": [{
                "to": dispatch,
                "input": "0x9af5325900000000",
                "error": "execution reverted"
            }]
        });
        assert!(trace_contains_cross_chain_call(&trace, dispatch));
    }

    #[test]
    fn call_trace_classifier_ignores_ordinary_dispatch_reads() {
        let dispatch = Address::repeat_byte(0x42);
        let trace = json!({
            "to": dispatch,
            "input": "0xeb20c0aa00000000",
            "calls": [{
                "to": Address::repeat_byte(0x11),
                "input": "0x9af5325900000000"
            }]
        });
        assert!(!trace_contains_cross_chain_call(&trace, dispatch));
    }

    #[tokio::test]
    async fn signed_ingress_uses_executed_l2_state_and_keeps_dispatch_guard() {
        use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        for (direction, expected_block) in [
            (Direction::Outbound, "latest"),
            (Direction::Inbound, "pending"),
        ] {
            let dispatcher = Address::repeat_byte(0x42);
            let (_, _, raw) = signed_eip1559(31337, U256::ZERO, 50_000, 1);
            let hash = keccak256(raw.as_ref());
            let dispatch = Arc::new(AtomicBool::new(false));
            let forwarded = Arc::new(AtomicUsize::new(0));
            let upstream = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", upstream.local_addr().unwrap());
            let mut module = RpcModule::new((dispatch.clone(), forwarded.clone()));
            module
                .register_method("debug_traceCall", move |params, state, _| {
                    let params: Vec<Value> = params.parse()?;
                    if params[1] != expected_block {
                        return Err(ErrorObjectOwned::owned(
                            -32603,
                            "pending L1 history import has not executed",
                            None::<()>,
                        ));
                    }
                    Ok::<_, ErrorObjectOwned>(if state.0.load(Ordering::Relaxed) {
                        json!({"type":"CALL", "calls":[{
                            "to":dispatcher, "input":"0x9af53259", "error":"execution reverted"
                        }]})
                    } else {
                        json!({"type":"CALL", "calls":[]})
                    })
                })
                .unwrap();
            module
                .register_method("eth_sendRawTransaction", move |_, state, _| {
                    state.1.fetch_add(1, Ordering::Relaxed);
                    json!(hash)
                })
                .unwrap();
            let server = upstream.start(module);
            let port = free_port();
            let pool = Arc::new(HeldPool::new(Default::default()));
            let task = tokio::spawn(run_cross_chain_front(
                port,
                url,
                direction,
                pool.clone(),
                ProviderBuilder::default().connect_mocked_client(Asserter::new()),
                31337,
                dispatcher,
                Arc::new(AtomicBool::new(false)),
                Arc::new(json!({})),
                None,
            ));
            wait_for_front(port).await;
            let ordinary = send_raw(port, &raw, 1).await;
            assert_eq!(
                ordinary["result"],
                json!(hash),
                "ordinary L2 work must not enter held ownership at a history boundary: {ordinary}"
            );
            assert!(!pool.owns(hash));
            dispatch.store(true, Ordering::Relaxed);
            let guarded = send_raw(port, &raw, 2).await;
            assert_eq!(guarded["error"]["message"], STARTING_UP_MSG);
            assert_eq!(
                forwarded.load(Ordering::Relaxed),
                1,
                "a caught dispatcher call cannot enter the ordinary mempool"
            );
            task.abort();
            server.stop().unwrap();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn wait_for_front(port: u16) {
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("cross-chain front did not bind to port {port}");
    }

    async fn post(port: u16, body: &Value) -> Value {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}"))
            .json(body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn l2_fee_rpc_without_policy_reports_disabled_locally() {
        let port = free_port();
        let task = tokio::spawn(run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Outbound,
            Arc::new(HeldPool::new(Default::default())),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(serde_json::json!({})),
            None,
        ));
        wait_for_front(port).await;
        let response = post(
            port,
            &serde_json::json!({
                "jsonrpc":"2.0", "id":1, "method":crate::fee_quotes::METHOD, "params":[{}]
            }),
        )
        .await;
        assert_eq!(response["error"]["code"], -32004);
        assert_eq!(response["error"]["data"]["code"], "quotes_disabled");
        task.abort();
    }

    /// Discovery is answered by the front itself: before readiness, without
    /// contacting the (here unreachable) upstream, singly and in batches.
    #[tokio::test]
    async fn front_answers_composer_info_locally() {
        let port = free_port();
        let info = serde_json::json!({"eezContracts": {"eezRegistryAddress": "0x11"}});
        let task = tokio::spawn(run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Inbound,
            Arc::new(HeldPool::new(Default::default())),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(info.clone()),
            None,
        ));
        wait_for_front(port).await;

        let response = post(
            port,
            &serde_json::json!({"jsonrpc": "2.0", "method": "eez_composerInfo", "id": 7}),
        )
        .await;
        assert_eq!(
            response,
            serde_json::json!({"jsonrpc": "2.0", "id": 7, "result": info})
        );
        let response = post(
            port,
            &serde_json::json!({"jsonrpc": "2.0", "method": "eez_composerInfo", "params": [1], "id": 8}),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602);
        let response = post(
            port,
            &serde_json::json!([{"jsonrpc": "2.0", "method": "eez_composerInfo", "id": 9}]),
        )
        .await;
        assert_eq!(
            response,
            serde_json::json!([{"jsonrpc":"2.0", "id":9, "result":info}])
        );
        task.abort();
    }

    fn discovery_request(id: Value) -> Value {
        serde_json::json!({"jsonrpc":"2.0", "id":id, "method":"eth_call", "params":[{
            "to":eez_protocol::discovery::ADDRESS,
            "input":format!("0x98da5085{}", "a5".repeat(32)),
            "from":Address::repeat_byte(0x99), "gas":"0x0", "value":"0x0"
        }, "latest"]})
    }

    #[tokio::test]
    async fn front_discovery_works_before_readiness_on_both_chains_and_suppresses_notifications() {
        for (direction, chain) in [(Direction::Inbound, 1), (Direction::Outbound, 10201)] {
            let port = free_port();
            let info = serde_json::json!({"version":"test", "supportedNetworks":{"eezL1":1,"eezL2":10201}});
            let task = tokio::spawn(run_cross_chain_front(
                port,
                "http://127.0.0.1:1".into(),
                direction,
                Arc::new(HeldPool::new(Default::default())),
                ProviderBuilder::default().connect_mocked_client(Asserter::new()),
                chain,
                Address::ZERO,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                Arc::new(info.clone()),
                None,
            ));
            wait_for_front(port).await;
            for id in [
                serde_json::json!("wallet"),
                serde_json::json!(7),
                Value::Null,
            ] {
                for block in [serde_json::json!("latest"), serde_json::json!("0x9ff58")] {
                    let mut request = discovery_request(id.clone());
                    request["params"][1] = block;
                    let response = post(port, &request).await;
                    assert_eq!(response["id"], id);
                    assert_eq!(
                        response["result"],
                        serde_json::json!(Bytes::from(eez_protocol::discovery::encode_response(
                            B256::repeat_byte(0xa5),
                            chain,
                            info.to_string().as_bytes()
                        )))
                    );
                }
            }
            let mut notification = discovery_request(Value::Null);
            notification.as_object_mut().unwrap().remove("id");
            let mut invalid_notification = notification.clone();
            invalid_notification["params"][1] = serde_json::json!("not-a-block");
            for body in [
                notification.clone(),
                serde_json::json!([
                    notification, invalid_notification,
                    {"jsonrpc":"2.0", "method":"eez_composerInfo"},
                    {"jsonrpc":"2.0", "method":"eez_unknown"}
                ]),
            ] {
                let response = reqwest::Client::new()
                    .post(format!("http://127.0.0.1:{port}"))
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NO_CONTENT);
                assert!(response.bytes().await.unwrap().is_empty());
            }
            task.abort();
        }
    }

    #[tokio::test]
    async fn front_discovery_mixed_batches_keep_routing_ids_and_submission_guard() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", upstream.local_addr().unwrap());
        let forwarded = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let observed = Arc::clone(&forwarded);
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = upstream.accept().await.unwrap();
                let observed = Arc::clone(&observed);
                connections.spawn(async move {
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let observed = Arc::clone(&observed);
                        async move {
                            let bytes = req.into_body().collect().await.unwrap().to_bytes();
                            let request: Value = serde_json::from_slice(&bytes).unwrap();
                            observed.lock().unwrap().push(request.clone());
                            let result = if request["method"] == "eth_estimateGas" {
                                "0x5208"
                            } else {
                                "0xdeadbeef"
                            };
                            Ok::<_, std::convert::Infallible>(json_response(
                                serde_json::json!({
                                    "jsonrpc":"2.0", "id":request["id"], "result":result
                                })
                                .to_string(),
                            ))
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let port = free_port();
        let info = serde_json::json!({"version":"test"});
        let task = tokio::spawn(run_cross_chain_front(
            port,
            url,
            Direction::Outbound,
            Arc::new(HeldPool::new(Default::default())),
            ProviderBuilder::default().connect_mocked_client(Asserter::new()),
            10201,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(info.clone()),
            None,
        ));
        wait_for_front(port).await;
        let mut ordinary = discovery_request(serde_json::json!("ordinary"));
        ordinary["params"][0]["to"] = serde_json::json!(Address::repeat_byte(0x12));
        let mut other_selector = discovery_request(serde_json::json!("selector"));
        other_selector["params"][0]["input"] = serde_json::json!("0x12345678");
        let mut estimate = ordinary.clone();
        estimate["method"] = serde_json::json!("eth_estimateGas");
        estimate["id"] = serde_json::json!("estimate");
        let mut notification = ordinary.clone();
        notification.as_object_mut().unwrap().remove("id");
        let batch = serde_json::json!([
            discovery_request(serde_json::json!(1)), ordinary, other_selector, estimate, notification,
            {"jsonrpc":"2.0", "id":null, "method":"eez_composerInfo"},
            {"jsonrpc":"2.0", "id":-2, "method":"eez_getCrossChainQueueStatus"},
            {"jsonrpc":"2.0", "id":3, "method":"eez_getCrossChainTransaction", "params":[B256::ZERO]},
            {"jsonrpc":"2.0", "id":4, "method":"eez_unknown"},
            {"jsonrpc":"2.0", "id":5, "method":"debug_traceCall"},
            false,
            {"jsonrpc":"2.0", "id":[], "method":"eez_composerInfo"}
        ]);
        let response = post(port, &batch).await;
        let items = response.as_array().unwrap();
        assert_eq!(items.len(), 11);
        assert_eq!(items[1]["id"], "ordinary");
        assert_eq!(items[1]["result"], "0xdeadbeef");
        assert_eq!(items[2]["id"], "selector");
        assert_eq!(items[2]["result"], "0xdeadbeef");
        assert_eq!(items[3]["id"], "estimate");
        assert_eq!(items[3]["result"], "0x5208");
        assert!(items[4]["id"].is_null());
        assert_eq!(items[4]["result"], info);
        assert_eq!(items[5]["result"]["queued"], 0);
        assert_eq!(items[5]["id"], -2);
        assert!(items[6]["result"].is_null());
        assert_eq!(items[7]["error"]["code"], -32601);
        assert_eq!(items[8]["error"]["code"], -32601);
        assert_eq!(items[9]["error"]["code"], -32600);
        assert_eq!(items[10]["error"]["code"], -32600);
        assert_eq!(
            *forwarded.lock().unwrap(),
            vec![ordinary, other_selector, estimate, notification]
        );
        for method in [
            "eth_sendRawTransaction",
            "eth_sendRawTransactionConditional",
        ] {
            let response = post(
                port,
                &serde_json::json!([
                    discovery_request(serde_json::json!(1)),
                    {"jsonrpc":"2.0", "id":2, "method":method, "params":["0x01"]}
                ]),
            )
            .await;
            assert_eq!(response["error"]["code"], -32600);
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("singly")
            );
            assert_eq!(forwarded.lock().unwrap().len(), 4);
        }
        task.abort();
        server.abort();
    }

    async fn send_raw(port: u16, raw: &Bytes, id: u64) -> Value {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_sendRawTransaction",
                "params": [format!("{raw:#x}")],
                "id": id,
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn signed_transfer(
        signer: &PrivateKeySigner,
        nonce: u64,
        value: u64,
        max_fee_per_gas: u128,
    ) -> (TxEnvelope, Bytes) {
        let mut tx = TxEip1559 {
            chain_id: 1,
            nonce,
            gas_limit: 21_000,
            max_fee_per_gas,
            max_priority_fee_per_gas: max_fee_per_gas,
            to: TxKind::Call(Address::repeat_byte(0x42)),
            value: U256::from(value),
            access_list: AccessList::default(),
            input: Bytes::new(),
        };
        let sig = signer.sign_transaction_sync(&mut tx).unwrap();
        let envelope = TxEnvelope::from(tx.into_signed(sig));
        let raw = Bytes::from(envelope.encoded_2718());
        (envelope, raw)
    }

    #[tokio::test]
    async fn gate_handles_landed_but_not_derived_reservation() {
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(1)).unwrap();
        let (first, first_raw) = signed_transfer(&signer, 0, 1, 1);
        let (same_nonce, same_nonce_raw) = signed_transfer(&signer, 0, 2, 2);
        let (next, next_raw) = signed_transfer(&signer, 1, 3, 1);
        let (gapped, gapped_raw) = signed_transfer(&signer, 2, 4, 1);
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());

        for _ in 0..2 {
            push_admission_lookups(&asserter, 1_000_000, 0);
        }
        push_admission_lookups(&asserter, 1_000_000, 1);
        push_admission_lookups(&asserter, 1_000_000, 1);

        assert!(matches!(
            gate_and_hold(&first, &first_raw, Direction::Inbound, &pool, 1, &provider,).await,
            Admission::Held(_)
        ));

        let drained = pool.pop_n(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(pool.len(), 0);

        let rejected = gate_and_hold(
            &same_nonce,
            &same_nonce_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await;
        let Admission::Rejected(message) = rejected else {
            panic!("same nonce should be rejected while first tx is in flight");
        };
        assert!(message.contains("invalid nonce 0"), "{message}");
        assert!(message.contains("next unreserved nonce 1"), "{message}");

        let rejected = gate_and_hold(
            &gapped,
            &gapped_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await;
        let Admission::Rejected(message) = rejected else {
            panic!("nonce 2 must not be admitted while nonce 1 is unreserved");
        };
        assert!(message.contains("nonce 1"), "{message}");

        assert!(matches!(
            gate_and_hold(&next, &next_raw, Direction::Inbound, &pool, 1, &provider,).await,
            Admission::Held(_)
        ));
    }

    #[tokio::test]
    async fn gate_replaces_queued_nonce_and_preserves_higher_suffix() {
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(1)).unwrap();
        let (original, original_raw) = signed_transfer(&signer, 0, 1, 100);
        let (suffix, suffix_raw) = signed_transfer(&signer, 1, 2, 100);
        let (replacement, replacement_raw) = signed_transfer(&signer, 0, 3, 110);
        let (underpriced, underpriced_raw) = signed_transfer(&signer, 0, 4, 109);
        let pool = HeldPool::new(Default::default());
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());

        for _ in 0..4 {
            push_admission_lookups(&asserter, 10_000_000, 0);
        }

        let Admission::Held(original_hash) = gate_and_hold(
            &original,
            &original_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await
        else {
            panic!("original transaction should be held");
        };
        // A 9% bump is below the required 10% replacement threshold.
        let Admission::Rejected(msg) = gate_and_hold(
            &underpriced,
            &underpriced_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await
        else {
            panic!("underpriced replacement must be rejected");
        };
        assert!(msg.contains("replacement underpriced"), "{msg}");
        let Admission::Held(suffix_hash) = gate_and_hold(
            &suffix,
            &suffix_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await
        else {
            panic!("suffix transaction should be held");
        };
        let Admission::Held(replacement_hash) = gate_and_hold(
            &replacement,
            &replacement_raw,
            Direction::Inbound,
            &pool,
            1,
            &provider,
        )
        .await
        else {
            panic!("replacement transaction should be held");
        };

        assert_ne!(original_hash, replacement_hash);
        let queued = pool.pop_all();
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].hash, replacement_hash);
        assert_eq!(queued[0].nonce, 0);
        assert_eq!(queued[1].hash, suffix_hash);
        assert_eq!(queued[1].nonce, 1);
    }

    #[tokio::test]
    async fn mismatched_upstream_chain_id_aborts_front_startup() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        asserter.push_success(&2_u64);

        let err = validate_cross_chain_front(&provider, 1)
            .await
            .expect_err("front must refuse to start on a chain-id mismatch");

        let msg = err.to_string();
        assert!(msg.contains("chain id 2"), "{msg}");
        assert!(msg.contains("expected 1"), "{msg}");
    }

    #[tokio::test]
    async fn failed_upstream_chain_id_lookup_aborts_front_startup() {
        // Empty asserter queue → the startup lookup errors.
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let err = validate_cross_chain_front(&provider, 1)
            .await
            .expect_err("front must refuse to start when the chain id lookup fails");

        assert!(err.to_string().contains("chain id lookup failed"), "{err}");
    }

    #[tokio::test]
    async fn real_http_fronts_route_directions_and_preserve_replacement_suffix() {
        let inbound_signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(1)).unwrap();
        let outbound_signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(2)).unwrap();
        let (_, inbound_original) = signed_transfer(&inbound_signer, 0, 1, 1);
        let (_, inbound_suffix) = signed_transfer(&inbound_signer, 1, 2, 1);
        let (_, inbound_replacement) = signed_transfer(&inbound_signer, 0, 3, 2);
        let (_, outbound) = signed_transfer(&outbound_signer, 0, 4, 1);

        let pool = HeldPool::new(Default::default());
        let pool = Arc::new(pool);
        let inbound_port = free_port();
        let outbound_port = free_port();

        let inbound_asserter = Asserter::new();
        let inbound_provider =
            ProviderBuilder::default().connect_mocked_client(inbound_asserter.clone());
        for _ in 0..3 {
            push_admission_lookups(&inbound_asserter, 1_000_000, 0);
        }
        let outbound_asserter = Asserter::new();
        let outbound_provider =
            ProviderBuilder::default().connect_mocked_client(outbound_asserter.clone());
        push_admission_lookups(&outbound_asserter, 1_000_000, 0);

        let inbound_task = tokio::spawn(run_cross_chain_front(
            inbound_port,
            "http://127.0.0.1:1".into(),
            Direction::Inbound,
            Arc::clone(&pool),
            inbound_provider,
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(serde_json::json!({"fixture": true})),
            None,
        ));
        let outbound_task = tokio::spawn(run_cross_chain_front(
            outbound_port,
            "http://127.0.0.1:1".into(),
            Direction::Outbound,
            Arc::clone(&pool),
            outbound_provider,
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(serde_json::json!({"fixture": true})),
            None,
        ));
        wait_for_front(inbound_port).await;
        wait_for_front(outbound_port).await;

        let original = send_raw(inbound_port, &inbound_original, 1).await;
        let suffix = send_raw(inbound_port, &inbound_suffix, 2).await;
        let replacement = send_raw(inbound_port, &inbound_replacement, 3).await;
        let outbound_response = send_raw(outbound_port, &outbound, 4).await;
        for response in [&original, &suffix, &replacement, &outbound_response] {
            assert!(response.get("error").is_none(), "{response}");
            assert!(response.get("result").is_some(), "{response}");
        }

        let queued = pool.pop_all();
        assert_eq!(queued.len(), 3);
        assert_eq!(queued[0].hash.to_string(), replacement["result"]);
        assert_eq!(queued[0].direction, Direction::Inbound);
        assert_eq!(queued[0].nonce, 0);
        assert_eq!(queued[1].hash.to_string(), suffix["result"]);
        assert_eq!(queued[1].direction, Direction::Inbound);
        assert_eq!(queued[1].nonce, 1);
        assert_eq!(queued[2].hash.to_string(), outbound_response["result"]);
        assert_eq!(queued[2].direction, Direction::Outbound);
        assert_eq!(queued[2].nonce, 0);

        inbound_task.abort();
        outbound_task.abort();
    }

    /// The refusal is a wire contract: harnesses retry on that text.
    #[tokio::test]
    async fn front_refuses_submissions_until_ready_without_touching_the_pool() {
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(3)).unwrap();
        let (_, tx) = signed_transfer(&signer, 0, 1, 1);

        let pool = HeldPool::new(Default::default());
        let pool = Arc::new(pool);
        let port = free_port();
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        push_admission_lookups(&asserter, 1_000_000, 0);

        let task = tokio::spawn(run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Inbound,
            Arc::clone(&pool),
            provider,
            1,
            Address::ZERO,
            Arc::clone(&ready),
            Arc::new(serde_json::json!({"fixture": true})),
            None,
        ));
        wait_for_front(port).await;

        let refused = send_raw(port, &tx, 1).await;
        assert_eq!(refused["error"]["code"], -32000, "{refused}");
        assert!(
            refused["error"]["message"].as_str().unwrap() == STARTING_UP_MSG,
            "harnesses match this text: {refused}"
        );
        assert_eq!(
            refused["id"], 1,
            "the caller's id must come back: {refused}"
        );
        assert!(pool.pop_all().is_empty(), "a refused tx must not be held");

        ready.store(true, std::sync::atomic::Ordering::Relaxed);
        let accepted = send_raw(port, &tx, 2).await;
        assert!(accepted.get("error").is_none(), "{accepted}");
        assert_eq!(pool.pop_all().len(), 1);

        task.abort();
    }

    /// Upstream RPC stub: records every request body, answers with `result`.
    async fn stub_upstream(result: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let recorder = Arc::clone(&recorder);
                        async move {
                            let body = req.into_body().collect().await.unwrap().to_bytes();
                            recorder
                                .lock()
                                .unwrap()
                                .push(String::from_utf8_lossy(&body).into_owned());
                            Ok::<_, hyper::Error>(json_response(
                                json!({"jsonrpc": "2.0", "result": result, "id": 1}).to_string(),
                            ))
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn signed_creation(signer: &PrivateKeySigner) -> Bytes {
        let mut tx = TxEip1559 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
            to: TxKind::Create,
            value: U256::ZERO,
            access_list: AccessList::default(),
            input: Bytes::from_static(&[0x60, 0x00]),
        };
        let sig = signer.sign_transaction_sync(&mut tx).unwrap();
        Bytes::from(TxEnvelope::from(tx.into_signed(sig)).encoded_2718())
    }

    /// A deploy sent to a front must reach the source chain, not the held pool.
    #[tokio::test]
    async fn creation_is_forwarded_upstream_and_never_held() {
        const UPSTREAM_HASH: &str =
            "0x1111111111111111111111111111111111111111111111111111111111111111";

        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(7)).unwrap();
        let raw = signed_creation(&signer);
        let (upstream, seen) = stub_upstream(UPSTREAM_HASH).await;
        let pool = HeldPool::new(Default::default());
        let pool = Arc::new(pool);
        let port = free_port();
        // Empty asserter: the gate must not run a single lookup for a creation.
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let task = tokio::spawn(run_cross_chain_front(
            port,
            upstream,
            Direction::Outbound,
            Arc::clone(&pool),
            provider,
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(serde_json::json!({"fixture": true})),
            None,
        ));
        wait_for_front(port).await;

        let response = send_raw(port, &raw, 9).await;
        assert_eq!(response["result"], UPSTREAM_HASH, "{response}");
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(
            bodies.len(),
            1,
            "upstream must receive the creation exactly once: {bodies:?}"
        );
        assert!(
            bodies[0].contains("eth_sendRawTransaction"),
            "the raw creation must be relayed verbatim: {}",
            bodies[0]
        );
        assert!(pool.pop_all().is_empty(), "a creation must not be held");

        task.abort();
    }

    /// A target with no code has no proxy to route through, so it is rejected at
    /// the door with the remedy instead of a quiet compose-time evict.
    #[tokio::test]
    async fn codeless_target_is_rejected_with_the_remedy_and_pool_untouched() {
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(8)).unwrap();
        let (_, raw) = signed_transfer(&signer, 0, 1, 1);
        let pool = HeldPool::new(Default::default());
        let pool = Arc::new(pool);
        let port = free_port();
        let asserter = Asserter::new();
        let provider = ProviderBuilder::default().connect_mocked_client(asserter.clone());
        // Empty code, then a good balance and nonce, so only the code check can
        // be what rejects this tx.
        push_latest_block(&asserter, SOURCE_BLOCK_GAS_LIMIT);
        asserter.push_success(&Bytes::new());
        asserter.push_success(&U256::from(1_000_000u64));
        asserter.push_success(&0_u64);

        let task = tokio::spawn(run_cross_chain_front(
            port,
            "http://127.0.0.1:1".into(),
            Direction::Inbound,
            Arc::clone(&pool),
            provider,
            1,
            Address::ZERO,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(serde_json::json!({"fixture": true})),
            None,
        ));
        wait_for_front(port).await;

        let rejected = send_raw(port, &raw, 1).await;
        assert_eq!(rejected["error"]["code"], -32000, "{rejected}");
        let msg = rejected["error"]["message"].as_str().unwrap();
        assert!(msg.contains("has no code on the source chain"), "{msg}");
        assert!(msg.contains("createCrossChainProxy"), "{msg}");
        assert!(msg.contains("resubmit"), "{msg}");
        assert!(pool.pop_all().is_empty(), "a rejected tx must not be held");

        task.abort();
    }

    /// `gate_and_hold` is public; a creation reaching it directly must not panic.
    #[tokio::test]
    async fn gate_refuses_a_creation_defensively() {
        let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(7)).unwrap();
        let raw = signed_creation(&signer);
        let envelope = TxEnvelope::decode_2718(&mut raw.as_ref()).unwrap();
        let pool = HeldPool::new(Default::default());
        let provider = ProviderBuilder::default().connect_mocked_client(Asserter::new());

        let msg = rejection(
            gate_and_hold(&envelope, &raw, Direction::Outbound, &pool, 1, &provider).await,
        );

        assert!(msg.contains("creations are not cross-chain"), "{msg}");
        assert_eq!(pool.len(), 0);
    }
}
