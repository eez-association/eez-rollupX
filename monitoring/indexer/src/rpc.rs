use anyhow::{Context, Result, anyhow, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

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
            self.block(0).await?.hash == config.genesis_hash,
            "HTTP genesis mismatch"
        );
        Ok(())
    }
}

fn result(value: Value) -> Result<Value> {
    ensure!(
        value["jsonrpc"] == "2.0" && value["id"] == 1 && value.get("error").is_none(),
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
        socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await?;
        let value = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match socket.next().await.context("WS closed")?? {
                    Message::Text(text) => break result(serde_json::from_str(&text)?),
                    Message::Close(_) => return Err(anyhow!("WS closed")),
                    _ => continue,
                }
            }
        })
        .await??;
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
