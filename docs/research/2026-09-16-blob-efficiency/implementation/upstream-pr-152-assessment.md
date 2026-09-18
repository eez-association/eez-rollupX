# Upstream PR #152 assessment

Reviewed 2026-09-17 at upstream head
[`f026c8dd`](https://github.com/eez-association/eez-rollup0/commit/f026c8ddcc5afea91039b767ff6f2c09f73be4c2),
based on upstream main
[`c5120e13`](https://github.com/eez-association/eez-rollup0/commit/c5120e137decfc9726a59570c6eebcabf1c905c6)
and protocol gitlink `9735f53abbb6b9f5e863f405ad4555b4701b7fda`.
The target fork base is `5469c88860408daf776ee812156dd280c1a9e262`
with protocol gitlink `3c0cdf4955941a5c23639346932b55b33babebbd`.
The two main branches have diverged; this is a design assessment, not a
cherry-pick or compatibility claim.

## Adopted principles

- Keep the decoded chain-specific payload structured through follower replay.
  Runtime consumers call the bounded sparse `Payload::decode` directly; the
  deriver consumes its allocation-free `BlockPlan` iterator instead of
  materializing duplicate legacy-shaped block-count and transaction vectors.
  Tags 0 through 2 and the dense `DecodedBatch` API were deleted entirely, so no
  alternate decoder can expand a runtime payload back into that representation.
- Do not derive replay inputs from local cadence or operator header defaults.
  The deriver no longer accepts a local block-time argument or contains a legacy
  default-header branch; v3 derived records and full records provide every replay
  input.
- Treat complete execution inputs and ancestry as block identity before an
  existing block may be reused. The v3 follower has one input-aware matching path
  for fresh and resumed reconciliation.
- Require encoder/decoder closure for transactions. A direct constructor
  regression covers empty transaction bytes, the asymmetry found in PR #152.
- Reuse the already decoded terminal block during fresh reconciliation rather
  than parsing its RLP a second time.
- Keep replay transaction buffers as reference-counted `Bytes`; sparse records
  and reconstructed Sync transactions no longer copy their byte contents into
  an intermediate `Vec<Vec<u8>>`.
- Require one explicit Sync-slot outcome from every composer. The optional
  compatibility method, pool-driven outcome, and disabled-policy runtime path
  were removed for the new network.

These changes leave the v3 wire bytes, terminal block, semantic cross-chain
messages, signer authorization gates and on-chain calldata unchanged.

## Not adopted

- The PR's action-manifest replacement changes cross-chain bytes and loses some
  entry shapes. This conflicts with the immutable-cross-chain scope.
- Its per-block transaction-count column costs approximately one byte for every
  empty block. Sparse gaps are materially smaller for the target workload.
- Its calldata-only zero-blob stream omits the blob portion's mandatory
  `CloseBlobStream` under the pinned specification. The existing native blob
  codec retains valid framing and exact rollup binding.
- Its single-operation parser decodes chain-specific bytes before selecting a
  rollup and cannot support multiple `ChainOperation` messages. The target
  deployment explicitly uses the narrower single-rollup native profile.
- It keeps the 512-block signer request default and has no incremental prepared
  validation path, so it does not solve the target 65,536-block proof window.

## Feedback for upstream

1. The fresh and resumed reconciliation skip paths must compare beneficiary and
   extra data before accepting a transaction-only match. The existing
   [review finding](https://github.com/eez-association/eez-rollup0/pull/152#discussion_r4037557245)
   remains valid at the reviewed head.
2. `action_from_entry` should reject unrepresentable destination, nested-call,
   static-call and revert-count shapes, and local-to-local must not be classified
   as inbound. The signer comparison limits the present impact, but unsupported
   entries otherwise fail late and the projection's stated lossless contract is
   false.
3. Protobuf `bytes` lengths must remain in the specified five-byte `u32` domain;
   the padded-length test must pad the original value rather than change it.
4. `encode` must reject a zero-length raw transaction because `decode` rejects
   the resulting payload.
5. Either specify a normative zero-blob framing exception or describe the
   calldata-only format as an interim Rollup0 profile.
6. Restore the ignored captured signer regression and obtain a completed
   cross-chain lane. The reviewed red lane ended with exit 143 while compiling,
   so it is missing evidence rather than a demonstrated test assertion failure.

The author correctly fixed terminal beneficiary selection and rollup-ID binding.
The dismissed legacy-format finding also does not apply to a new network.
