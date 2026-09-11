//! Read-only canonical L1 access for the Deriver.

use std::{sync::Arc, time::Duration};

use alloy_eips::BlockNumberOrTag;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::Filter;

use crate::blob_source::BeaconBlobSource;
use crate::config::L1ReaderConfig;
use crate::error::{L1Error, L1Result};
use crate::scan::{BatchLogChunks, ScannedBatch, scan_next_batch_log_chunk};

/// Read-only L1 client for historical batch scans and canonicality checks.
/// Cheaply [`Clone`]able.
#[derive(Clone)]
pub struct L1Reader {
    config: Arc<L1ReaderConfig>,
    blob_source: Arc<BeaconBlobSource>,
    /// Bound the entire HTTP request, including reading the response body, so
    /// unavailable source data reaches the caller's existing recovery policy.
    request_timeout: Duration,
}

/// What the L1 source can currently serve. See [`L1Reader::readiness`].
#[derive(Debug, Clone, Copy)]
pub struct L1Readiness {
    /// Highest block number the source will serve.
    pub head_block_number: u64,
    /// Advisory: an endpoint that refuses `eth_syncing` reads as not-syncing.
    pub syncing: bool,
}

impl std::fmt::Debug for L1Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L1Reader")
            .field("config", &self.config)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl L1Reader {
    /// Build a read-only L1 client from its config.
    #[must_use]
    pub fn new(config: L1ReaderConfig) -> Self {
        let blob_source = Arc::new(BeaconBlobSource::new(config.beacon_url.clone()));
        Self {
            config: Arc::new(config),
            blob_source,
            request_timeout: Duration::from_secs(30),
        }
    }

    /// Creates bounded `BatchPosted` log chunks from `from_block` to the
    /// current L1 head. Call [`Self::next_batch_log_chunk`] to consume it.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn batch_log_chunks(&self, from_block: u64) -> L1Result<BatchLogChunks> {
        let provider = self.build_provider();
        let latest = provider
            .get_block_number()
            .await
            .map_err(|e| L1Error::Provider(format!("get_block_number: {e}")))?;
        Ok(BatchLogChunks::new(from_block, latest))
    }

    /// Scans and returns the next `BatchPosted` log chunk, or `None` when
    /// the chunks are exhausted.
    ///
    /// # Errors
    ///
    /// - [`L1Error::Provider`] on RPC failure (log fetch, tx fetch).
    /// - [`L1Error::SourceIncomplete`] when a canonical batch tx is not
    ///   served by the L1 source yet — retryable once the source syncs.
    pub async fn next_batch_log_chunk(
        &self,
        chunks: &mut BatchLogChunks,
    ) -> L1Result<Option<Vec<ScannedBatch>>> {
        let provider = self.build_provider();
        let Some(mut scanned) =
            scan_next_batch_log_chunk(&provider, self.config.eez, self.config.rollup_id, chunks)
                .await?
        else {
            return Ok(None);
        };
        self.blob_source
            .hydrate_scanned_batches(&provider, self.config.rollup_id, &mut scanned)
            .await?;
        Ok(Some(scanned))
    }

    /// Hash of the canonical L1 block at `number`, or `None` if none. Used by
    /// the Deriver's resync to check whether an indexed batch is still canonical.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn canonical_l1_hash(&self, number: u64) -> L1Result<Option<alloy_primitives::B256>> {
        let provider = self.build_provider();
        Ok(provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| L1Error::Provider(format!("get_block_by_number({number}): {e}")))?
            .map(|b| b.header.hash))
    }

    /// Whether the source serves `block`'s header, logs, and tx bodies — the
    /// three things the batch scan reads.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn serves_history(&self, block: u64) -> L1Result<bool> {
        let provider = self.build_provider();
        let header = provider
            .get_block_by_number(BlockNumberOrTag::Number(block))
            .await
            .map_err(|e| L1Error::Provider(format!("get_block_by_number({block}): {e}")))?;
        if header.is_none() {
            return Ok(false);
        }
        let filter = Filter::new()
            .from_block(block)
            .to_block(block)
            .address(self.config.eez);
        // Result discarded: this asks whether eth_getLogs works at all (some
        // endpoints restrict it), not whether the block has events.
        provider
            .get_logs(&filter)
            .await
            .map_err(|e| L1Error::Provider(format!("get_logs probe at {block}: {e}")))?;
        // The scan reads bodies only by (block hash, index), so a source that
        // prunes them passes the probes above and stalls boot later.
        // `None` here just means an empty block.
        let header = header.expect("checked above");
        provider
            .get_transaction_by_block_hash_and_index(header.header.hash, 0)
            .await
            .map_err(|e| L1Error::Provider(format!("tx-by-index probe at {block}: {e}")))?;
        Ok(true)
    }

    /// Chain id the configured L1 RPC serves.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn chain_id(&self) -> L1Result<u64> {
        self.build_provider()
            .get_chain_id()
            .await
            .map_err(|e| L1Error::Provider(format!("eth_chainId: {e}")))
    }

    /// L1 head, and whether the node reports itself syncing.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] when the head is unreadable.
    pub async fn readiness(&self) -> L1Result<L1Readiness> {
        let provider = self.build_provider();
        let head = provider
            .get_block_number()
            .await
            .map_err(|e| L1Error::Provider(format!("get_block_number: {e}")))?;
        let syncing = provider
            .syncing()
            .await
            .is_ok_and(|s| !matches!(s, alloy_rpc_types_eth::SyncStatus::None));
        Ok(L1Readiness {
            head_block_number: head,
            syncing,
        })
    }

    /// The L1 source's finalized block `(number, hash)`, or `None` when the
    /// source reports no finalized block yet (fresh embedded chiado before
    /// the CL's first FCU; dev chains without a CL).
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn finalized_block(&self) -> L1Result<Option<(u64, alloy_primitives::B256)>> {
        let provider = self.build_provider();
        Ok(provider
            .get_block_by_number(BlockNumberOrTag::Finalized)
            .await
            .map_err(|e| L1Error::Provider(format!("get_block(finalized): {e}")))?
            .map(|b| (b.header.number, b.header.hash)))
    }

    pub(crate) fn rpc_url(&self) -> url::Url {
        self.config.rpc_url.clone()
    }

    pub(crate) fn eez(&self) -> alloy_primitives::Address {
        self.config.eez
    }

    pub(crate) fn rollup_id(&self) -> u64 {
        self.config.rollup_id
    }

    fn build_provider(&self) -> impl Provider + use<> {
        ProviderBuilder::new()
            .disable_recommended_fillers()
            .with_reqwest(self.config.rpc_url.clone(), |builder| {
                builder
                    .timeout(self.request_timeout)
                    .build()
                    .expect("L1 reader HTTP client configuration is valid")
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

    use super::*;

    #[derive(Clone, Copy)]
    enum Response {
        StalledHeaders,
        StalledBody,
        Complete,
    }

    // Exercise the real HTTP transport, including a response that stalls after
    // its headers. A timeout around connection establishment alone is not enough.
    async fn canonical_read(response: Response) -> L1Result<Option<alloy_primitives::B256>> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(AtomicBool::new(false));
        let server_received = Arc::clone(&received);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let json = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0, "request ended before its JSON body");
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    && let Ok(json) =
                        serde_json::from_slice::<serde_json::Value>(&request[end + 4..])
                {
                    break json;
                }
            };
            server_received.store(true, Ordering::SeqCst);
            match response {
                Response::StalledHeaders => {}
                Response::StalledBody => {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                        .await
                        .unwrap();
                }
                Response::Complete => {
                    let body =
                        serde_json::json!({"jsonrpc": "2.0", "id": json["id"], "result": null})
                            .to_string();
                    let header =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    stream.write_all(header.as_bytes()).await.unwrap();
                    stream.write_all(body.as_bytes()).await.unwrap();
                    return;
                }
            }
            std::future::pending::<()>().await;
        });
        let mut reader = L1Reader::new(L1ReaderConfig {
            rpc_url: format!("http://{address}").parse().unwrap(),
            beacon_url: None,
            eez: alloy_primitives::Address::ZERO,
            rollup_id: 1,
            deploy_block: 0,
        });
        reader.request_timeout = Duration::from_millis(100);
        let result = timeout(Duration::from_secs(2), reader.canonical_l1_hash(100)).await;
        server.abort();
        assert!(
            received.load(Ordering::SeqCst),
            "the fixture must receive the RPC"
        );
        result.expect("L1 reader must bound both request and response-body waits")
    }

    #[tokio::test]
    async fn canonical_read_times_out_when_server_never_sends_headers() {
        assert!(matches!(
            canonical_read(Response::StalledHeaders).await,
            Err(L1Error::Provider(_))
        ));
    }

    #[tokio::test]
    async fn canonical_read_times_out_when_response_body_stalls() {
        assert!(matches!(
            canonical_read(Response::StalledBody).await,
            Err(L1Error::Provider(_))
        ));
    }

    #[tokio::test]
    async fn successful_missing_block_response_remains_distinct_from_transport_failure() {
        assert!(canonical_read(Response::Complete).await.unwrap().is_none());
    }
}
