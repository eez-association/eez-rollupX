use alloy_consensus::{SignableTransaction, TxEip4844, TxEnvelope, transaction::Recovered};
use alloy_primitives::{Address, B256, Bytes, Signature, U256, hex};
use alloy_sol_types::{SolCall, SolEvent};
use eez_protocol::abi::{
    BatchPosted, ProofSystemBatchPerVerificationEntriesSol, RollupIdWithProofSystemsSol,
    postAndVerifyBatchCall,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) struct BlobFixture {
    pub block: alloy_rpc_types_eth::Block,
    pub tx: alloy_rpc_types_eth::Transaction,
    pub log: alloy_rpc_types_eth::Log,
    pub payload: Bytes,
    pub blobs: Value,
}

impl BlobFixture {
    pub fn new() -> Self {
        let payload: Bytes =
            eez_payload_codec::v0::encode_current(&[eez_payload_codec::v0::BlockRun {
                blocks: 1,
                beneficiary: Address::ZERO,
                extra_data: &[],
                transactions: &[],
            }])
            .unwrap()
            .into();
        let blobs = eez_blob_codec::encode_native_payload(1, payload.clone(), vec![]).unwrap();
        let batch = ProofSystemBatchPerVerificationEntriesSol {
            blobIndices: vec![U256::ZERO],
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 1,
                proofSystemIndexes: vec![],
            }],
            ..Default::default()
        };
        let tx = TxEip4844 {
            input: postAndVerifyBatchCall { batch }.abi_encode().into(),
            blob_versioned_hashes: eez_blob_codec::blob_versioned_hashes(&blobs).unwrap(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature());
        let tx_hash = *tx.hash();
        let mut block: alloy_rpc_types_eth::Block = Default::default();
        block.header.hash = B256::repeat_byte(0x42);
        block.header.inner.number = 42;
        block.header.inner.timestamp = 1060;
        block.header.inner.parent_hash = B256::repeat_byte(0x41);
        let tx = alloy_rpc_types_eth::Transaction {
            inner: Recovered::new_unchecked(TxEnvelope::from(tx), Address::ZERO),
            block_hash: Some(block.header.hash),
            block_number: Some(42),
            transaction_index: Some(0),
            block_timestamp: None,
            effective_gas_price: None,
        };
        let log = alloy_rpc_types_eth::Log {
            inner: alloy_primitives::Log {
                address: Address::ZERO,
                data: BatchPosted {
                    sharedPublicInput: B256::ZERO,
                    rollupIds: vec![1],
                }
                .encode_log_data(),
            },
            block_hash: Some(block.header.hash),
            block_number: Some(42),
            transaction_hash: Some(tx_hash),
            transaction_index: Some(0),
            log_index: Some(0),
            ..Default::default()
        };
        Self {
            block,
            tx,
            log,
            payload,
            blobs: json!({"data": blobs.iter().map(|b| format!("0x{}", hex::encode(b.as_bytes()))).collect::<Vec<_>>()}),
        }
    }

    pub fn scan_replies(&self, replies: &alloy_transport::mock::Asserter) {
        replies.push_success(&vec![self.log.clone()]);
        replies.push_success(&Vec::<alloy_rpc_types_eth::Log>::new());
        replies.push_success(&self.tx);
    }
}

/// Strict, finite HTTP fixture: every expected beacon request must occur.
pub(crate) async fn beacon(
    replies: Vec<(&'static str, u16, Value)>,
) -> (url::Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (path, status, body) in replies {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut request = vec![0; 4096];
            let n = stream.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..n])
                    .starts_with(&format!("GET /{path} HTTP/1.1\r\n"))
            );
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    (format!("http://{address}/").parse().unwrap(), server)
}

pub(crate) fn timing() -> Vec<(&'static str, u16, Value)> {
    vec![
        (
            "eth/v1/beacon/genesis",
            200,
            json!({"data": {"genesis_time": "1000"}}),
        ),
        (
            "eth/v1/config/spec",
            200,
            json!({"data": {"SECONDS_PER_SLOT": "12"}}),
        ),
    ]
}
