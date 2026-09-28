use alloy_primitives::{B256, Bytes, keccak256};
use anyhow::{Context, Result, anyhow, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::{
    config::Config,
    model::{Block, hash, number},
};

#[derive(Clone)]
pub struct Rpc {
    client: reqwest::Client,
    url: String,
}

impl Rpc {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?,
            url: url.into(),
        })
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut response = self
            .client
            .post(&self.url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .map_err(|_| anyhow!("HTTP RPC transport failed"))?;
        ensure!(response.status().is_success(), "HTTP RPC status failure");
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!("HTTP RPC body failed"))?
        {
            ensure!(
                body.len() + chunk.len() <= 16 * 1024 * 1024,
                "RPC response exceeds 16 MiB"
            );
            body.extend_from_slice(&chunk);
        }
        result(serde_json::from_slice(&body)?)
    }
    pub async fn block(&self, number: u64) -> Result<Block> {
        let block = Block::parse(
            &self
                .call(
                    "eth_getBlockByNumber",
                    json!([format!("0x{number:x}"), false]),
                )
                .await?,
        )?;
        ensure!(block.number == number, "RPC returned wrong block height");
        Ok(block)
    }
    pub async fn identity(&self, config: &Config) -> Result<()> {
        ensure!(
            number(&self.call("eth_chainId", json!([])).await?)? == config.chain_id,
            "HTTP chain ID mismatch"
        );
        ensure!(
            self.genesis_hash().await? == config.genesis_hash,
            "HTTP genesis mismatch"
        );
        Ok(())
    }

    /// Verify retained genesis identity even when Reth prunes the full-block RPC.
    /// Only its explicit pruning error permits the raw-header fallback; callers
    /// must still compare the computed hash with their configured chain identity.
    pub async fn genesis_hash(&self) -> Result<B256> {
        match self.block(0).await {
            Ok(block) => Ok(block.hash),
            Err(error) if error.is::<PrunedHistory>() => {
                raw_header_hash(&self.call("debug_getRawHeader", json!(["0x0"])).await?)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug)]
struct PrunedHistory;
impl std::fmt::Display for PrunedHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RPC history pruned")
    }
}
impl std::error::Error for PrunedHistory {}

fn raw_header_hash(value: &Value) -> Result<B256> {
    let text = value.as_str().context("missing raw genesis header")?;
    // Hashing the exact bounded bytes against the configured genesis commits to
    // every header field. No remote claimed hash or decoded metadata is trusted.
    ensure!(
        text.len() > 2 && text.len() <= 2 + 8192 * 2,
        "invalid raw genesis header size"
    );
    let bytes: Bytes = text.parse()?;
    Ok(keccak256(bytes))
}

fn result(value: Value) -> Result<Value> {
    ensure!(
        value["jsonrpc"] == "2.0" && value["id"] == 1,
        "invalid or failed RPC response"
    );
    if value["error"]["code"].as_i64() == Some(4444) && value.get("result").is_none() {
        return Err(PrunedHistory.into());
    }
    ensure!(
        value.get("error").is_none(),
        "invalid or failed RPC response"
    );
    value.get("result").cloned().context("missing RPC result")
}

// WS is a wake-up path only. Persistent scanning also runs periodically and
// always fetches every missing block, even if notifications were lost/coalesced.
pub async fn notifications(config: Config, wake: watch::Sender<u64>) {
    let mut delay = 1;
    loop {
        match subscribe(&config, &wake).await {
            Ok(()) => return,
            Err(_) => tracing::warn!("L1 WebSocket disconnected or rejected; polling continues"),
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

async fn subscribe(config: &Config, wake: &watch::Sender<u64>) -> Result<()> {
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&config.ws_url),
    )
    .await?
    .map_err(|_| anyhow!("WS connection failed"))?;
    for (method, params, expected) in [
        ("eth_chainId", json!([]), 0),
        ("eth_getBlockByNumber", json!(["0x0", false]), 1),
        ("eth_subscribe", json!(["newHeads"]), 2),
    ] {
        let response = ws_call(&mut socket, method, params).await;
        let value = if expected == 1 {
            let genesis = match response {
                Ok(value) => hash(&value["hash"])?,
                Err(error) if error.is::<PrunedHistory>() => raw_header_hash(
                    &ws_call(&mut socket, "debug_getRawHeader", json!(["0x0"])).await?,
                )?,
                Err(error) => return Err(error),
            };
            json!({"hash":genesis})
        } else {
            response?
        };
        match expected {
            0 => ensure!(number(&value)? == config.chain_id, "WS chain ID mismatch"),
            1 => ensure!(
                hash(&value["hash"])? == config.genesis_hash,
                "WS genesis mismatch"
            ),
            _ => {
                let subscription = value
                    .as_str()
                    .context("missing subscription ID")?
                    .to_owned();
                tracing::info!("L1 WebSocket subscribed");
                wake.send_modify(|n| *n = n.wrapping_add(1));
                loop {
                    // A silent connection is periodically replaced; scanner polling
                    // continues independently while this task reconnects.
                    let msg = tokio::time::timeout(Duration::from_secs(60), socket.next())
                        .await?
                        .context("WS closed")??;
                    match msg {
                        Message::Text(text) => {
                            let v: Value = serde_json::from_str(&text)?;
                            if v["method"] == "eth_subscription"
                                && v["params"]["subscription"] == subscription
                            {
                                number(&v["params"]["result"]["number"])?;
                                wake.send_modify(|n| *n = n.wrapping_add(1));
                            }
                        }
                        Message::Close(_) => return Err(anyhow!("WS closed")),
                        Message::Ping(_) => socket.flush().await?,
                        _ => {}
                    }
                    if wake.is_closed() {
                        return Ok(());
                    }
                }
            }
        }
    }
    unreachable!("subscription enters notification loop")
}

async fn ws_call(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    method: &str,
    params: Value,
) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await?;
        loop {
            match socket.next().await.context("WS closed")?? {
                Message::Text(text) => break result(serde_json::from_str(&text)?),
                Message::Close(_) => return Err(anyhow!("WS closed")),
                _ => continue,
            }
        }
    })
    .await?
}
