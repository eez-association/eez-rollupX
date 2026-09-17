//! Only four read-only RPC methods. Responses/errors never become log fields.
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

use crate::{
    config::Endpoint,
    model::{Data, Head, Queue, Source, quantity},
};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Transport,
    Http,
    Rpc,
    Unsupported,
    Malformed,
    Identity,
    Oversized,
}

impl Failure {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Http => "http",
            Self::Rpc => "rpc",
            Self::Unsupported => "unsupported",
            Self::Malformed => "malformed",
            Self::Identity => "identity_mismatch",
            Self::Oversized => "oversized",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rpc {
    client: reqwest::Client,
    permits: Arc<Semaphore>,
}

impl Rpc {
    pub fn new() -> Result<Self, &'static str> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "cannot initialize RPC client")?,
            permits: Arc::new(Semaphore::new(4)),
        })
    }

    pub(crate) async fn call(
        &self,
        url: &str,
        method: &'static str,
        params: Value,
    ) -> Result<Value, Failure> {
        self.call_bounded(url, method, params, MAX_RESPONSE_BYTES)
            .await
    }

    pub(crate) async fn block(
        &self,
        endpoint: &Endpoint,
        selector: Value,
        full: bool,
    ) -> Result<Value, Failure> {
        self.call_bounded(
            &endpoint.url,
            "eth_getBlockByNumber",
            json!([selector, full]),
            8 * MAX_RESPONSE_BYTES,
        )
        .await
    }

    pub(crate) async fn transaction(
        &self,
        endpoint: &Endpoint,
        hash: alloy_primitives::B256,
    ) -> Result<Value, Failure> {
        self.call_bounded(
            &endpoint.url,
            "eth_getTransactionByHash",
            json!([format!("{hash:#x}")]),
            8 * MAX_RESPONSE_BYTES,
        )
        .await
    }

    /// Receipt logs can be larger than aggregate probe responses. Retain the
    /// same timeout and shared concurrency budget, with an explicit byte cap.
    pub(crate) async fn receipt(
        &self,
        endpoint: &Endpoint,
        hash: alloy_primitives::B256,
    ) -> Result<Value, Failure> {
        self.call_bounded(
            &endpoint.url,
            "eth_getTransactionReceipt",
            json!([format!("{hash:#x}")]),
            8 * MAX_RESPONSE_BYTES,
        )
        .await
    }

    async fn call_bounded(
        &self,
        url: &str,
        method: &'static str,
        params: Value,
        limit: usize,
    ) -> Result<Value, Failure> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| Failure::Transport)?;
        let mut response = self
            .client
            .post(url)
            .json(&json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}))
            .send()
            .await
            .map_err(|_| Failure::Transport)?;
        if !response.status().is_success() {
            return Err(Failure::Http);
        }
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(Failure::Oversized);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Transport)? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(Failure::Oversized);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Malformed)?;
        if value["jsonrpc"] != "2.0" || value["id"] != 1 {
            return Err(Failure::Malformed);
        }
        if let Some(error) = value.get("error") {
            return Err(if error["code"] == -32601 {
                Failure::Unsupported
            } else {
                Failure::Rpc
            });
        }
        value.get("result").cloned().ok_or(Failure::Malformed)
    }

    pub async fn collect(&self, source: Source, endpoint: &Endpoint) -> Result<Data, Failure> {
        self.validate_identity(endpoint).await?;
        self.collect_validated(source, endpoint).await
    }

    pub(crate) async fn validate_identity(&self, endpoint: &Endpoint) -> Result<(), Failure> {
        // Revalidate on every poll: the same URL can point to a restarted/reset chain.
        let chain_id = self.call(&endpoint.url, "eth_chainId", json!([])).await?;
        if quantity(&chain_id)? != endpoint.chain_id {
            return Err(Failure::Identity);
        }
        let genesis = self
            .call(&endpoint.url, "eth_getBlockByNumber", json!(["0x0", false]))
            .await?;
        let genesis = Head::parse(&genesis)?;
        if genesis.number != 0 || !genesis.hash.eq_ignore_ascii_case(&endpoint.genesis_hash) {
            return Err(Failure::Identity);
        }
        Ok(())
    }

    async fn collect_validated(
        &self,
        source: Source,
        endpoint: &Endpoint,
    ) -> Result<Data, Failure> {
        if source == Source::Queue {
            let value = self
                .call(&endpoint.url, "eez_getCrossChainQueueStatus", json!([]))
                .await?;
            Ok(Data::Queue(Queue::parse(value)?))
        } else {
            let value = self
                .call(
                    &endpoint.url,
                    "eth_getBlockByNumber",
                    json!(["latest", false]),
                )
                .await?;
            Ok(Data::Head(Head::parse(&value)?))
        }
    }
}
