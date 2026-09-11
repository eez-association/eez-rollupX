# Production readiness TODO

This checklist tracks the work still required before the EEZ rollup can be
described as production ready. A successful disposable devnet or protocol E2E
run is evidence for an implementation, but does not by itself close any item
below.

## P0 — release blockers

- [x] Replace the deprecated Beacon API `blob_sidecars` dependency with a
  fork-aware blob retrieval abstraction that supports
  `/eth/v1/beacon/blobs/{block_id}` and a deliberate legacy fallback.
- [ ] Define and operate a durable historical blob source. It must support a
  fresh node bootstrap after normal consensus-client blob retention has
  expired, and must have authenticated access, integrity verification,
  backups, and a tested recovery procedure.
- [ ] Add a snapshot/bootstrap format and prove that a fresh node can recover
  from deployment using the snapshot plus the durable blob source.
- [x] Remove the implicit `MockEcdsaProver` production fallback. Composer startup
  requires a nonempty prover endpoint and attester address; the legacy mock
  verifier and deployment script are removed. Test doubles remain in unit tests.
- [ ] Verify the configured signer, attester key, verifier contract, proof
  system address, and verification-key identity against L1 during production
  startup.
- [ ] Decide and document the protocol guarantee for
  `immediateEntryCount`/`immediateStaticEntryCount`. If atomicity or eventual
  effect consumption is required, bind the scheduling decision
  cryptographically or make scheduling deterministic in the protocol.
- [ ] Update `crates/eez-proof-signer/SPEC.md` to the pinned protocol commit and
  current blob-DA/block-hash behavior, including protocol-version and migration
  rules.
- [ ] Obtain an independent security review of the blob commitment path,
  block-hash reconstruction, settlement validation, protocol materialization,
  and signer trust boundary.

## P0 — missing correctness evidence

- [ ] Run native semantic blobs through the complete live matrix: L1→L2,
  L2→L1, L1→L2→L1, L2→L1→L2, sibling calls, static calls, call failure,
  Snapshot/Revert rollback, multi-root transactions, and mixed-direction slots.
  Preserve blob bytes, decoded messages, signer decisions, receipts, and final
  state as evidence.
- [ ] Add live adversarial semantic-blob cases that mutate transaction origin,
  L2 `tx_data`, call fields, return/revert data, nesting, rollback spans, message
  order, and transaction boundaries; prove the signer rejects every mutation.
- [ ] Bind L1-origin semantic `tx_data`, transaction boundaries, and their
  relative ordering to independently observed L1 bundle transactions. The
  current signer proves their aggregate mutable/static tables and L2 deliveries
  exactly, but its settlement request does not include the later L1 user
  transactions needed to authenticate those three descriptive fields.
- [ ] Exercise partial settlement and deferred effect consumption with prefix
  block-hash commitments on a live network, then prove the materialized block
  is canonical and retrievable by `eth_getBlockByHash`.
- [ ] Repeat the isolated Anvil blob-reorg coverage against real execution and
  consensus clients: orphan a settled blob batch, replace the same L2 height,
  and verify L2 unwind/replay, safe-head convergence, canonical Beacon
  sidecars, and both correlation RPC directions.
- [ ] Inject process crashes during blob ingestion, settlement application,
  checkpoint persistence, and correlation-index updates; prove deterministic
  recovery after every crash point.
- [ ] Run a maximum six-blob batch near the L1 transaction gas limit and verify
  every KZG versioned hash, decoded L2 transaction, L2 block hash, and L1
  registry commitment.
- [ ] Test blob fee spikes, transaction replacement, delayed inclusion,
  duplicate delivery, and submission retry without duplicated L2 effects.
- [ ] Test Beacon API outage, timeout, malformed response, missing/pruned blob,
  wrong commitment, wrong slot, wrong chain, and partial multi-blob responses.
- [ ] Test both EIP-4844 and EIP-7594 transaction sidecar wrappers across the
  applicable fork transition using real execution and consensus clients.
- [ ] Run the complete upstream protocol E2E suite on the exact release commit
  in blob mode and remove, or formally document and upstream, every local
  compatibility override.
- [ ] Run the deployment and protocol suite on a public testnet with the real
  authenticated signer, including restart and resynchronization evidence.

## P1 — signer and service hardening

- [ ] Put the proof-signer gRPC service behind mTLS or an equivalent
  authenticated private transport; do not expose it directly to the Internet.
- [ ] Move attester keys to an HSM or production secret manager and document
  generation, access control, backup, rotation, revocation, and emergency
  replacement.
- [ ] Add request size limits, deadlines, concurrency limits, rate limiting,
  replay protection where applicable, and protection against a slow client
  monopolizing the signer.
- [ ] Add high availability without permitting two signer instances to attest
  inconsistent histories; document the failover and fencing model.
- [ ] Add metrics and alerts for signer decisions, validation failures, stale
  windows, proof latency, queue saturation, and key/configuration mismatch.

## P1 — storage, recovery, and correlation

- [ ] Make persistent correlation storage mandatory when the correlation RPC is
  enabled; never silently serve partial in-memory history in production.
- [ ] Add a correlation-index rebuild command, schema migration procedure,
  periodic/full consistency scrub, and backup/restore verification.
- [ ] Test corrupted, truncated, foreign-chain, and partially committed
  correlation databases and prove fail-closed recovery behavior.
- [ ] Add pagination and request limits to reverse-correlation RPC responses and
  clearly label them as derived settlement provenance rather than consensus
  proofs.
- [ ] Define L1 reorg depth, finality, checkpoint, pruning, and disaster-recovery
  policies for every persistent database.

## P1 — test depth and independent compatibility

- [ ] Add property and fuzz tests for blob framing, varints, field-element
  constraints, padding, malformed payloads, multi-blob boundaries, and
  encode/decode round trips.
- [ ] Publish independent Solidity/Rust golden vectors for single- and
  multi-blob payloads, KZG commitments, versioned hashes, public inputs, prefix
  block hashes, and final on-chain commitments.
- [ ] Add multiple competing composers/posters, overlapping batches, same-block
  submissions, and reorg/replacement races.
- [ ] Add sustained load, resource-exhaustion, long-soak, disk-full, database
  latency, network partition, and dependency-restart tests.
- [ ] Verify UI bridge, wrap/unwrap, flash-loan, nested call, revert, and wallet
  routing behavior against the exact deployed protocol version.

## P1 — release and operations

- [ ] Pin every production image by digest and every source dependency by an
  auditable immutable reference; remove floating `latest` and `develop` tags.
- [ ] Add dependency/license/vulnerability policy checks, reproducible release
  images, generated SBOMs, signed artifacts, and provenance attestations.
- [ ] Document production network topology, trust boundaries, exposed ports,
  firewall policy, RPC authentication/rate limits, and denial-of-service
  assumptions.
- [ ] Add dashboards, alerts, log retention, audit logs, readiness checks, SLOs,
  and an operator runbook for all critical services.
- [ ] Perform restore, signer-key rotation, L1 reorg, Beacon outage, and total
  node replacement drills before launch.
- [ ] Define release gates requiring exact-commit evidence artifacts for unit,
  integration, protocol E2E, blob E2E, reorg/recovery, public-testnet, and
  security-review results.
