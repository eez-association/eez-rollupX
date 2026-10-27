# Combined history, observations and cross-chain settlement

Status: combined live validation completed on 2026-10-25 UTC. The real Composer
request settled, its independent offline replay signed the correct result, and
all six wrong-state variants reached observation replay and produced no
signature. The complete updated-head CI remains a pre-merge requirement.

The experiment uses the isolated `codex/l1-history-offline-proving` worktree,
starting at `7558372be60b0f9edcd986f41e6676368bc04634`, with unchanged protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. Only dedicated Kurtosis enclaves
`eez-combined-history-755-20261025` and
`eez-combined-history-fixed-20261025` were operated. Existing networks and the
original checkout were untouched.

## Findings and scope

A two-second, six-slot network with `historyUpdateOffset=5` puts a history import
at the beginning of a rich Sync block. Actual Composer policy places ordinary
mempool transactions in preceding blocks of the proven window; it does not
produce the proposed `[history, ordinary mempool write, cross-chain effects]`
body. The live workload respects that policy. Separate signer regressions cover
that more general body.

The real experiment found two fail-closed Composer defects:

1. Group execution checkpoints index the entire block, but their final comparison
   used positions relative to the cross-chain suffix. A scheduled history prefix
   made every valid group fail that comparison and fall back to minimal posting.
   The fix adds `history_prefix.len()` to the canonical pair positions.
2. Composer's local observation replay selected the first native `System`
   transaction for B[0]. That selected transaction zero when history was present,
   although composition had correctly executed history in its initial draft.
   The fix uses `is_cross_chain_system()`, matching the already-fixed signer's
   independently validated boundary. The original draft overlay was correct.

Both restore existing B[0] and checkpoint requirements. They introduce no wire,
profile, genesis, production contract-runtime or protocol-pin change.
Unsupported requests and malformed observations remain rejected.

The L2 test probe exposes a marker plus the current L1 history endpoint. An L1
reader makes a static observation, catches a failed mutable lookup with the same
bytes, performs a real L2 write, and observes the new marker. The failed lookup
is distinct from the successful lookup under a reverted source frame in the
older signer fixture.

## Artifact and failure provenance

Baseline runtime bytes were fetched from CI run `40220870906`, producer
`121251960869`, and checked with `prepare-runtime-artifacts.sh` against its
merge commit `4306be37743829705f910b3ef14161a4ffe185ac`. Its tree equals the
requested `7558372...` tree. Node, signer and deploy digests were verified;
using a tag or reading producer logs alone was not treated as verification.

| Artifact | Identity |
| --- | --- |
| CI runtime payload | `sha256:d90f13eabadaeec3d923c157d3d35ffd72c1c95589b6d96f3a0a316016fb600b` |
| Baseline node image | `sha256:a4967544922dba917252a8e0161089c0a6b6c0983ac6bc8bc6989849fde0b811` |
| Signer image | `sha256:2dd43610fe904d17b4a96cc8bb6f99c2475318b82310d857ddf221b8c02ac932` |
| Deploy image | `sha256:144e64088bc57aa10369729b783fb0f2ed80e06181f777578ea4c83c3ec6086f` |
| First Composer patch (positions only) | `7deee67a287799d971697ef987cdff00c9c75570cc06bc6744d3609f920bae1c` (SHA-256) |
| First local Composer binary | `afa4fc771d94b5b44d7f63d242cf9788eb97b477e9fd01efdc0e15c5ba8ad7aa` (SHA-256) |
| Both production fixes | `9c71ed9a970a1d960414cfb0ffe16570e84b69aba0c85c1186c99c2739536a5b` (patch SHA-256) |
| Final local Composer binary | `6071e891f2d95f66a22721dc27caf5c115fecad662398a2278563706c497e03f` (SHA-256) |
| Final E2E executable | `cb2c8c9842855126e89e953d31945145d915b622ce2e3e32241bfa15b397469a` (SHA-256) |
| Independent signer executable | `43680285f6ab00260e91201bf80d545a3b332a2094ab6a525135176b8a52c1cf` (SHA-256) |

The candidate Composer uses a local Rust 1.95.0/LLVM 22 development build,
`--locked`, with one build job. It is explicitly distinct from CI's release
candidate. The test adapter has no signing logic; the signer and deployment
artifacts remain the verified CI bytes.
The local runtime was copied before compiling the E2E target, and both process
and live tests used that preserved copy. The test compilation's different
feature selection did not substitute its resulting runtime executable.

Local-only evidence is under `artifacts/combined-history/` in the isolated
worktree. It is ignored by Git and unavailable from another checkout:

- `ci-runtime/manifest.json`, `runtime-fetch.log`, and `runtime-verify.log`:
  producer payload and verification.
- `baseline/summary.json`: 18 consecutive real position-comparison failures,
  first at 19:27:05 UTC, with node/signer log digests. The enclave was stopped,
  with its data retained.
- `before-process/summary.json`: the non-ignored q4 bidirectional regression
  against the same baseline runtime; 18 matching failures and an inbound receipt
  timeout after 90 seconds.
- `history-overlay-before/summary.json`: the first candidate's local observation
  mismatch at 19:55:03 UTC. The directory name is historical: the overlay did
  include history; the replay index was wrong. Expected output hash was
  `dabad86316c56776e7ccdcc9072eb53157e5b42b087af861b8ec0a0bc45d3d62`;
  replay returned `d5c97e591614cd903ad42cb303888751d3c4f251e32576879847a71f65f8b57f`.
  The request was evicted without reaching signer finalization. Logs, captures
  and the stopped node's `/data` snapshot were retained.
- `observation-before-process/summary.json`: the new history-sensitive process
  test also reproduced that local replay mismatch against the first candidate.
  The identical E2E executable passed against the final Composer.
- `capture-3/combined-regression/`: the selected live request, six coherent
  wrong-state variants, accepted-ancestry selection and per-case outcomes.
- `final-live/`: final node, signer and capture-adapter logs, safe/latest block
  records and successful enclave stop. Both task-owned enclaves are stopped;
  their data and first failures are retained.

Fresh PBS infrastructure initially had no validator registrations and excluded
bundles. Registrations subsequently appeared without a configuration change.
This separate readiness condition does not explain either Composer defect.

## Reproduction and checks

The maintained live procedure is in the [Kurtosis guide](../../../testing/kurtosis/README.md#capture-history-and-cross-chain-observation-replay).
The CI process regression uses a one-second, five-slot fixture with offset 4,
so the scheduled import likewise falls in Sync. The history-sensitive regression
locates the exact probe delivery in accepted safe ancestry, rather than assuming
the latest safe block is that delivery.

| Check | Observed result |
| --- | --- |
| Baseline q4 bidirectional process regression | Failed as expected: source could not settle; 18 position errors |
| First fixed q4 process attempt | State, both directions, root and signature checks passed; final new harness assertion failed because `debug_getRawBlock` was unavailable |
| Same first fix with `eth_getBlockByHash(full)` assertion | 1 passed, 181 filtered; 36.54 seconds |
| History-sensitive static/lookup process, first candidate | Failed at the expected local replay mismatch; 43.42 seconds |
| Identical history-sensitive test, final candidate | 1 passed, 182 filtered; 47.88 seconds |
| Final q4 bidirectional process regression | 1 passed, 182 filtered; 37.33 seconds |
| Final default-offset bidirectional smoke | 1 passed, 182 filtered; 37.15 seconds |
| Real Kurtosis workload and capture | Passed; three included source transactions, exact reader counts 1/2/3; 64 seconds |
| Cold offline replay of real capture and six mutations | 1 passed, 182 filtered; 2.44 seconds; one signature and six observation-replay rejections |
| Composer and testkit unit suites | 410 + 20 passed; one manual timing profiler ignored; no failures |
| Workspace Clippy, all targets, warnings denied | Passed; Rust 1.95.0, locked dependencies, 130 seconds |
| Integration Foundry suite | 142 passed, 0 failed/skipped, 16 suites |
| Kurtosis Python tooling | 22 passed; 2 optional live-resource probes explicitly skipped |
| Genesis fixture consistency | Passed; existing genesis/runtime fixtures unchanged |
| Final Rust/Solidity formatting, shell syntax, diff and documentation links | Passed |

The first harness failure is retained. A source correction to the assertion,
not an unqualified retry-only pass, enabled the subsequent complete process
result. Commands, test selection, log digests, runtime identities and outcomes
are retained in the small [evidence record](evidence.json). The new process
tests are non-ignored members of the existing `node_e2e` target and therefore
enter the normal cross-chain CI partition inventory.

## What the live replay established

The selected terminal is L2 block 786,
`0xe955ff4aa3f0cade9328255895e6f22dfbabe06c8c534065245de8f591591dab`.
The replay test checked its canonical hash and walked parent hashes from safe
block 798 to that exact terminal. It is an accepted ancestor, not an arbitrary
speculative capture. This establishes safe ancestry, not L1 finality.

Its history import advances L1 history from block 130 to 131. Ordinary writes
starting at L2 block 781 leave marker 12 at B[0]. That prefix contains the
history transaction; the subsequent cross-chain delivery sets the marker to
1,000,000. Both the static call and failed mutable lookup observe marker 12
with the new L1 block hash. The post-delivery static call and canonical
terminal-state query agree on the later marker. Three source calls settled in
L1 blocks 132–134, with reader counts checked as exactly 1, 2 and 3.

A separate, cold signer used the existing seccomp launcher to deny outbound
connections. Its environment was cleared and contained no L1/L2 RPC setting.
It reexecuted seven blocks in each case using the captured witnesses. The
correct request produced the captured public-input hash and a signature
recovering the configured attester. For each static/mutable lookup, the test
substituted the result from before history, before the ordinary window, or
after the cross-chain effect. Each mutation updates both ABI and native
semantic data and reconstructs blobs, so all six pass earlier gates and fail
specifically at `observation_replay`. Actual signing events remain exactly one
after every negative case.

The public context, seven length-delimited v2 frame streams and expected
outcomes are also packaged locally as
`artifacts/combined-history/combined-history-v2-conformance.tar.gz`
(229,941 bytes; SHA-256
`d472e4d261b61e95e5134d2fbecdfdc67758c3cddcd496a7f11bf45fb2970c0f`).
This can be reused by another prover implementation; no C# implementation was
executed. The archive is local-only, not committed or published. Its README
describes session identity rebinding and exact expected responses. The original
selection JSON is preserved byte-for-byte; the evidence record converts its
OS-string filename representation to a readable string.

## Implementation, review and limits

The only production edits are the two local fixes in
[Composer](../../../crates/eez-composer/src/composer.rs). Existing `eez-testkit`
capture and process infrastructure supplies the adapter, optional history
offset and test fixture deployment. The optional Kurtosis capture service and
script support repeatable live evidence; default network behavior is unchanged.
The new fixture contract is application test code, not a predeploy.

Fresh cross-review checked the observation boundary for history plus effects,
history without effects, and no history. With no cross-chain suffix the entire
transaction count remains the prefix; effect pins still use validated absolute
checkpoint positions plus one. Unknown pins, out-of-range positions and
non-FIFO schedules still fail. The signer independently derives its boundary
from validated execution; the Composer does not supply a trusted replay index.

The live production policy does not place ordinary mempool writes inside rich
Sync, so this experiment must not be described as that exact body shape. Its
before-ordinary negative case addresses the preceding proven window. The
existing signer regression separately covers history, an ordinary write and an
optional native suffix in one body. No producer policy was changed to make a
test pass.

Complete CI against the updated head, release-artifact verification and
deployment of that tested artifact remain required. These development
process/live results do not substitute for those gates, a new performance
comparison, a new comprehensive fork/reorg campaign, or an independent review
of unrelated prover paths. No existing network was upgraded.
