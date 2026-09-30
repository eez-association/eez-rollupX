# Implementation and upstream mapping

This is the initial backport inventory. The subsequent [Reth v2.7 audit](../2026-09-29-reth-v2.7/README.md) records the final PR #42 base, published Git dependencies replacing vendor copies, validation identity schema 4, and later fixes. Dependency and revision details below describe the original implementation.

This inventories the task's delta against PR #42 at `0b7beed5ca48157d720d6685435ba2121c1e9c1e`.
It is not a validation result; revision-specific executed evidence is in the
[audit](README.md), and the accepted task contract is in the
[proposal](../../proposals/native-system-transactions.md).

| Upstream requirement | Target component | Disposition |
| --- | --- | --- |
| #154 unsigned type 0x76, fixed sender, nonce, exact body/hash | `eez-primitives::{SystemTransaction,EezTxEnvelope}`, native receipt and engine modules | Adapted to PR #42's Reth/Alloy graph |
| #154 fixed 2M metering, zero price/fees, mint and outer-failure rollback | `eez-evm::{EezEvm,EezEvmConfig}` | Adapted to REVM 43; direct TxEnv inputs checked too |
| #154 normal fees and retained outbound ETH | `eez-evm` execution adapter/tests | Ported; ordinary Ethereum execution remains delegated |
| #154 native receipts/storage/RPC and public admission rejection | `eez-primitives`, `eez-node-common::{node,pool,pool_transaction,rpc,payload_builder}` | Adapted; pooled wire remains Ethereum-only |
| #154 no L2 blobs; preserve L1 DA blobs | Shared supported-transaction checks, pool validator, Composer builder and replay | Adapted; L1 simulation has its own execution profile |
| #154 one deterministic system reconstruction | `eez-protocol::system_tx`, Composer `local/build`, deriver, signer `system_transactions` | Adapted to whole grouped/nested calls and #42's prefix cache |
| #154 stateless generic primitives/receipts | Narrow patch to vendored Stateless `ffe41a3`, signer adapter | Adapted without Reth/REVM/Alloy upgrades |
| #154 separate upstream prover roles | Existing independent `eez-proof-signer` | Obsolete structure; behavior integrated into fork architecture |
| #154 keys/prefunding removed; new genesis | Runtime/configuration, genesis generator/fixtures, Compose/Kurtosis consumers | Ported; L1 poster and proof-attestation keys retained |
| #154 Rustls advisory update | Existing PR #42 lockfile already has Rustls 0.23.45 | Already present; no incidental dependency update |
| #162 checked inbound transport | `eez-protocol::entries::InboundSidecar`, semantic materializer, Composer, deriver and signer | Adapted to nested-capable pinned ABI; no supplied-field normalization |
| #162 explicit direction throughout reconstruction | `ReconstructionActions::{outbound,inbound}`, existing `OutboundSyncGroup` and checked callback deliveries | Adapted using existing grouped architecture; no second generic entry enum |
| #162 shape versus settlement authorization | Signer `inbound`, `outbound`, `semantics`, `da`; shared semantic settlement binding | Preserved and extended to direct V0; exact call/rolling hashes and bytes remain mandatory |
| #162 nested GASPRICE, reimbursement and positive-price guards | `eez-evm/src/tests.rs` | Ported with ordinary paid-transaction comparison |
| #35 two-byte positive counts and maximal empty runs | `eez-payload-codec::v0::{counts,...}` | #37's count semantics retained, moved into the direct body |
| #35 direct V0 columns, full-u32 bounded parsing, exact sizing | `eez-payload-codec::v0`, `BlockDaSizer`, driver encoding, Composer policy/admission, payload final checks | Implemented; old outer RLP/full-block fallback removed |
| #35 fixed execution/header profile and genesis | `eez-protocol::l2`, `eez-evm::profile`, node-common chain/RPC loaders, driver assembly | Implemented; EIP-2935 retained, beacon/request side effects disabled, empty requests hash retained |
| #35 authenticated live-anchor seed, reorg/restart, complete Sync intervals | `eez-l1::derivation`, canonical cursor, deriver correlation, driver/composer environment wiring | Implemented; no unverified transported seed or old checkpoint fallback |
| #35 independent signer parent and target binding | `service/l1`, RPC/worker handoff, shared public inputs, `prove.proto`, Solidity vectors | Implemented against the pinned manager's latest-context primitive |
| #35 terminal ordinary prefix B[0] and all applied action variants | Composer checkpoints, Stateless pre-transaction checkpoint patch, signer effect binding and follower resume | Adapted while retaining block-hash commitments and the fork's failed-outbound carrier |
| #35 draft 0x45/no-nonce/dynamic-gas contract | Owner-approved amendment to 0x76/nonce/2M | Intentionally superseded; no dual contract |
| #35 inbound-only draft profile and wider section F | Explicit richer-action extension and F1–F4 disposition table in proposal | Not claimed as full draft conformance; external manager/trigger/availability work remains separate |

## Purpose of the file groups

- `eez-primitives` and `eez-evm` contain the native format/execution adaptation.
  Their tests cover independent wire/hash vectors, malformed envelopes, fixed
  environment fields, mint rollback, gas-price behavior, normal fees and blobs.
- `eez-protocol` owns checked inbound tables, deterministic transaction construction,
  semantic action/settlement binding, fixed chain rules and latest-context hashes.
  Its existing ABI, rolling-hash and public-input oracles remain independent.
- `eez-node-common`, `eez-node` and `eez-follower` wire those primitives into Engine,
  RPC, pool, payload building and witness capture. Pending RPC simulation derives
  the same next timestamp and seed; completed payload checks charge V0 columns.
- `eez-payload-codec` replaces the old operations envelope. Its standalone #35
  vectors protect the 31-byte/600-empty-block example, resource bounds and canonical
  count/metadata/transaction framing. `eez-driver` executes and assembles every
  block, including empty ones, from those inputs.
- `eez-l1` authenticates the genesis reference, accepted endpoints and live-slot
  occurrences. `eez-deriver` persists/rebuilds that provenance with the canonical
  cursor and reconstructs/resumes whole action groups. Removing its old one-record
  checkpoint avoids a competing seed source.
- `eez-composer` adapts existing FIFO admission, prefix snapshots, DA sizing and
  proving to the new format. PR #42's cache, deadlines, queue ownership and bounded
  skipped-slot replacements remain the base implementation, not duplicated work.
- `eez-proof-signer` binds independently replayed blocks, typed effects, actual V0
  bytes and canonical L1 facts before signing. New native captures replace obsolete
  positive fixtures; the test-only old DA decoder is removed.
- `eez-control-rpc`, `eez-prover` and `eez-prover-client` carry parent provenance,
  required posting context and validation identity schema 3. Protobuf is generated
  by the existing Cargo build, never hand-edited.
- `eez-testkit` and `eez-node-e2e` provide keyless multi-node, late-follower and cold
  L1/DA-only replay coverage with the required genesis/target context. The existing
  queue/reorg/partial-consumption cases exercise the adapted runtime.
- `vendor/stateless` retains its exact upstream source with the documented generic
  primitive/checkpoint patch. `Cargo.toml`/`Cargo.lock` use local paths for those
  packages and add direct references to versions already resolved by PR #42.
- Genesis/configuration/deployment files remove the system key and allocation,
  require the new execution profile, and render separate new-network artifacts.
  Operator documentation describes matching-version startup and the rollback
  boundary; this task performs no deployment or existing-network reset.
- CI's native test inventory includes the new `native_system` process target and
  `issue35` vectors. Exact-artifact verification and four-slice consumers remain
  unchanged. `Dockerfile.runtime` includes the root genesis used by the new
  profile regressions. Tooling fixtures reflect removed configuration inputs.
- Specifications and signer guides document the new validation boundaries and
  fresh-network contract. Historical proposals/audits remain labeled as evidence
  for their original revisions, with links to the current contract.

## Acceptance evidence by invariant

Counts and exact command results are in the audit; this table identifies the
regressions that exercise each boundary, rather than treating compilation as
behavioral evidence.

| Requirement | Executed regression boundary |
| --- | --- |
| N1: native identity | `eez-primitives` independent wire/sender/hash/storage vector, malformed and trailing-byte rejection, native receipt round trip |
| N2: gas and ordinary fees | `eez-evm` raw-environment rejection; outer/nested GASPRICE, reimbursement and positive-price guard comparison; ordinary fee deduction |
| N3: mint atomicity | `eez-evm` success, revert, halt, overflow, invalid nonce, preview and inspector equivalence; process inbound value/calldata atomicity |
| N4: admission and headers | Ethereum-only pooled decoding/conversion; EVM blob and beacon-withdrawal rejection on build/import/Engine paths; all-four-node RPC rejection; empty requests-hash preservation |
| N5: role and nonce agreement | Two producing Composers, two followers and late join; cold Ethereum/Gnosis L1/DA replay; multiple inbound groups, partial consumption, same-block resume and restart; captured real signer fixtures |
| T1: checked inbound transport | Original ABI mutation tests for destination/source/root updates, shape, call hash and flat rolling hash; checked type retained through action reconstruction |
| T2: independent settlement authorization | Signer tests for settlement substitution, identities, call/rolling hashes, complete nested tables and exact reconstructed bytes; real-process tampered PostBatch and witness rejection |
| D1: complete V0 | Exact #35 31-byte/600-empty fixture fails on unchanged #42 decoder and passes here; shared vectors, exhaustive counts and independent sizing/concatenation properties |
| D2: bounded parsing | Full-u32 empty spans represented lazily, malformed/overlong/overshoot/length-sum cases and arbitrary-byte properties; explicit request quotas and cancellation tests |
| D3: derived environment and seed | Independent header/schedule vectors, complete live-slot endpoint proof tests, four-node seed refresh, L1 reorg, partial-consumption restart and clean L1-only replay |
| D4: all terminal variants | Independent paid-prefix plus two-native-action trie/receipt/checkpoint oracle; changing-static and caught-revert process cases; exact-once same-block partial resume |
| D5: fixed lifecycle | Real EIP-2935 predeploy updates on empty blocks with adversarial beacon/request predeploy state; canonical Engine/header/genesis validation and retained L1 execution profile |
| B1: PR #42 preservation | Composer cache/FIFO/ownership unit suite; five skipped-slot, historical-prefix and late-original process cases; chained/partial/reorg process suites |
| C1: fresh-network contract | Regenerated genesis verification; deployment/configuration/tooling regressions; root genesis included in actual exported CI source stage |

The two old Solidity formatting differences, ignored utilities, inherited dependency
warnings, unrun full release/Kurtosis matrix and wider draft section F dispositions
remain explicit audit limits. Passing the rows above does not assert those separate
checks or protocol requirements are complete.
