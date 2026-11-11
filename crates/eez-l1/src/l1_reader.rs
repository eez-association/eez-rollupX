//! Read-only canonical L1 access for the Deriver.

use std::{sync::Arc, time::Duration};

use alloy_eips::BlockNumberOrTag;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::Filter;
use alloy_sol_types::SolEvent as _;

use crate::blob_source::BeaconBlobSource;
use crate::config::L1ReaderConfig;
use crate::error::{L1Error, L1Result};
use crate::scan::{BatchLogChunks, ScannedBatch, scan_next_batch_log_chunk};

alloy_sol_types::sol! {
    event RollupCreated(uint64 indexed rollupId, address indexed rollupContract, bytes32 initialRoot);
}

/// Validated bootstrap commitment and the containing L1 timestamp.
#[derive(Debug, Clone, Copy)]
pub struct RegistrationAnchor {
    pub commitment: crate::l1_canonical_head::CommitmentRef,
    pub timestamp: u64,
}

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

    /// Install the selected L1 profile before sharing the reader. The profile
    /// resolves historical rules independently of local posting preferences.
    #[must_use]
    pub fn with_blob_rules(mut self, rules: crate::BlobRuleSource) -> Self {
        self.blob_source =
            Arc::new(BeaconBlobSource::new(self.config.beacon_url.clone()).with_rules(rules));
        self
    }

    /// Authenticate the new network's immutable genesis reference through the
    /// same bounded, operator-configured canonical source used for settlement.
    pub async fn derivation_source(
        &self,
        schedule: eez_protocol::l2::Schedule,
    ) -> Result<
        crate::derivation::DerivationSource<alloy_provider::DynProvider>,
        crate::derivation::DerivationError,
    > {
        crate::derivation::DerivationSource::authenticate(self.build_provider().erased(), schedule)
            .await
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

    /// Resolve a timestamp from the same canonical source used for derivation,
    /// checking the hash as well as the height. Simulation providers may lag
    /// this source and cannot establish a settlement-policy anchor.
    pub async fn canonical_timestamp(&self, block: crate::L1BlockRef) -> L1Result<u64> {
        Self::canonical_timestamp_from(&self.build_provider(), block).await
    }

    async fn canonical_timestamp_from(
        provider: &impl Provider,
        block: crate::L1BlockRef,
    ) -> L1Result<u64> {
        let canonical = provider
            .get_block_by_number(block.number.into())
            .await
            .map_err(|error| L1Error::Provider(format!("canonical anchor header: {error}")))?
            .filter(|header| {
                header.header.hash == block.hash && header.header.number == block.number
            })
            .ok_or_else(|| {
                L1Error::Provider("canonical anchor header unavailable or replaced".into())
            })?;
        Ok(canonical.header.timestamp)
    }

    /// Resolve the initial anchor from the canonical registration event. The
    /// caller supplies its actual L2 genesis hash; neither deploy time nor
    /// process startup is a substitute for registration evidence.
    pub async fn registration_anchor(
        &self,
        genesis_hash: alloy_primitives::B256,
    ) -> L1Result<RegistrationAnchor> {
        let provider = self.build_provider();
        self.registration_anchor_from(&provider, genesis_hash).await
    }

    async fn registration_anchor_from(
        &self,
        provider: &impl Provider,
        genesis_hash: alloy_primitives::B256,
    ) -> L1Result<RegistrationAnchor> {
        let tip = provider
            .get_block_number()
            .await
            .map_err(|error| L1Error::Provider(format!("registration head: {error}")))?;
        let mut from = self.config.deploy_block;
        while from <= tip {
            let mut to = from
                .saturating_add(crate::scan::LOG_SCAN_CHUNK_BLOCKS - 1)
                .min(tip);
            let logs = loop {
                let filter = Filter::new()
                    .address(self.config.eez)
                    .event_signature(RollupCreated::SIGNATURE_HASH)
                    .topic1(alloy_primitives::B256::from(alloy_primitives::U256::from(
                        self.config.rollup_id,
                    )))
                    .from_block(from)
                    .to_block(to);
                match provider.get_logs(&filter).await {
                    Ok(logs) => break logs,
                    Err(error) => {
                        let error = L1Error::Provider(format!("registration logs: {error}"));
                        if to > from && crate::scan::is_range_too_wide(&error) {
                            to = from + (to - from) / 2;
                        } else {
                            return Err(error);
                        }
                    }
                }
            };
            if logs.len() > 1 {
                return Err(L1Error::Decode(
                    "multiple registrations for the configured rollup".into(),
                ));
            }
            if let Some(log) = logs.first() {
                let registration = RollupCreated::decode_log(&log.inner)
                    .map_err(|error| L1Error::Decode(format!("registration event: {error}")))?;
                if registration.rollupId != self.config.rollup_id
                    || registration.initialRoot != genesis_hash
                    || log.inner.address != self.config.eez
                    || log.removed
                {
                    return Err(L1Error::Decode(
                        "registration does not match the canonical L2 genesis".into(),
                    ));
                }
                let number = log
                    .block_number
                    .ok_or_else(|| L1Error::Decode("registration block number missing".into()))?;
                if !(from..=to).contains(&number) {
                    return Err(L1Error::Decode(
                        "registration is outside the canonical scan range".into(),
                    ));
                }
                let hash = log
                    .block_hash
                    .ok_or_else(|| L1Error::Decode("registration block hash missing".into()))?;
                let tx_hash = log.transaction_hash.ok_or_else(|| {
                    L1Error::Decode("registration transaction hash missing".into())
                })?;
                let tx_index = log.transaction_index.ok_or_else(|| {
                    L1Error::Decode("registration transaction index missing".into())
                })?;
                let block = provider
                    .get_block_by_number(number.into())
                    .await
                    .map_err(|error| L1Error::Provider(format!("registration block: {error}")))?
                    .ok_or_else(|| L1Error::Provider("registration block unavailable".into()))?;
                if block.header.hash != hash || block.header.number != number {
                    return Err(L1Error::Provider(
                        "registration block changed during bootstrap".into(),
                    ));
                }
                return Ok(RegistrationAnchor {
                    commitment: crate::l1_canonical_head::CommitmentRef {
                        l1: crate::L1BlockRef { number, hash },
                        tx_hash,
                        tx_index,
                        endpoint: crate::L2Endpoint {
                            number: 0,
                            hash: genesis_hash,
                        },
                    },
                    timestamp: block.header.timestamp,
                });
            }
            let Some(next) = to.checked_add(1) else {
                break;
            };
            from = next;
        }
        Err(L1Error::Provider(
            "bootstrap readiness: canonical rollup registration is unavailable".into(),
        ))
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

    /// Highest block number reported by the canonical L1 source.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] when the head is unreadable.
    pub async fn head_block_number(&self) -> L1Result<u64> {
        self.build_provider()
            .get_block_number()
            .await
            .map_err(|e| L1Error::Provider(format!("get_block_number: {e}")))
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

    /// Registry whose events this reader authenticates for reconstruction.
    pub fn eez(&self) -> alloy_primitives::Address {
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
    use alloy_primitives::{Address, B256, U64};
    use alloy_transport::mock::Asserter;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

    use super::*;

    fn policy_reader() -> L1Reader {
        L1Reader::new(L1ReaderConfig {
            rpc_url: "http://127.0.0.1:1".parse().unwrap(),
            beacon_url: None,
            eez: Address::repeat_byte(1),
            rollup_id: 7,
            deploy_block: 5,
        })
    }

    fn registration() -> alloy_rpc_types_eth::Log {
        alloy_rpc_types_eth::Log {
            inner: alloy_primitives::Log {
                address: Address::repeat_byte(1),
                data: RollupCreated {
                    rollupId: 7,
                    rollupContract: Address::repeat_byte(2),
                    initialRoot: B256::repeat_byte(3),
                }
                .encode_log_data(),
            },
            block_number: Some(5),
            block_hash: Some(B256::repeat_byte(4)),
            transaction_hash: Some(B256::repeat_byte(5)),
            transaction_index: Some(3),
            ..Default::default()
        }
    }

    fn anchor_block() -> alloy_rpc_types_eth::Block {
        let mut block: alloy_rpc_types_eth::Block = Default::default();
        block.header.hash = B256::repeat_byte(4);
        block.header.inner.number = 5;
        block.header.inner.timestamp = 1_000;
        block
    }

    #[tokio::test]
    async fn registration_bootstrap_uses_canonical_event_identity_and_timestamp() {
        let asserter = Asserter::new();
        asserter.push_success(&U64::from(5));
        asserter.push_success(&vec![registration()]);
        asserter.push_success(&anchor_block());
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let anchor = policy_reader()
            .registration_anchor_from(&provider, B256::repeat_byte(3))
            .await
            .unwrap();
        assert_eq!(anchor.timestamp, 1_000);
        assert_eq!(
            anchor.commitment.l1,
            crate::L1BlockRef {
                number: 5,
                hash: B256::repeat_byte(4)
            }
        );
        assert_eq!(anchor.commitment.tx_hash, B256::repeat_byte(5));
        assert_eq!(anchor.commitment.tx_index, 3);
        assert_eq!(
            anchor.commitment.endpoint,
            crate::L2Endpoint {
                number: 0,
                hash: B256::repeat_byte(3)
            }
        );
    }

    #[tokio::test]
    async fn missing_registration_is_readiness_failure_and_not_a_new_clock() {
        let asserter = Asserter::new();
        asserter.push_success(&U64::from(5));
        asserter.push_success(&Vec::<alloy_rpc_types_eth::Log>::new());
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let error = policy_reader()
            .registration_anchor_from(&provider, B256::repeat_byte(3))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("bootstrap readiness"), "{error}");
    }

    #[tokio::test]
    async fn registration_rejects_wrong_genesis_identity_removed_and_incomplete_evidence() {
        let mut cases = Vec::new();
        let mut log = registration();
        log.removed = true;
        cases.push(vec![log]);
        let mut log = registration();
        log.inner.address = Address::repeat_byte(9);
        cases.push(vec![log]);
        let mut log = registration();
        log.transaction_index = None;
        cases.push(vec![log]);
        let mut log = registration();
        log.transaction_hash = None;
        cases.push(vec![log]);
        let mut log = registration();
        log.block_hash = None;
        cases.push(vec![log]);
        let mut log = registration();
        log.block_number = Some(6);
        cases.push(vec![log]);
        let mut log = registration();
        log.inner.data = RollupCreated {
            rollupId: 8,
            rollupContract: Address::repeat_byte(2),
            initialRoot: B256::repeat_byte(3),
        }
        .encode_log_data();
        cases.push(vec![log]);
        let mut log = registration();
        log.inner.data = RollupCreated {
            rollupId: 7,
            rollupContract: Address::repeat_byte(2),
            initialRoot: B256::repeat_byte(9),
        }
        .encode_log_data();
        cases.push(vec![log]);
        cases.push(vec![registration(), registration()]);
        for logs in cases {
            let asserter = Asserter::new();
            asserter.push_success(&U64::from(5));
            asserter.push_success(&logs);
            let provider = ProviderBuilder::new().connect_mocked_client(asserter);
            assert!(
                policy_reader()
                    .registration_anchor_from(&provider, B256::repeat_byte(3))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn canonical_anchor_rejects_same_height_reorg_and_unavailable_headers() {
        let mut replaced = anchor_block();
        replaced.header.hash = B256::repeat_byte(9);
        let mut wrong_height = anchor_block();
        wrong_height.header.inner.number = 6;
        for block in [None, Some(replaced), Some(wrong_height)] {
            let asserter = Asserter::new();
            asserter.push_success(&block);
            let provider = ProviderBuilder::new().connect_mocked_client(asserter);
            assert!(
                L1Reader::canonical_timestamp_from(
                    &provider,
                    crate::L1BlockRef {
                        number: 5,
                        hash: B256::repeat_byte(4)
                    }
                )
                .await
                .is_err()
            );
            let asserter = Asserter::new();
            asserter.push_success(&U64::from(5));
            asserter.push_success(&vec![registration()]);
            asserter.push_success(&block);
            let provider = ProviderBuilder::new().connect_mocked_client(asserter);
            assert!(
                policy_reader()
                    .registration_anchor_from(&provider, B256::repeat_byte(3))
                    .await
                    .is_err()
            );
        }
        let asserter = Asserter::new();
        asserter.push_success(&anchor_block());
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        assert_eq!(
            L1Reader::canonical_timestamp_from(
                &provider,
                crate::L1BlockRef {
                    number: 5,
                    hash: B256::repeat_byte(4)
                }
            )
            .await
            .unwrap(),
            1_000
        );
    }

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
