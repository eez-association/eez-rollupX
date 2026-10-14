//! Authenticated EIP-4844 blob retrieval for historical and live derivation.
//!
//! Execution transactions contain only versioned hashes. Blob bodies live on
//! the consensus layer, so derivation downloads the slot's sidecars from a
//! beacon REST endpoint and recomputes every body's KZG versioned hash before
//! accepting it.

use std::{collections::HashMap, sync::Arc};

use alloy_eips::eip4844::kzg_to_versioned_hash;
use alloy_primitives::{B256, Bytes, U256, hex};
use alloy_provider::Provider;
use alloy_sol_types::SolCall as _;
use eez_blob_codec::{
    Blob, BlobLimits, MAX_BLOBS_PER_TRANSACTION, blob_kzg_commitment_from_bytes,
    blob_versioned_hash_from_bytes, decode_native_payload,
};
use eez_protocol::abi::postAndVerifyBatchCall;
use serde_json::Value;
use tokio::sync::OnceCell;
use url::Url;

use crate::{
    error::{L1Error, L1Result},
    scan::ScannedBatch,
};

#[derive(Debug, Clone, Copy)]
struct BeaconTiming {
    genesis_time: u64,
    seconds_per_slot: u64,
}

#[derive(Debug, Clone)]
struct AvailableBlob {
    versioned_hash: B256,
    bytes: Bytes,
}

/// Consensus-layer source shared by repeated scans. Chain timing is immutable
/// and fetched lazily only after the first blob-backed batch is observed.
pub(crate) struct BeaconBlobSource {
    beacon_url: Option<Url>,
    client: reqwest::Client,
    timing: OnceCell<BeaconTiming>,
    rules: Option<BlobRuleSource>,
}

/// Immutable operator-selected L1 fork schedule. Historical admission uses the
/// actual containing block's timestamp; outgoing operator caps are not inputs.
pub type BlobRuleSource = Arc<dyn Fn(u64) -> Option<alloy_eips::eip7840::BlobParams> + Send + Sync>;

impl BeaconBlobSource {
    pub(crate) fn new(beacon_url: Option<Url>) -> Self {
        Self {
            beacon_url: beacon_url.map(normalize_base_url),
            client: reqwest::Client::new(),
            timing: OnceCell::new(),
            rules: None,
        }
    }

    pub(crate) fn with_rules(mut self, rules: BlobRuleSource) -> Self {
        self.rules = Some(rules);
        self
    }

    fn historical_limit(&self, timestamp: u64) -> L1Result<usize> {
        let Some(rules) = &self.rules else {
            return Ok(MAX_BLOBS_PER_TRANSACTION);
        };
        let params = rules(timestamp).ok_or_else(|| {
            L1Error::Config(format!(
                "L1 blob rules unavailable at containing timestamp {timestamp}"
            ))
        })?;
        let maximum = BlobLimits::from_params(params)
            .and_then(|limits| limits.outgoing(0, u64::MAX, None))
            .map_err(|error| L1Error::Config(format!("containing L1 blob rules: {error}")))?;
        usize::try_from(maximum)
            .map_err(|_| L1Error::Config("L1 blob allowance exceeds platform capacity".into()))
    }

    /// Populate every batch's authenticated operations payload from its
    /// mandatory native blobs. The scanner excludes posts whose registry event
    /// does not verify our rollup; those may use a different DA profile.
    pub(crate) async fn hydrate_scanned_batches(
        &self,
        provider: &impl Provider,
        rollup_id: u64,
        batches: &mut [ScannedBatch],
    ) -> L1Result<()> {
        let mut sidecars_by_block: HashMap<B256, (usize, Vec<AvailableBlob>)> = HashMap::new();

        for scanned in batches {
            let posted =
                postAndVerifyBatchCall::abi_decode(&scanned.post_batch_input).map_err(|error| {
                    L1Error::Decode(format!(
                        "decode postAndVerifyBatch({}): {error}",
                        scanned.tx_hash
                    ))
                })?;
            let indices = &posted.batch.blobIndices;
            if self.beacon_url.is_none() {
                return Err(L1Error::Config(
                    "EEZ_L1_BEACON_URL is required to derive native blob batches".into(),
                ));
            }

            if let std::collections::hash_map::Entry::Vacant(entry) =
                sidecars_by_block.entry(scanned.l1_block_hash)
            {
                let block = provider
                    .get_block_by_hash(scanned.l1_block_hash)
                    .await
                    .map_err(|error| {
                        L1Error::Provider(format!(
                            "get_block_by_hash({}): {error}",
                            scanned.l1_block_hash
                        ))
                    })?
                    .ok_or_else(|| L1Error::SourceIncomplete {
                        block: scanned.l1_block_number,
                        tx_hash: scanned.tx_hash,
                        detail: "execution block body is not available while retrieving blobs"
                            .into(),
                    })?;
                if block.header.hash != scanned.l1_block_hash
                    || block.header.number != scanned.l1_block_number
                {
                    return Err(L1Error::SourceIncomplete {
                        block: scanned.l1_block_number,
                        tx_hash: scanned.tx_hash,
                        detail: "execution block metadata changed during blob retrieval; retry"
                            .into(),
                    });
                }
                let maximum = self.historical_limit(block.header.inner.timestamp)?;
                validate_blob_profile_with_limit(
                    scanned.tx_hash,
                    &posted.batch.callData,
                    indices,
                    &scanned.transaction_blob_hashes,
                    maximum,
                )?;
                let available = self
                    .fetch_slot_blobs(
                        block.header.inner.timestamp,
                        scanned.l1_block_number,
                        scanned.tx_hash,
                    )
                    .await?;
                entry.insert((maximum, available));
            }

            let (maximum, available) = sidecars_by_block
                .get(&scanned.l1_block_hash)
                .expect("inserted immediately above");
            // Every transaction in the block has its own carrier and indices.
            validate_blob_profile_with_limit(
                scanned.tx_hash,
                &posted.batch.callData,
                indices,
                &scanned.transaction_blob_hashes,
                *maximum,
            )?;
            let mut transaction_blobs = Vec::with_capacity(scanned.transaction_blob_hashes.len());
            for expected_hash in &scanned.transaction_blob_hashes {
                let Some(found) = available
                    .iter()
                    .find(|candidate| candidate.versioned_hash == *expected_hash)
                else {
                    return Err(L1Error::SourceIncomplete {
                        block: scanned.l1_block_number,
                        tx_hash: scanned.tx_hash,
                        detail: format!(
                            "beacon blob response does not contain transaction blob {expected_hash}"
                        ),
                    });
                };
                let blob_index = transaction_blobs.len();
                let blob = Blob::try_from_slice(blob_index, &found.bytes).map_err(|error| {
                    L1Error::Decode(format!(
                        "decode authenticated native blob payload for {}: {error}",
                        scanned.tx_hash
                    ))
                })?;
                transaction_blobs.push(blob);
            }

            let native = decode_native_payload(&transaction_blobs, rollup_id).map_err(|error| {
                L1Error::Decode(format!(
                    "decode authenticated native blob payload for {}: {error}",
                    scanned.tx_hash
                ))
            })?;
            let semantic_transactions = eez_protocol::blob_semantics::decode_semantic_transactions(
                &native.semantic_messages,
            )
            .map_err(|error| {
                L1Error::Decode(format!(
                    "decode authenticated blob semantics for {}: {error}",
                    scanned.tx_hash
                ))
            })?;
            scanned.operations_payload = native.operations;
            scanned.semantic_transactions = semantic_transactions.into();
        }

        Ok(())
    }

    async fn fetch_slot_blobs(
        &self,
        execution_timestamp: u64,
        block: u64,
        tx_hash: B256,
    ) -> L1Result<Vec<AvailableBlob>> {
        let timing = self.timing().await?;
        let since_genesis = execution_timestamp
            .checked_sub(timing.genesis_time)
            .ok_or_else(|| {
                L1Error::Config(format!(
                    "execution timestamp {execution_timestamp} precedes configured beacon genesis {}; EEZ_L1_BEACON_URL is likely on a different chain",
                    timing.genesis_time
                ))
            })?;
        if !since_genesis.is_multiple_of(timing.seconds_per_slot) {
            return Err(L1Error::Config(format!(
                "execution timestamp {execution_timestamp} is not aligned to configured beacon genesis {} and {} seconds/slot; EEZ_L1_BEACON_URL is likely on a different chain",
                timing.genesis_time, timing.seconds_per_slot
            )));
        }
        let slot = since_genesis / timing.seconds_per_slot;
        let context = Some((block, tx_hash, slot));
        let blobs_path = format!("eth/v1/beacon/blobs/{slot}");
        if let Some(value) = self.get_json_if_present(&blobs_path, context).await? {
            return parse_blobs(&value).map_err(|detail| {
                L1Error::Decode(format!(
                    "invalid beacon blobs response for slot {slot}: {detail}"
                ))
            });
        }

        // Beacon API v4 replaced blob_sidecars with blobs for Fulu/PeerDAS.
        // Keep the deprecated endpoint as a compatibility fallback for older
        // pre-Fulu clients, but never prefer it on a server implementing the
        // standard full-blob endpoint.
        let value = self
            .get_json(&format!("eth/v1/beacon/blob_sidecars/{slot}"), context)
            .await?;
        parse_sidecars(&value).map_err(|detail| {
            L1Error::Decode(format!(
                "invalid legacy beacon blob_sidecars response for slot {slot}: {detail}"
            ))
        })
    }

    async fn timing(&self) -> L1Result<&BeaconTiming> {
        self.timing
            .get_or_try_init(|| async {
                let genesis = self.get_json("eth/v1/beacon/genesis", None).await?;
                let spec = self.get_json("eth/v1/config/spec", None).await?;
                let genesis_time = json_u64(&genesis, "/data/genesis_time")?;
                let seconds_per_slot = json_u64(&spec, "/data/SECONDS_PER_SLOT")?;
                if seconds_per_slot == 0 {
                    return Err(L1Error::Provider(
                        "beacon spec returned SECONDS_PER_SLOT=0".into(),
                    ));
                }
                Ok(BeaconTiming {
                    genesis_time,
                    seconds_per_slot,
                })
            })
            .await
    }

    async fn get_json(
        &self,
        path: &str,
        sidecar_context: Option<(u64, B256, u64)>,
    ) -> L1Result<Value> {
        let base = self.beacon_url.as_ref().ok_or_else(|| {
            L1Error::Config("EEZ_L1_BEACON_URL is required to retrieve native blobs".into())
        })?;
        let endpoint = base
            .join(path)
            .map_err(|error| L1Error::Config(format!("EEZ_L1_BEACON_URL: {error}")))?;
        let response = self
            .client
            .get(endpoint.clone())
            .send()
            .await
            .map_err(|error| L1Error::Provider(format!("beacon GET {endpoint}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let response_detail = response.text().await.unwrap_or_default();
            if let Some((block, tx_hash, slot)) = sidecar_context {
                return Err(L1Error::SourceIncomplete {
                    block,
                    tx_hash,
                    detail: format!(
                        "beacon blob sidecars for slot {slot} returned HTTP {status}: {response_detail}"
                    ),
                });
            }
            return Err(L1Error::Provider(format!(
                "beacon GET {endpoint} returned HTTP {status}: {response_detail}"
            )));
        }
        response.json::<Value>().await.map_err(|error| {
            L1Error::Provider(format!("decode beacon JSON from {endpoint}: {error}"))
        })
    }

    /// Fetch a fork-aware endpoint, returning `None` only when the beacon node
    /// does not expose it. This lets pre-Fulu clients fall back to the legacy
    /// sidecar API without masking transport, decoding, or server failures.
    async fn get_json_if_present(
        &self,
        path: &str,
        sidecar_context: Option<(u64, B256, u64)>,
    ) -> L1Result<Option<Value>> {
        let base = self.beacon_url.as_ref().ok_or_else(|| {
            L1Error::Config("EEZ_L1_BEACON_URL is required to retrieve native blobs".into())
        })?;
        let endpoint = base
            .join(path)
            .map_err(|error| L1Error::Config(format!("EEZ_L1_BEACON_URL: {error}")))?;
        let response = self
            .client
            .get(endpoint.clone())
            .send()
            .await
            .map_err(|error| L1Error::Provider(format!("beacon GET {endpoint}: {error}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return Ok(None);
        }
        if !status.is_success() {
            let response_detail = response.text().await.unwrap_or_default();
            if let Some((block, tx_hash, slot)) = sidecar_context {
                return Err(L1Error::SourceIncomplete {
                    block,
                    tx_hash,
                    detail: format!(
                        "beacon blobs for slot {slot} returned HTTP {status}: {response_detail}"
                    ),
                });
            }
            return Err(L1Error::Provider(format!(
                "beacon GET {endpoint} returned HTTP {status}: {response_detail}"
            )));
        }
        response.json::<Value>().await.map(Some).map_err(|error| {
            L1Error::Provider(format!("decode beacon JSON from {endpoint}: {error}"))
        })
    }
}

#[cfg(test)]
fn validate_blob_profile(
    tx_hash: B256,
    abi_call_data: &Bytes,
    blob_indices: &[U256],
    transaction_blob_hashes: &[B256],
) -> L1Result<()> {
    validate_blob_profile_with_limit(
        tx_hash,
        abi_call_data,
        blob_indices,
        transaction_blob_hashes,
        MAX_BLOBS_PER_TRANSACTION,
    )
}

fn validate_blob_profile_with_limit(
    tx_hash: B256,
    abi_call_data: &Bytes,
    blob_indices: &[U256],
    transaction_blob_hashes: &[B256],
    maximum: usize,
) -> L1Result<()> {
    if !abi_call_data.is_empty() {
        return Err(L1Error::Decode(format!(
            "blob-backed batch {tx_hash} must have empty callData"
        )));
    }
    if transaction_blob_hashes.is_empty() {
        return Err(L1Error::Decode(format!(
            "batch {tx_hash} selects blobs but its transaction has no EIP-4844 versioned hashes"
        )));
    }
    if transaction_blob_hashes.len() > maximum {
        return Err(L1Error::Decode(format!(
            "batch {tx_hash} contains {} transaction blobs; containing fork maximum is {maximum}",
            transaction_blob_hashes.len()
        )));
    }
    let exact_indices = blob_indices.len() == transaction_blob_hashes.len()
        && blob_indices
            .iter()
            .enumerate()
            .all(|(position, index)| *index == U256::from(position));
    if !exact_indices {
        return Err(L1Error::Decode(format!(
            "batch {tx_hash} blobIndices must select every transaction blob exactly once in order"
        )));
    }
    Ok(())
}

fn parse_sidecars(value: &Value) -> Result<Vec<AvailableBlob>, String> {
    let sidecars = value
        .pointer("/data")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing data array".to_owned())?;
    let mut available = Vec::with_capacity(sidecars.len());
    for (position, sidecar) in sidecars.iter().enumerate() {
        let blob_hex = sidecar
            .get("blob")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("data[{position}].blob is missing or not a string"))?;
        let commitment_hex = sidecar
            .get("kzg_commitment")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("data[{position}].kzg_commitment is missing or not a string"))?;
        let blob_bytes = decode_hex(blob_hex, &format!("data[{position}].blob"))?;
        let commitment = decode_hex(commitment_hex, &format!("data[{position}].kzg_commitment"))?;
        if commitment.len() != 48 {
            return Err(format!(
                "data[{position}].kzg_commitment has {} bytes, expected 48",
                commitment.len()
            ));
        }
        let computed_commitment = blob_kzg_commitment_from_bytes(&blob_bytes)
            .map_err(|error| format!("data[{position}].blob KZG commitment: {error}"))?;
        let computed_hash = kzg_to_versioned_hash(&computed_commitment);
        let advertised_hash = kzg_to_versioned_hash(&commitment);
        if computed_hash != advertised_hash {
            return Err(format!(
                "data[{position}] blob does not match its advertised KZG commitment"
            ));
        }
        available.push(AvailableBlob {
            versioned_hash: computed_hash,
            bytes: blob_bytes.into(),
        });
    }
    Ok(available)
}

/// Parse the post-Fulu Beacon API `getBlobs` response. The response carries
/// only blob bodies; recomputing each KZG commitment and versioned hash
/// authenticates it against the hashes in the canonical execution transaction.
fn parse_blobs(value: &Value) -> Result<Vec<AvailableBlob>, String> {
    let blobs = value
        .pointer("/data")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing data array".to_owned())?;
    let mut available = Vec::with_capacity(blobs.len());
    for (position, item) in blobs.iter().enumerate() {
        let blob_hex = item
            .as_str()
            .ok_or_else(|| format!("data[{position}] is not a blob string"))?;
        let blob_bytes = decode_hex(blob_hex, &format!("data[{position}]"))?;
        let versioned_hash = blob_versioned_hash_from_bytes(&blob_bytes)
            .map_err(|error| format!("data[{position}] KZG commitment: {error}"))?;
        available.push(AvailableBlob {
            versioned_hash,
            bytes: blob_bytes.into(),
        });
    }
    Ok(available)
}

fn decode_hex(raw: &str, field: &str) -> Result<Vec<u8>, String> {
    let raw = raw
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} is missing the 0x prefix"))?;
    hex::decode(raw).map_err(|error| format!("{field} is invalid hex: {error}"))
}

fn json_u64(value: &Value, pointer: &str) -> L1Result<u64> {
    let raw = value
        .pointer(pointer)
        .ok_or_else(|| L1Error::Provider(format!("beacon response is missing {pointer}")))?;
    if let Some(number) = raw.as_u64() {
        return Ok(number);
    }
    raw.as_str()
        .ok_or_else(|| L1Error::Provider(format!("beacon response {pointer} is not a u64")))?
        .parse::<u64>()
        .map_err(|error| L1Error::Provider(format!("beacon response {pointer}: {error}")))
}

fn normalize_base_url(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        let mut path = url.path().to_owned();
        path.push('/');
        url.set_path(&path);
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_provider::ProviderBuilder;
    use alloy_transport::mock::Asserter;
    use eez_blob_codec::blob_versioned_hashes;
    use eez_protocol::abi::ProofSystemBatchPerVerificationEntriesSol;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn non_eez_consensus_blob() -> Vec<u8> {
        let mut blob = vec![0; eez_blob_codec::BLOB_BYTES];
        // This is a valid, small BLS scalar in the first field element, but
        // EEZ reserves the element's most-significant byte as zero.
        blob[0] = 1;
        assert!(Blob::try_from_slice(0, &blob).is_err());
        blob
    }

    #[test]
    fn profile_requires_empty_calldata_and_all_indices_in_order() {
        let tx_hash = B256::repeat_byte(0x11);
        let hashes = [B256::repeat_byte(0x22), B256::repeat_byte(0x33)];
        assert!(
            validate_blob_profile(
                tx_hash,
                &Bytes::new(),
                &[U256::ZERO, U256::from(1)],
                &hashes,
            )
            .is_ok()
        );
        assert!(matches!(
            validate_blob_profile(tx_hash, &Bytes::new(), &[], &[]),
            Err(L1Error::Decode(_))
        ));
        assert!(matches!(
            validate_blob_profile(
                tx_hash,
                &Bytes::from_static(b"unexpected"),
                &[U256::ZERO, U256::from(1)],
                &hashes,
            ),
            Err(L1Error::Decode(_))
        ));
        assert!(matches!(
            validate_blob_profile(
                tx_hash,
                &Bytes::new(),
                &[U256::from(1), U256::ZERO],
                &hashes,
            ),
            Err(L1Error::Decode(_))
        ));
    }

    #[test]
    fn base_url_normalization_preserves_path_prefix() {
        let url = normalize_base_url("https://example.test/beacon".parse().unwrap());
        assert_eq!(
            url.join("eth/v1/beacon/genesis").unwrap().as_str(),
            "https://example.test/beacon/eth/v1/beacon/genesis"
        );
    }

    #[test]
    fn json_u64_accepts_consensus_string_encoding() {
        let value = serde_json::json!({"data": {"genesis_time": "123"}});
        assert_eq!(json_u64(&value, "/data/genesis_time").unwrap(), 123);
    }

    #[test]
    fn sidecar_parser_authenticates_blob_against_commitment() {
        let blobs = eez_blob_codec::encode_native_payload(
            7,
            Bytes::from_static(b"authenticated payload"),
            Vec::new(),
        )
        .unwrap();
        let commitment = eez_blob_codec::blob_kzg_commitments(&blobs)
            .unwrap()
            .remove(0);
        let unrelated_blob = non_eez_consensus_blob();
        let unrelated_commitment = blob_kzg_commitment_from_bytes(&unrelated_blob).unwrap();
        let response = serde_json::json!({
            "data": [{
                "blob": format!("0x{}", hex::encode(blobs[0].as_bytes())),
                "kzg_commitment": format!("0x{}", hex::encode(commitment)),
            }, {
                "blob": format!("0x{}", hex::encode(&unrelated_blob)),
                "kzg_commitment": format!("0x{}", hex::encode(unrelated_commitment)),
            }]
        });

        let parsed = parse_sidecars(&response).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].versioned_hash,
            blob_versioned_hashes(&blobs).unwrap()[0]
        );

        let mut bad_commitment = commitment;
        bad_commitment[0] ^= 1;
        let mismatched = serde_json::json!({
            "data": [{
                "blob": format!("0x{}", hex::encode(blobs[0].as_bytes())),
                "kzg_commitment": format!("0x{}", hex::encode(bad_commitment)),
            }]
        });
        assert!(parse_sidecars(&mismatched).is_err());
    }

    #[test]
    fn blobs_parser_recomputes_versioned_hash() {
        let blobs = eez_blob_codec::encode_native_payload(
            7,
            Bytes::from_static(b"post-Fulu authenticated payload"),
            Vec::new(),
        )
        .unwrap();
        let response = serde_json::json!({
            "data": [format!("0x{}", hex::encode(blobs[0].as_bytes()))]
        });

        let parsed = parse_blobs(&response).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].versioned_hash,
            blob_versioned_hashes(&blobs).unwrap()[0]
        );
        assert!(parse_blobs(&serde_json::json!({"data": [{}]})).is_err());
    }

    #[tokio::test]
    async fn hydration_ignores_unrelated_non_eez_blob_in_same_slot() {
        let rollup_id = 7;
        let expected_payload = Bytes::from_static(b"real derivation payload");
        let messages = vec![
            eez_blob_codec::Message::InitiateCrossChainTransaction {
                chain_id: 0,
                // L1-origin brackets publish no transaction bytes.
                tx_data: Bytes::new(),
            },
            eez_blob_codec::Message::Call {
                to_chain: rollup_id,
                from_address: alloy_primitives::Address::with_last_byte(1),
                to_address: alloy_primitives::Address::with_last_byte(2),
                value: U256::ZERO,
                gas: 0,
                data: Bytes::from_static(b"call"),
            },
            eez_blob_codec::Message::ReturnFail {
                return_data: Bytes::from_static(b"caught"),
            },
            eez_blob_codec::Message::FinishCrossChainTransaction,
        ];
        let blobs = eez_blob_codec::encode_native_payload(
            rollup_id,
            expected_payload.clone(),
            messages.clone(),
        )
        .unwrap();
        let versioned_hashes = blob_versioned_hashes(&blobs).unwrap();
        // Beacon endpoints return every blob in the slot. Authentication must
        // not apply EEZ stream rules to another transaction's blob.
        let unrelated_blob = non_eez_consensus_blob();
        let beacon_blobs = serde_json::json!({
            "data": [
                format!("0x{}", hex::encode(blobs[0].as_bytes())),
                format!("0x{}", hex::encode(&unrelated_blob)),
            ]
        })
        .to_string();
        let beacon_url = serve_json_responses(vec![
            serde_json::json!({"data": {"genesis_time": "1000"}}).to_string(),
            serde_json::json!({"data": {"SECONDS_PER_SLOT": "12"}}).to_string(),
            beacon_blobs,
        ])
        .await;

        let block_hash = B256::repeat_byte(0x44);
        let mut block: alloy_rpc_types_eth::Block = alloy_rpc_types_eth::Block::default();
        block.header.hash = block_hash;
        block.header.inner.number = 42;
        block.header.inner.timestamp = 1_060;
        let asserter = Asserter::new();
        asserter.push_success(&block);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let abi_batch = ProofSystemBatchPerVerificationEntriesSol {
            blobIndices: vec![U256::ZERO],
            ..Default::default()
        };
        let post_batch_input = postAndVerifyBatchCall { batch: abi_batch }.abi_encode();
        let mut scanned = vec![ScannedBatch {
            l1_block_number: 42,
            l1_block_hash: block_hash,
            tx_hash: B256::repeat_byte(0x55),
            tx_index: 0,
            submitter: alloy_primitives::Address::ZERO,
            operations_payload: Bytes::new(),
            semantic_transactions: Default::default(),
            transaction_blob_hashes: versioned_hashes,
            post_batch_input: post_batch_input.into(),
            state_applied: true,
            settlement: crate::scan::Settlement::NONE,
            claimed_current_root: None,
            claimed_new_root: None,
        }];

        BeaconBlobSource::new(Some(beacon_url))
            .hydrate_scanned_batches(&provider, rollup_id, &mut scanned)
            .await
            .unwrap();
        assert_eq!(scanned[0].operations_payload, expected_payload);
        assert_eq!(scanned[0].semantic_transactions.len(), 1);
        assert_eq!(
            eez_protocol::blob_semantics::encode_semantic_transaction(
                &scanned[0].semantic_transactions[0]
            )
            .unwrap(),
            messages,
            "hydration must preserve the authenticated action stream for follower reconstruction",
        );
    }

    #[tokio::test]
    async fn historical_nine_blob_batch_uses_containing_fork_and_checks_each_transaction() {
        use alloy_eips::eip7840::BlobParams;
        let rollup_id = 7;
        let payload = Bytes::from(vec![0; 8 * eez_blob_codec::DATA_BYTES_PER_BLOB + 100]);
        let blobs =
            eez_blob_codec::encode_native_payload(rollup_id, payload.clone(), Vec::new()).unwrap();
        assert_eq!(blobs.len(), 9);
        let beacon_url = serve_json_responses(vec![
            serde_json::json!({"data": {"genesis_time": "1000"}}).to_string(),
            serde_json::json!({"data": {"SECONDS_PER_SLOT": "12"}}).to_string(),
            serde_json::json!({"data": blobs.iter().map(|blob| format!("0x{}", hex::encode(blob.as_bytes()))).collect::<Vec<_>>()}).to_string(),
        ]).await;
        let source =
            BeaconBlobSource::new(Some(beacon_url)).with_rules(Arc::new(
                |timestamp| match timestamp {
                    0..1_000 => None,
                    1_000..1_072 => Some(BlobParams::prague()),
                    _ => Some(BlobParams::osaka()),
                },
            ));
        assert!(source.historical_limit(999).is_err());
        assert_eq!(source.historical_limit(1_071).unwrap(), 9);
        assert_eq!(source.historical_limit(1_072).unwrap(), 6);

        let block_hash = B256::repeat_byte(0x44);
        let mut block: alloy_rpc_types_eth::Block = alloy_rpc_types_eth::Block::default();
        block.header.hash = block_hash;
        block.header.inner.number = 42;
        block.header.inner.timestamp = 1_060;
        let asserter = Asserter::new();
        asserter.push_success(&block);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let mut batch = ProofSystemBatchPerVerificationEntriesSol {
            blobIndices: (0..blobs.len()).map(U256::from).collect(),
            ..Default::default()
        };
        let valid = ScannedBatch {
            l1_block_number: 42,
            l1_block_hash: block_hash,
            tx_hash: B256::repeat_byte(0x55),
            tx_index: 0,
            submitter: alloy_primitives::Address::ZERO,
            operations_payload: Bytes::new(),
            semantic_transactions: Default::default(),
            transaction_blob_hashes: blob_versioned_hashes(&blobs).unwrap(),
            post_batch_input: postAndVerifyBatchCall {
                batch: batch.clone(),
            }
            .abi_encode()
            .into(),
            state_applied: true,
            settlement: crate::scan::Settlement::NONE,
            claimed_current_root: None,
            claimed_new_root: None,
        };
        // A second event at the same L1 height must not bypass its own
        // validation just because its containing block's blobs are cached.
        batch.blobIndices.swap(0, 1);
        let invalid = ScannedBatch {
            tx_hash: B256::repeat_byte(0x56),
            tx_index: 1,
            post_batch_input: postAndVerifyBatchCall { batch }.abi_encode().into(),
            ..valid.clone()
        };
        let mut scanned = [valid, invalid];
        let error = source
            .hydrate_scanned_batches(&provider, rollup_id, &mut scanned)
            .await
            .unwrap_err();
        assert_eq!(scanned[0].operations_payload, payload);
        assert!(scanned[1].operations_payload.is_empty());
        assert!(
            error.to_string().contains("exactly once in order"),
            "{error}"
        );

        // Even an authenticated nine-blob carrier becomes invalid if the
        // containing block is in the fork with a six-blob transaction maximum.
        block.header.inner.timestamp = 1_072;
        let asserter = Asserter::new();
        asserter.push_success(&block);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let error = source
            .hydrate_scanned_batches(&provider, rollup_id, &mut scanned[..1])
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("containing fork maximum is 6"),
            "{error}"
        );
    }

    async fn serve_json_responses(responses: Vec<String>) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 4_096];
                let _ = stream.read(&mut request).await.unwrap();
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        format!("http://{address}/").parse().unwrap()
    }
}
