//! Reproducible input-side construction of frozen v2 conformance cases.
//! No service response supplies an expected cursor, state root or proof digest.
//! Native full-window proofs are captured on-chain oracles; ordinary subranges
//! use separately constructed DA and public inputs, with a public fixture key.

use super::*;
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{Bytes, Signature, U256};
use alloy_sol_types::SolCall as _;
use eez_control_rpc::{v1::BlockWitness, v2::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tonic::Code;

struct Fixture {
    name: &'static str,
    begin: Begin,
    blocks: Vec<BlockWitness>,
    finalize: Finalize,
    proof: Proof,
}

impl Fixture {
    fn native(name: &'static str) -> Self {
        let mut begin = None;
        let mut blocks = Vec::new();
        let mut finalize = None;
        for frame in crate::testkit::native_frames(name) {
            match frame.kind.unwrap() {
                client_frame::Kind::Begin(value) => {
                    assert!(begin.replace(value).is_none());
                }
                client_frame::Kind::Block(value) => blocks.push(value.data.unwrap()),
                client_frame::Kind::Finalize(value) => {
                    assert!(finalize.replace(value).is_none());
                }
                _ => panic!("capture must be a complete cold exchange"),
            }
        }
        let response: Value = serde_json::from_slice(
            &std::fs::read(
                crate::testkit::manifest_dir()
                    .join("tests/fixtures")
                    .join(name)
                    .join("response.json"),
            )
            .unwrap(),
        )
        .unwrap();
        Self {
            name,
            begin: begin.unwrap(),
            blocks,
            finalize: finalize.unwrap(),
            proof: Proof {
                public_inputs_hash: hex::decode(response["public_inputs_hash"].as_str().unwrap())
                    .unwrap(),
                proof: hex::decode(response["signature"].as_str().unwrap()).unwrap(),
            },
        }
    }

    fn ordinary() -> Self {
        let mut fixture = Self::native("native-offline-bidirectional");
        let bridge = fixture
            .blocks
            .iter()
            .flat_map(|b| b.witness.as_ref().unwrap().l1_headers_rlp.iter().cloned())
            .collect::<Vec<_>>();
        fixture.blocks.truncate(15);
        for block in &mut fixture.blocks {
            block.witness.as_mut().unwrap().l1_headers_rlp.clear();
        }
        // Supplying the complete bridge early makes each prefix's evidence
        // checkpoint sufficient for independent Sync-aligned finalization.
        fixture.blocks[0].witness.as_mut().unwrap().l1_headers_rlp = bridge;
        let (finalize, proof) = fixture.range(1, 15);
        fixture.finalize = finalize;
        fixture.proof = proof;
        fixture
    }

    fn hash(&self, number: u64) -> Vec<u8> {
        if number == self.begin.anchor_number {
            self.begin.anchor_hash.clone()
        } else {
            self.blocks[(number - self.begin.anchor_number - 1) as usize]
                .hash
                .clone()
        }
    }

    fn range(&self, from: u64, to: u64) -> (Finalize, Proof) {
        use eez_protocol::l1_history::{EvidenceAnchor, OfflineContext};
        let state = fixture_state(self.name);
        let mut batch = eez_protocol::abi::postAndVerifyBatchCall::abi_decode_validate(
            &self.finalize.settlement.as_ref().unwrap().abi_calldata,
        )
        .unwrap()
        .batch;
        batch.entries.truncate(1);
        batch.entries[0].rootUpdates[0].currentRoot = B256::from_slice(&self.hash(from - 1));
        batch.entries[0].rootUpdates[0].newRoot = B256::from_slice(&self.hash(to));
        batch.expectedRootPerRollup[0].root = B256::from_slice(&self.hash(from - 1));
        batch.immediateEntryCount = U256::from(1);
        batch.staticEntries.clear();
        eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
        let blocks = self.blocks[(from - 1) as usize..to as usize]
            .iter()
            .map(|b| alloy_rlp::decode_exact::<eez_primitives::Block>(&b.rlp).unwrap())
            .collect::<Vec<_>>();
        let transactions = blocks
            .iter()
            .map(|b| {
                let ordinary = eez_driver::derivation::ordinary_prefix(b);
                assert_eq!(
                    ordinary.len(),
                    b.body.transactions.len(),
                    "subrange contains native actions"
                );
                ordinary
                    .iter()
                    .map(|tx| Bytes::from(tx.encoded_2718()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let runs = blocks
            .iter()
            .zip(&transactions)
            .map(|(b, txs)| eez_payload_codec::v0::BlockRun {
                blocks: 1,
                beneficiary: b.header.beneficiary,
                extra_data: &b.header.extra_data,
                transactions: txs,
            })
            .collect::<Vec<_>>();
        let operations = eez_payload_codec::v0::encode(&runs).unwrap();
        let blobs = eez_blob_codec::encode_native_payload(
            self.begin.rollup_id,
            operations.into(),
            Vec::new(),
        )
        .unwrap();
        batch.blobIndices = (0..blobs.len()).map(U256::from).collect();
        let posting = self
            .finalize
            .settlement
            .as_ref()
            .unwrap()
            .posting_context
            .clone()
            .unwrap();
        let endpoint: alloy_consensus::Header = alloy_rlp::decode_exact(
            self.blocks[0]
                .witness
                .as_ref()
                .unwrap()
                .l1_headers_rlp
                .last()
                .unwrap(),
        )
        .unwrap();
        let context = OfflineContext::new(
            &state.deployment.profile,
            state.deployment.registry,
            state.deployment.manager,
            self.begin.rollup_id,
            batch.expectedRootPerRollup[0].root,
            EvidenceAnchor::ActualParent {
                number: posting.parent_number,
                hash: B256::from_slice(&posting.parent_hash),
                target_timestamp: posting.target_timestamp,
            },
            endpoint.timestamp,
        )
        .unwrap();
        let hashes = eez_protocol::public_inputs_hashes_with_blob_hashes(
            &batch,
            state.attester.proof_system_vkey().get(),
            &eez_blob_codec::blob_versioned_hashes(&blobs).unwrap(),
            context,
        )
        .unwrap();
        assert_eq!(hashes.len(), 1);
        let key = alloy_signer_local::PrivateKeySigner::from_bytes(&b256!(
            "8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
        ))
        .unwrap();
        let signature = Signature::from(
            key.credential()
                .sign_prehash_recoverable(hashes[0].as_slice())
                .unwrap(),
        );
        let mut proof = signature.as_bytes().to_vec();
        proof[64] = u8::from(signature.v()) + 27;
        (
            Finalize {
                from_block: from,
                to_block: to,
                terminal_hash: self.hash(to),
                settlement: Some(SettlementInput {
                    abi_calldata: eez_protocol::entries::encode_postbatch(&batch),
                    blobs: blobs.into_iter().map(|b| b.as_bytes().to_vec()).collect(),
                    posting_context: Some(posting),
                }),
            },
            Proof {
                public_inputs_hash: hashes[0].to_vec(),
                proof,
            },
        )
    }
}

#[derive(Clone)]
struct Binding {
    id: Vec<u8>,
    epoch: u64,
    tip: (u64, Vec<u8>),
}

struct Transcript {
    value: Value,
    next_request: u64,
    bindings: BTreeMap<u64, Binding>,
}

impl Transcript {
    fn new(name: &str, fixture: &Fixture) -> Self {
        Self {
            next_request: 1,
            bindings: BTreeMap::new(),
            value: json!({
                "format": 2, "case": name, "category": "offline-v2", "fixture": fixture.name,
                "authority": "Input-side construction; native capture or derived ordinary subrange, never response-derived expectations",
                "normalization": "Distinct session tokens and monotonic epochs rebound; diagnostic wording/reused omitted",
                "oracle": { "attester": "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc" },
                "expected_proofs": 0, "events": [],
                "server_profile": { "max_window_blocks": 4096, "max_window_bytes": 16777216,
                    "max_window_witness_items": 100000, "request_timeout_secs": 120, "stream_idle_timeout_secs": 30 }
            }),
        }
    }

    fn event(&mut self, value: Value) {
        self.value["events"].as_array_mut().unwrap().push(value);
    }
    fn edge(&mut self, transport: u64, direction: &str) {
        self.event(json!({"transport":transport,"direction":direction}));
    }
    fn frame(&mut self, transport: u64, kind: Option<client_frame::Kind>) -> ClientFrame {
        let request_id = self.next_request;
        self.next_request += 1;
        let (session_id, epoch) = self
            .bindings
            .get(&transport)
            .map_or((vec![], 0), |b| (b.id.clone(), b.epoch));
        ClientFrame {
            session_id,
            epoch,
            request_id,
            kind,
        }
    }
    fn send(&mut self, transport: u64, frame: &ClientFrame, check: &str) {
        self.event(json!({"transport":transport,"direction":"client","protobuf_hex":hex::encode(frame.encode_to_vec()),"check":check}));
    }
    fn response(
        &mut self,
        transport: u64,
        request_id: u64,
        kind: server_frame::Kind,
        authorized: bool,
        check: &str,
    ) {
        let binding = authorized.then(|| self.bindings.get(&transport).unwrap());
        let frame = ServerFrame {
            request_id,
            session_id: binding.map_or(vec![], |b| b.id.clone()),
            epoch: binding.map_or(0, |b| b.epoch),
            kind: Some(kind),
        };
        self.event(json!({"transport":transport,"direction":"server","protobuf_hex":hex::encode(frame.encode_to_vec()),"check":check}));
    }
    fn rejected(
        &mut self,
        transport: u64,
        frame: ClientFrame,
        code: Code,
        authorized: bool,
        check: &str,
    ) {
        let tip = if authorized {
            self.bindings[&transport].tip.clone()
        } else {
            (0, vec![])
        };
        self.send(transport, &frame, check);
        self.response(
            transport,
            frame.request_id,
            server_frame::Kind::Rejected(Rejected {
                code: i32::from(code),
                message: String::new(),
                details: vec![],
                validated_through: tip.0,
                validated_hash: tip.1,
            }),
            authorized,
            check,
        );
    }
    fn reject(&mut self, transport: u64, kind: client_frame::Kind, code: Code, check: &str) {
        let frame = self.frame(transport, Some(kind));
        self.rejected(transport, frame, code, true, check);
    }
    fn begin(&mut self, transport: u64, begin: &Begin) {
        let mut frame = self.frame(transport, Some(client_frame::Kind::Begin(begin.clone())));
        frame.session_id.clear();
        frame.epoch = 0;
        self.send(transport, &frame, "Begin exact native predecessor");
        self.bindings.insert(
            transport,
            Binding {
                id: vec![transport as u8 + 1; 32],
                epoch: 1,
                tip: (begin.anchor_number, begin.anchor_hash.clone()),
            },
        );
        self.ready(transport, frame.request_id);
    }
    fn ready(&mut self, transport: u64, request: u64) {
        let tip = self.bindings[&transport].tip.clone();
        self.response(
            transport,
            request,
            server_frame::Kind::Ready(Ready {
                validated_through: tip.0,
                validated_hash: tip.1,
            }),
            true,
            "Ready exact cursor and binding",
        );
    }
    fn block(&mut self, transport: u64, block: &BlockWitness) {
        let frame = self.frame(
            transport,
            Some(client_frame::Kind::Block(Block {
                data: Some(block.clone()),
            })),
        );
        self.send(transport, &frame, "Complete execution and L1 evidence");
        self.ack(transport, frame.request_id, block);
    }
    fn ack(&mut self, transport: u64, request: u64, block: &BlockWitness) {
        let decoded: eez_primitives::Block = alloy_rlp::decode_exact(&block.rlp).unwrap();
        let tip = &mut self.bindings.get_mut(&transport).unwrap().tip;
        if block.number > tip.0 {
            *tip = (block.number, block.hash.clone());
        }
        self.response(
            transport,
            request,
            server_frame::Kind::Validated(Validated {
                number: block.number,
                hash: decoded.header.hash_slow().to_vec(),
                post_state_root: decoded.header.state_root.to_vec(),
                reused: false,
            }),
            true,
            "ACK independent native header and state root",
        );
    }
    fn blocks(&mut self, transport: u64, blocks: &[BlockWitness]) {
        for b in blocks {
            self.block(transport, b);
        }
    }
    fn proof(&mut self, transport: u64, finalize: &Finalize, proof: &Proof) {
        let frame = self.frame(
            transport,
            Some(client_frame::Kind::Finalize(finalize.clone())),
        );
        self.send(
            transport,
            &frame,
            "Finalize exact Sync range and settlement",
        );
        self.response(
            transport,
            frame.request_id,
            server_frame::Kind::Proof(proof.clone()),
            true,
            "Independent digest and configured attester",
        );
        let count = self.value["expected_proofs"].as_u64().unwrap();
        self.value["expected_proofs"] = (count + 1).into();
        self.edge(transport, "eof");
    }
    fn resume(&mut self, previous: u64, transport: u64) {
        let mut binding = self.bindings[&previous].clone();
        let mut frame = self.frame(transport, Some(client_frame::Kind::Resume(Resume {})));
        frame.session_id = binding.id.clone();
        frame.epoch = 0;
        self.send(transport, &frame, "Resume retained evidence checkpoint");
        binding.epoch += 1;
        self.bindings.insert(transport, binding);
        self.ready(transport, frame.request_id);
    }
    fn rewind(&mut self, transport: u64, number: u64, hash: Vec<u8>) {
        let frame = self.frame(
            transport,
            Some(client_frame::Kind::Rewind(Rewind {
                ancestor_hash: hash.clone(),
            })),
        );
        self.send(
            transport,
            &frame,
            "Rewind execution and L1 evidence together",
        );
        let binding = self.bindings.get_mut(&transport).unwrap();
        binding.epoch += 1;
        binding.tip = (number, hash);
        self.ready(transport, frame.request_id);
    }
    fn cancel(&mut self, transport: u64) {
        let frame = self.frame(transport, Some(client_frame::Kind::Cancel(Cancel {})));
        self.send(transport, &frame, "Cancel bound session only");
        self.response(
            transport,
            frame.request_id,
            server_frame::Kind::Cancelled(Cancelled {}),
            true,
            "Cancelled retires this session",
        );
        self.edge(transport, "eof");
    }
    fn close(&mut self, transport: u64) {
        self.edge(transport, "half_close");
        self.edge(transport, "eof");
    }
}

const CASES: &[&str] = &[
    "baseline-lookup",
    "baseline-static",
    "begin-boundaries",
    "block-boundaries",
    "block-quota",
    "byte-quota",
    "cancel-session-isolation",
    "competing-branches",
    "concurrent-composers",
    "detached-session-eviction",
    "finalize-byte-quota",
    "finalize-ranges",
    "frame-size-limit",
    "half-close-drains",
    "invalid-witness-retry",
    "pipelined-gap-recovery",
    "posting-context-boundaries",
    "resume-after-proof",
    "resume-boundaries",
    "resume-takeover-fencing",
    "rewind-boundaries",
    "settlement-abi-binding",
    "settlement-blob-encoding",
    "settlement-required-inputs",
    "sync-range-proof",
    "stream-capacity",
    "subrange-promoted-anchor",
    "terminal-before-tip",
    "witness-quota",
];

fn block_kind(block: BlockWitness) -> client_frame::Kind {
    client_frame::Kind::Block(Block { data: Some(block) })
}

fn make_case(name: &str) -> Value {
    use client_frame::Kind as C;
    let fixture = match name {
        "baseline-lookup" => Fixture::native("native-osaka-lookup"),
        "baseline-static"
        | "posting-context-boundaries"
        | "settlement-abi-binding"
        | "settlement-blob-encoding"
        | "settlement-required-inputs" => Fixture::native("native-osaka-changing-static"),
        _ => Fixture::ordinary(),
    };
    let f = &fixture;
    let mut t = Transcript::new(name, f);
    match name {
        "baseline-lookup" | "baseline-static" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            t.proof(0, &f.finalize, &f.proof);
        }
        "begin-boundaries" => {
            for field in [0, 1] {
                for width in [0, 31, 33] {
                    let mut begin = f.begin.clone();
                    if field == 0 {
                        begin.anchor_hash = vec![1; width];
                    } else {
                        begin.anchor_state_root = vec![1; width];
                    }
                    let frame = t.frame(0, Some(C::Begin(begin)));
                    t.rejected(
                        0,
                        frame,
                        Code::InvalidArgument,
                        false,
                        "Begin hash/root width",
                    );
                }
            }
            for defect in 0..5 {
                let mut begin = f.begin.clone();
                match defect {
                    0 => begin.rollup_id = 0,
                    1 => begin.rollup_id += 1,
                    2 => begin.anchor_number = u64::MAX,
                    _ => {}
                }
                let mut frame = t.frame(0, Some(C::Begin(begin)));
                if defect == 3 {
                    frame.session_id = vec![1; 32];
                }
                if defect == 4 {
                    frame.epoch = 1;
                }
                t.rejected(
                    0,
                    frame,
                    if defect == 1 {
                        Code::Unimplemented
                    } else {
                        Code::InvalidArgument
                    },
                    false,
                    "Begin profile, overflow or initial binding",
                );
            }
            let frame = t.frame(0, None);
            t.rejected(
                0,
                frame,
                Code::Unimplemented,
                false,
                "Unknown unbound operation",
            );
            let frame = t.frame(0, Some(C::Cancel(Cancel {})));
            t.rejected(
                0,
                frame,
                Code::PermissionDenied,
                false,
                "Cancel before Begin",
            );
            t.begin(0, &f.begin);
            t.reject(
                0,
                C::Begin(f.begin.clone()),
                Code::InvalidArgument,
                "Repeated Begin preserves cursor",
            );
            let frame = t.frame(0, None);
            t.rejected(
                0,
                frame,
                Code::Unimplemented,
                true,
                "Unknown bound operation preserves cursor",
            );
            t.close(0);
        }
        "block-boundaries" => {
            t.begin(0, &f.begin);
            t.reject(
                0,
                C::Block(Block { data: None }),
                Code::InvalidArgument,
                "Missing Block data",
            );
            for defect in 0..10 {
                let mut block = f.blocks[0].clone();
                let code = match defect {
                    0 => {
                        block.hash.pop();
                        Code::InvalidArgument
                    }
                    1 => {
                        block.parent_hash.push(0);
                        Code::InvalidArgument
                    }
                    2 => {
                        block.witness = None;
                        Code::InvalidArgument
                    }
                    3 => {
                        block.rlp.clear();
                        Code::FailedPrecondition
                    }
                    4 => {
                        block.rlp = vec![0xc0];
                        Code::FailedPrecondition
                    }
                    5 => {
                        block.hash = vec![0; 32];
                        Code::FailedPrecondition
                    }
                    6 => {
                        block.parent_hash = vec![1; 32];
                        Code::FailedPrecondition
                    }
                    7 => {
                        block.number += 1;
                        Code::FailedPrecondition
                    }
                    8 => {
                        block.witness.as_mut().unwrap().state.clear();
                        Code::FailedPrecondition
                    }
                    _ => {
                        block.witness.as_mut().unwrap().l1_headers_rlp.clear();
                        Code::FailedPrecondition
                    }
                };
                t.reject(
                    0,
                    block_kind(block),
                    code,
                    "Malformed metadata, execution or L1 evidence",
                );
            }
            t.reject(
                0,
                block_kind(f.blocks[1].clone()),
                Code::FailedPrecondition,
                "Gap before first block",
            );
            t.blocks(0, &f.blocks[..2]);
            let mut changed = f.blocks[0].clone();
            changed.witness.as_mut().unwrap().keys.push(vec![1]);
            t.reject(
                0,
                block_kind(changed),
                Code::FailedPrecondition,
                "Changed duplicate witness cannot replace checked evidence",
            );
            for request_id in [0, u64::MAX] {
                let mut frame = t.frame(0, Some(block_kind(f.blocks[0].clone())));
                frame.request_id = request_id;
                t.send(0, &frame, "Boundary request ID and duplicate before tip");
                t.ack(0, request_id, &f.blocks[0]);
            }
            t.close(0);
        }
        "block-quota" | "byte-quota" | "witness-quota" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks[..2]);
            let bytes = t.value["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["direction"] == "client")
                .map(|e| e["protobuf_hex"].as_str().unwrap().len() / 2)
                .sum::<usize>();
            let items = f.blocks[..2]
                .iter()
                .map(|b| crate::window::wire_witness_item_count(b.witness.as_ref().unwrap()))
                .sum::<usize>();
            let (key, limit) = match name {
                "block-quota" => ("max_window_blocks", 2),
                "byte-quota" => ("max_window_bytes", bytes),
                _ => ("max_window_witness_items", items),
            };
            t.value["server_profile"][key] = limit.into();
            t.begin(1, &f.begin);
            t.blocks(1, &f.blocks[..2]);
            t.reject(
                0,
                block_kind(f.blocks[2].clone()),
                Code::ResourceExhausted,
                "Next block exceeds exact retained quota",
            );
            t.rewind(0, 1, f.hash(1));
            t.block(0, &f.blocks[1]);
            t.reject(
                1,
                block_kind(f.blocks[2].clone()),
                Code::ResourceExhausted,
                "Quota release on A does not release B",
            );
            t.cancel(0);
            t.cancel(1);
        }
        "cancel-session-isolation" => {
            t.begin(0, &f.begin);
            t.begin(1, &f.begin);
            t.block(0, &f.blocks[0]);
            t.block(1, &f.blocks[0]);
            let mut frame = t.frame(0, Some(C::Cancel(Cancel {})));
            frame.session_id = t.bindings[&1].id.clone();
            t.rejected(
                0,
                frame,
                Code::PermissionDenied,
                false,
                "Cross-session Cancel",
            );
            t.cancel(0);
            let mut frame = t.frame(2, Some(C::Resume(Resume {})));
            frame.session_id = t.bindings[&0].id.clone();
            t.rejected(
                2,
                frame,
                Code::NotFound,
                false,
                "Retired session cannot Resume",
            );
            t.close(2);
            t.blocks(1, &f.blocks[1..]);
            t.proof(1, &f.finalize, &f.proof);
        }
        "competing-branches" => {
            t.begin(0, &f.begin);
            t.begin(1, &f.begin);
            t.block(0, &f.blocks[0]);
            let mut alternate = f.blocks[0].clone();
            let mut block: eez_primitives::Block = alloy_rlp::decode_exact(&alternate.rlp).unwrap();
            block.header.extra_data = Bytes::from_static(b"competing metadata");
            alternate.hash = block.header.hash_slow().to_vec();
            alternate.rlp = alloy_rlp::encode(block);
            t.block(1, &alternate);
            t.reject(
                1,
                block_kind(f.blocks[0].clone()),
                Code::FailedPrecondition,
                "Same height cannot replace checked branch",
            );
            t.reject(
                1,
                block_kind(f.blocks[1].clone()),
                Code::FailedPrecondition,
                "Original suffix cannot extend alternate parent",
            );
            t.rewind(1, 0, f.hash(0));
            t.blocks(1, &f.blocks);
            t.proof(1, &f.finalize, &f.proof);
            t.cancel(0);
        }
        "concurrent-composers" => {
            t.begin(0, &f.begin);
            t.begin(1, &f.begin);
            for block in &f.blocks {
                let a = t.frame(0, Some(block_kind(block.clone())));
                let b = t.frame(1, Some(block_kind(block.clone())));
                t.send(0, &a, "Composer A shared branch");
                t.send(1, &b, "Composer B shared branch");
                t.ack(0, a.request_id, block);
                t.ack(1, b.request_id, block);
            }
            t.proof(0, &f.finalize, &f.proof);
            t.proof(1, &f.finalize, &f.proof);
        }
        "detached-session-eviction" => {
            for transport in 0..9 {
                t.begin(transport, &f.begin);
                t.block(transport, &f.blocks[0]);
                t.close(transport);
            }
            let mut frame = t.frame(9, Some(C::Resume(Resume {})));
            frame.session_id = t.bindings[&0].id.clone();
            t.rejected(
                9,
                frame,
                Code::NotFound,
                false,
                "Oldest detached session is evicted without progress disclosure",
            );
            t.close(9);
            t.resume(8, 10);
            t.cancel(10);
        }
        "resume-after-proof" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            t.proof(0, &f.finalize, &f.proof);
            t.resume(0, 1);
            t.proof(1, &f.finalize, &f.proof);
        }
        "resume-boundaries" => {
            for width in [0, 1, 31, 32, 33] {
                let mut frame = t.frame(0, Some(C::Resume(Resume {})));
                frame.session_id = vec![0xee; width];
                t.rejected(
                    0,
                    frame,
                    if width == 32 {
                        Code::NotFound
                    } else {
                        Code::InvalidArgument
                    },
                    false,
                    "Malformed or unknown Resume discloses no cursor",
                );
            }
            t.begin(0, &f.begin);
            t.reject(
                0,
                C::Resume(Resume {}),
                Code::InvalidArgument,
                "Resume requires a new attachment",
            );
            t.cancel(0);
            let mut frame = t.frame(1, Some(C::Resume(Resume {})));
            frame.session_id = t.bindings[&0].id.clone();
            t.rejected(
                1,
                frame,
                Code::NotFound,
                false,
                "Cancelled session cannot Resume",
            );
            t.close(1);
        }
        "resume-takeover-fencing" => {
            t.begin(0, &f.begin);
            t.block(0, &f.blocks[0]);
            t.resume(0, 1);
            for epoch in [1, 2] {
                let mut frame = t.frame(0, Some(block_kind(f.blocks[1].clone())));
                frame.epoch = epoch;
                t.rejected(
                    0,
                    frame,
                    Code::PermissionDenied,
                    false,
                    "Fenced transport cannot use old or copied new epoch",
                );
            }
            t.edge(0, "disconnect");
            t.edge(1, "disconnect");
            t.resume(1, 2);
            t.blocks(2, &f.blocks[1..]);
            t.proof(2, &f.finalize, &f.proof);
        }
        "rewind-boundaries" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks[..5]);
            for width in [0, 31, 32, 33] {
                t.reject(
                    0,
                    C::Rewind(Rewind {
                        ancestor_hash: vec![0xee; width],
                    }),
                    Code::FailedPrecondition,
                    "Malformed or unknown Rewind preserves evidence",
                );
            }
            t.rewind(0, 3, f.hash(3));
            t.rewind(0, 3, f.hash(3));
            let mut frame = t.frame(0, Some(block_kind(f.blocks[3].clone())));
            frame.epoch = 1;
            t.rejected(
                0,
                frame,
                Code::PermissionDenied,
                false,
                "Stale epoch cannot mutate prefix",
            );
            t.reject(
                0,
                C::Rewind(Rewind {
                    ancestor_hash: f.hash(5),
                }),
                Code::FailedPrecondition,
                "Pruned suffix is not a retained ancestor",
            );
            t.reject(
                0,
                C::Finalize(f.range(1, 5).0),
                Code::FailedPrecondition,
                "Pruned suffix cannot finalize",
            );
            t.rewind(0, 0, f.hash(0));
            t.blocks(0, &f.blocks);
            t.proof(0, &f.finalize, &f.proof);
        }
        "sync-range-proof" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks[..5]);
            t.reject(
                0,
                C::Finalize(f.range(1, 1).0),
                Code::FailedPrecondition,
                "One block cannot finalize an incomplete Sync interval",
            );
            let (finalize, proof) = f.range(1, 5);
            t.proof(0, &finalize, &proof);
        }
        "subrange-promoted-anchor" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            let (finalize, proof) = f.range(6, 15);
            t.proof(0, &finalize, &proof);
            t.resume(0, 1);
            t.reject(
                1,
                C::Finalize(f.finalize.clone()),
                Code::FailedPrecondition,
                "Promoted predecessor rejects former full range",
            );
            t.reject(
                1,
                C::Rewind(Rewind {
                    ancestor_hash: f.hash(0),
                }),
                Code::FailedPrecondition,
                "Former anchor was pruned",
            );
            t.proof(1, &finalize, &proof);
        }
        "terminal-before-tip" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks[..10]);
            let (finalize, proof) = f.range(1, 5);
            t.proof(0, &finalize, &proof);
            t.resume(0, 1);
            let (finalize, proof) = f.range(1, 10);
            t.proof(1, &finalize, &proof);
        }
        "finalize-ranges" => {
            t.begin(0, &f.begin);
            t.reject(
                0,
                C::Finalize(f.finalize.clone()),
                Code::FailedPrecondition,
                "Empty prefix cannot finalize",
            );
            t.blocks(0, &f.blocks);
            for defect in 0..7 {
                let mut finalize = f.finalize.clone();
                match defect {
                    0 => finalize.from_block = 0,
                    1 => finalize.from_block = 16,
                    2 => finalize.to_block = 0,
                    3 => finalize.to_block = 16,
                    4 => finalize.terminal_hash.clear(),
                    5 => finalize.terminal_hash = vec![0xee; 32],
                    _ => finalize.terminal_hash = f.hash(14),
                }
                t.reject(
                    0,
                    C::Finalize(finalize),
                    Code::FailedPrecondition,
                    "Invalid range or terminal preserves validated tip",
                );
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        "stream-capacity" => {
            for transport in 0..8 {
                t.begin(transport, &f.begin);
            }
            t.event(json!({"transport":8,"direction":"rpc_open_error","code":8,"check":"Ninth stream exceeds eight-stream capacity"}));
            for transport in 0..8 {
                t.cancel(transport);
            }
        }
        "frame-size-limit" => {
            t.value["server_profile"]["max_window_bytes"] = 74.into();
            t.begin(0, &f.begin);
            t.cancel(0);
            let frame = t.frame(1, Some(block_kind(f.blocks[0].clone())));
            t.send(1, &frame, "Oversized frame before session admission");
            t.event(json!({"transport":1,"direction":"rpc_error","code":11,"check":"Transport decode ceiling"}));
            t.edge(1, "eof");
        }
        "finalize-byte-quota" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            let retained = t.value["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["direction"] == "client")
                .map(|e| e["protobuf_hex"].as_str().unwrap().len() / 2)
                .sum::<usize>();
            let frame = t.frame(0, Some(C::Finalize(f.finalize.clone())));
            t.value["server_profile"]["max_window_bytes"] = (retained + frame.encoded_len()).into();
            let mut larger = f.finalize.clone();
            larger.settlement.as_mut().unwrap().abi_calldata.push(0);
            t.reject(
                0,
                C::Finalize(larger),
                Code::ResourceExhausted,
                "Finalize exceeds retained-plus-settlement byte quota",
            );
            t.proof(0, &f.finalize, &f.proof);
        }
        "half-close-drains" => {
            t.begin(0, &f.begin);
            let frames = f
                .blocks
                .iter()
                .map(|b| t.frame(0, Some(block_kind(b.clone()))))
                .collect::<Vec<_>>();
            // Keep outstanding work below the fixed queue bound, then half-close
            // with the terminal and Finalize accepted but not yet acknowledged.
            for (frame, block) in frames[..frames.len() - 1].iter().zip(&f.blocks) {
                t.send(0, frame, "Complete prefix");
                t.ack(0, frame.request_id, block);
            }
            let terminal = frames.last().unwrap();
            let finalize = t.frame(0, Some(C::Finalize(f.finalize.clone())));
            t.send(0, terminal, "Terminal accepted before half-close");
            t.send(0, &finalize, "Finalize before half-close");
            t.edge(0, "half_close");
            t.ack(0, terminal.request_id, f.blocks.last().unwrap());
            t.response(
                0,
                finalize.request_id,
                server_frame::Kind::Proof(f.proof.clone()),
                true,
                "Half-close drains accepted settlement",
            );
            t.value["expected_proofs"] = 1.into();
            t.edge(0, "eof");
            t.begin(1, &f.begin);
            let block = t.frame(1, Some(block_kind(f.blocks[0].clone())));
            t.send(1, &block, "Block before half-close");
            t.edge(1, "half_close");
            t.ack(1, block.request_id, &f.blocks[0]);
            t.edge(1, "eof");
        }
        "invalid-witness-retry" => {
            t.begin(0, &f.begin);
            let mut block = f.blocks[0].clone();
            block.witness.as_mut().unwrap().state.clear();
            t.reject(
                0,
                block_kind(block),
                Code::FailedPrecondition,
                "Invalid witness cannot advance execution or L1 checkpoint",
            );
            t.blocks(0, &f.blocks);
            t.proof(0, &f.finalize, &f.proof);
        }
        "pipelined-gap-recovery" => {
            t.begin(0, &f.begin);
            let gap = t.frame(0, Some(block_kind(f.blocks[1].clone())));
            let first = t.frame(0, Some(block_kind(f.blocks[0].clone())));
            t.send(0, &gap, "Pipelined gap");
            t.send(0, &first, "Valid first block queued after gap");
            t.response(
                0,
                gap.request_id,
                server_frame::Kind::Rejected(Rejected {
                    code: i32::from(Code::FailedPrecondition),
                    message: String::new(),
                    details: vec![],
                    validated_through: f.begin.anchor_number,
                    validated_hash: f.begin.anchor_hash.clone(),
                }),
                true,
                "Gap does not advance cursor",
            );
            t.ack(0, first.request_id, &f.blocks[0]);
            for blocks in f.blocks[1..].chunks(4) {
                let frames = blocks
                    .iter()
                    .map(|b| t.frame(0, Some(block_kind(b.clone()))))
                    .collect::<Vec<_>>();
                for frame in &frames {
                    t.send(0, frame, "Four outstanding ordered requests");
                }
                for (frame, block) in frames.iter().zip(blocks) {
                    t.ack(0, frame.request_id, block);
                }
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        "posting-context-boundaries" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            for defect in 0..10 {
                let mut finalize = f.finalize.clone();
                let target = finalize
                    .settlement
                    .as_mut()
                    .unwrap()
                    .posting_context
                    .as_mut()
                    .unwrap();
                let code = match defect {
                    0 => {
                        target.chain_id = 0;
                        Code::InvalidArgument
                    }
                    1 => {
                        target.chain_id += 1;
                        Code::FailedPrecondition
                    }
                    2 => {
                        target.parent_hash.clear();
                        Code::InvalidArgument
                    }
                    3 => {
                        target.parent_hash = vec![0; 32];
                        Code::InvalidArgument
                    }
                    4 => {
                        target.parent_hash = vec![0xee; 32];
                        Code::FailedPrecondition
                    }
                    5 => {
                        target.parent_number += 1;
                        Code::FailedPrecondition
                    }
                    6 => {
                        target.target_timestamp = 0;
                        Code::InvalidArgument
                    }
                    7 => {
                        target.target_timestamp += 5;
                        Code::FailedPrecondition
                    }
                    8 => {
                        target.target_timestamp -= 5;
                        Code::FailedPrecondition
                    }
                    _ => {
                        finalize.settlement.as_mut().unwrap().posting_context = None;
                        Code::FailedPrecondition
                    }
                };
                t.reject(
                    0,
                    C::Finalize(finalize),
                    code,
                    "Posting context cannot be substituted or omitted for actual-parent mode",
                );
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        "settlement-required-inputs" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            for defect in 0..4 {
                let mut finalize = f.finalize.clone();
                match defect {
                    0 => finalize.settlement = None,
                    1 => finalize.settlement.as_mut().unwrap().abi_calldata.clear(),
                    2 => finalize.settlement.as_mut().unwrap().blobs.clear(),
                    _ => finalize.settlement = Some(SettlementInput::default()),
                }
                t.reject(
                    0,
                    C::Finalize(finalize),
                    Code::InvalidArgument,
                    "Required settlement input cannot be omitted",
                );
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        "settlement-blob-encoding" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            for defect in 0..5 {
                let mut finalize = f.finalize.clone();
                let blobs = &mut finalize.settlement.as_mut().unwrap().blobs;
                match defect {
                    0 => {
                        blobs[0].pop();
                    }
                    1 => blobs[0].push(0),
                    2 => blobs[0][0] = 0xff,
                    3 => {
                        let last = blobs[0].len() - 1;
                        blobs[0][last] = 1;
                    }
                    _ => blobs.push(blobs[0].clone()),
                }
                t.reject(
                    0,
                    C::Finalize(finalize),
                    Code::InvalidArgument,
                    "Complete canonical physical blob carrier required",
                );
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        "settlement-abi-binding" => {
            t.begin(0, &f.begin);
            t.blocks(0, &f.blocks);
            for defect in 0..11 {
                let mut finalize = f.finalize.clone();
                let input = finalize.settlement.as_mut().unwrap();
                let mut batch = eez_protocol::abi::postAndVerifyBatchCall::abi_decode_validate(
                    &input.abi_calldata,
                )
                .unwrap()
                .batch;
                let code = match defect {
                    0 => {
                        input.abi_calldata[0] ^= 1;
                        Code::InvalidArgument
                    }
                    1 => {
                        input.abi_calldata.push(0);
                        Code::InvalidArgument
                    }
                    2 => {
                        batch.blockNumber = 0;
                        Code::FailedPrecondition
                    }
                    3 => {
                        batch.expectedRootPerRollup[0].rollupId += 1;
                        Code::FailedPrecondition
                    }
                    4 => {
                        batch.expectedRootPerRollup[0].root = B256::ZERO;
                        Code::FailedPrecondition
                    }
                    5 => {
                        batch.expectedRootPerRollup[0].root = B256::repeat_byte(0xee);
                        Code::FailedPrecondition
                    }
                    6 => {
                        batch.entries[0].rollingHash = B256::repeat_byte(0xee);
                        Code::FailedPrecondition
                    }
                    7 => {
                        batch.immediateEntryCount = U256::from(batch.entries.len() + 1);
                        Code::FailedPrecondition
                    }
                    8 => {
                        batch.blobIndices[0] = U256::from(1);
                        Code::InvalidArgument
                    }
                    9 => {
                        batch.callData = Bytes::from_static(&[1]);
                        Code::InvalidArgument
                    }
                    _ => {
                        batch.entries[0].rootUpdates[0].newRoot = B256::repeat_byte(0xee);
                        Code::FailedPrecondition
                    }
                };
                if defect > 1 {
                    input.abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
                }
                t.reject(
                    0,
                    C::Finalize(finalize),
                    code,
                    "Canonical ABI, accepted root and execution/DA commitments required",
                );
            }
            t.proof(0, &f.finalize, &f.proof);
        }
        _ => panic!("unimplemented independent vector {name}"),
    }
    t.value
}

/// Artifact generation is explicit; normal conformance tests never rewrite
/// expectations. Regenerate after reviewing fixture/profile and case changes,
/// then replay every saved vector against a fresh service before accepting it.
#[test]
#[ignore = "explicit frozen fixture generation; see conformance/README.md"]
fn regenerate_offline_vectors() {
    let destination = crate::testkit::manifest_dir().join("tests/fixtures/streaming-v2");
    for name in CASES {
        let vector = make_case(name);
        std::fs::write(
            destination.join(format!("{name}.json")),
            format!("{}\n", serde_json::to_string_pretty(&vector).unwrap()),
        )
        .unwrap();
    }
}
