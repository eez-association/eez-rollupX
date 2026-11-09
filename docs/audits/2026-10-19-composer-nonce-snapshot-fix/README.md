# Composer source nonce/snapshot correction

Status: all targeted local checks passed, including exact-image Kurtosis
qualification in both fee directions. Runtime `de4209a5` is deployed to
`eez.asuscomm.com`; canonical settlement and public quote checks passed.

The operator authorized the Composer fix, explicit/omitted nonce regressions,
Kurtosis validation and deployment after the applicable checks pass. The protocol
checkout remains unchanged. The [baseline investigation](../2026-10-19-composer-quote-nonce-gap/README.md)
records a real native settlement whose scan lagged the canonical L1 account nonce.

## Identity and change

- Candidate: `de4209a5` on `feat/pr54-kurtosis-fees`; exact SHA and log digests in
  [validation.json](validation.json).
- Protocol: `f46af7c9833ac12a54f14d52f4da1f318a0f6525`, initialized and unmodified.
- Deployed baseline runtime: `110f952143853d0edbc51d6d8c2cb67be4d828f3`, node
  image `sha256:2fc8c6e121d1bd2116a0f78766ebd7c99b305e8e01720a2c91e693b21908a0b6`.
- Relative to that runtime, production Rust changes are confined to Composer's
  fee-quote context checks and the node's bounded fee workers. Cargo manifests,
  lockfile and genesis are identical. The protocol changes only test tooling,
  documentation and Foundry read permissions; executable contracts/compiler
  settings are unchanged.

Before nonce selection and after calculation, compare the sender at the sampled
L1 hash and current L1 hash. A mismatch returns structured `stale_context`, rather
than interpreting an already-mined ordinary predecessor as a missing queue entry
or returning a spent omitted nonce. Failed calculations receive the same final
check. Genuine gaps still fail, and held-pool selection/replay is unchanged.

The node refreshes only the typed mismatch detected before execution, up to eight
times at 250 ms intervals under the original RPC timeout and permits. A mismatch
after execution is returned without an internal retry, preserving the aggregate
frame budget. The [contract](../../configurable-settlement-policy.md#compact-fee-quotes)
and [authorized design](../../proposals/cross-chain-fee-recovery.md#l1-source-snapshot-correction-authorized-on-2026-10-19)
describe additive diagnostics and mixed-version/rollback behavior.

## Checks and retained evidence

Commands ran from the isolated candidate worktree with Rust 1.95.0, LLVM 22 and
the existing Cargo target cache; dependencies remained locked:

```bash
cargo +1.95.0 fmt --all --check
cargo +1.95.0 test --locked -p eez-composer -p eez-node --lib
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
node sdk/l2-fees.test.mjs
node --check testing/kurtosis/scripts/verify-nonce-snapshot.mjs
git diff --check
```

Composer: 381 passed, one deliberately ignored manual timing profile. Node:
93 passed. SDK: six passed. Workspace Clippy and formatting passed. The initial
sandboxed node run could not bind sockets in ten HTTP tests; repeating with
localhost access passed all tests. Both runs are retained; this is an environment
restriction, not evidence that skipped socket tests passed.

Regressions cover explicit and omitted nonces, hash-bound state access, unchanged
source nonce across advancing heads, real future gaps, bounded refreshes,
non-retry after execution, and permit retention through cancellation/timeout of
the original and refreshed worker. Existing pending-prefix tests remain selected.

The production builder is `scripts/ci/build-runtime-artifacts.sh` with publication
disabled. It packages the same release bytes into local node/signer images and
native test artifacts. No shared tags are promoted. Kurtosis runs the new
[`verify-nonce-snapshot.mjs`](../../../testing/kurtosis/scripts/verify-nonce-snapshot.mjs)
against baseline and candidate with fee collection enabled. The sealed release
producer passed 30 protocol/vector/blob tests. All 15 native `fee_quotes::`
process tests passed with zero retries, including explicit/omitted queued-prefix
replay and independent replay checks; 146 unrelated tests were filtered out.
The release producer's first invocation failed preflight because `GITHUB_SHA` was
missing, before any test selection; the corrected invocation and the failure
are retained. [Runtime provenance](runtime-provenance.json)
binds the images, binaries and test archive to the candidate.

### Same-workload baseline and candidate

Both enclaves use the same fee-enabled topology and fixture. The test combines an
ordinary source-account L1 transaction with a companion cross-chain source in the
same block, then immediately requests compact/debug quotes with an explicit and
an omitted nonce. It does not reserve a nonce or submit the quoted transactions.

The old runtime reproduced the bug in the first round: canonical nonce **4**, both
explicit requests rejected with `queued nonce prefix has a gap`, and both omitted
requests returned already-spent nonce **3**. The identical explicit request later
succeeded. [Baseline evidence](baseline.json) preserves those responses.

The candidate passed six rounds and all 24 concurrent race-window requests,
returning the correct nonce and both fee encodings. All six genuine future-gap
controls still rejected; read-only quotes left the sender nonce and target effects
unchanged. The log contains 164 internal snapshot refreshes, proving that the
stale path was exercised. The final companion value **89** was independently read
at L2 `safe`. [Candidate evidence](candidate.json) retains results and log identity.
This is a correctness regression, not a throughput benchmark.

The first baseline attempt started before PBS validator registration and timed
out waiting for inclusion. Builder logs showed no registered relay data. The
test now first requires a canonical ordinary-L2 warmup settlement, and the
corrected baseline run reproduced the bug. Preserve the initial setup failure;
it is not a nonce regression result. The readiness refinement changes only the
test harness, not the built runtime.

Raw build/test logs and enclave data are local-only under
`artifacts/nonce-snapshot-fix/`; the larger runtime artifacts are retained in the
operator checkout's `artifacts/nonce-snapshot-fix-20261019/`. They are not available
from a clean clone. Sanitized summaries and content hashes are retained here.

### Fee-enabled qualification

The existing L1 fee exercise passed four individual sources (nested calls and
Safe creation, each legacy/EIP-1559) plus two batches of four independent sources.
Assertions verified canonical effects, the ordered native blob post, builder
payouts from the largest measured tip, and continuation after a measured shortfall.
The Safe fixture is **Safe 1.4.1 with an EOA owner**, not the Falcon workload.

The L2 exercise rejected a deliberately underpaid source without accepting queue
ownership or executing it. Both funded nested sources then settled in one native
block. Canonical receipts, target effects and pooled fees covering the complete
post/blob tariff were verified. [Fee evidence](kurtosis-fees.json) preserves the
measured costs, payments and transaction identities.

Commands used the same candidate enclave and deployment bindings; the scripts
ran sequentially to avoid sharing fixture nonces concurrently:

```bash
export KURTOSIS_ENCLAVE=eez-nonce-candidate-20261019
source testing/kurtosis/ports.sh
# Export this enclave's deployment bindings and public fixture key.
# Use a separate EEZ_FEE_EVIDENCE_FILE for each exercise.
node testing/kurtosis/scripts/verify-l1-fees.mjs
node testing/kurtosis/scripts/verify-l2-fees.mjs
```

### Scope limits

The full remote CI matrix, full protocol stress matrix and Falcon workloads were
not rerun. PR #54's separate retry-classification issue remains with the protocol
engineer; this change does not modify its client. Native fee process tests include
independent replay checks; this Kurtosis run asserts canonical effects but does
not add a separate follower replay exercise. These are targeted deployment checks,
not qualification for promotion of shared release tags.

### Verified Chiado handover

The exact tested local node/signer images were selected by their verified
manifest; no registry tags were promoted. Existing node/signer command lines,
effective environment, fee settings, deployment bindings and genesis were
preserved. Public transaction HTTP/WebSocket routes were placed in maintenance,
both held queues and the ordinary transaction pool drained, and the captured L2
watermark was required to become canonical before stopping either process.

Both processes stopped cleanly. Complete stopped L1/L2 databases were copied to
new paths; originals, old image identities, environment and gateway configuration
were retained privately for recovery. The candidate was started with
`--no-build --pull never`, with no contract deployment or genesis replacement.
Running Composer/signer executable hashes match the frozen test artifacts.

Initial live checks passed: both directions, compact/debug methods, explicit and
omitted nonces, two fee encodings (eight quote calls, 16 options), with no
transaction broadcast. Prior safe and handover block hashes/state roots were
retained. The safe L2 head then advanced from `0xf8c0f` to `0xf8caa`, establishing
canonical settlement by the new runtime. This observation waited for the existing
150-unsettled-block trigger; the 900,000 ms pure-L2 interval was unchanged.

Public routing was restored byte-for-byte. The same eight quote checks passed
through the public HTTPS endpoints, covering both directions and both nonce
forms with 16 valid fee options. Both source accounts were unchanged; these live
checks broadcast no transactions. [Deployment evidence](deployment.json) records
image identities, running binary hashes, safe state, preserved settings and
sanitized quote outcomes.

The image references are unique **local** tags, not published releases. Preserve
these images and the recorded runtime artifacts on this host; registry pulls on
another host cannot retrieve them. Restart uses the preserved configuration and
`--pull never`. A rollback would restore the old nonce/snapshot bug. Do not replace
the now-active databases with the original stopped copies after accepting new
work; those copies are a historical recovery point, not current queue ownership.
