# L2 fee gas-cap correction

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; deployed and post-deployment regression checks passed |
| Scope | Align unsigned quotes, signed tariff admission and the SDK with the active 16,777,216 transaction gas limit |
| Runtime baseline | `246514ed1da11eb9fe51616f0b65eddaccbaea4d` |
| Runtime fix | `14d9eac364d92d316da4f00a4cb44154d9c28661` |
| Process regression source | `f12a227567cd01eb1380a18a044e0a81ab1b54c6`; production code identical to the runtime fix |
| Protocol | Initialized, unchanged `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Dashboard | `d2e85905864ec88de493829d763a8cfeb9a45b99` |
| Environment | Existing ROG Chiado L2; Rust 1.95.0, Foundry 1.7.1, LLVM 22, locked dependencies |
| Contract | [Accepted correction](../../proposals/cross-chain-fee-recovery.md#accepted-correction-full-source-transaction-gas-range), [quote specification](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes) |

## Finding and deployed change

The application quote parser imposed a 5,000,000 source gas limit below native
execution's active EIP-7825 cap. Signed tariff admission reused that parser, and
the SDK repeated its bound. Recorded Falcon Safe creation and transfer needed
limits of 6,050,247 and 6,037,754, respectively. Lowering those submitted limits
would not address the workload. The old live endpoint reproduced
`invalid_intent: gas must be 21000..5000000`.

Composer now uses REVM's canonical EIP-7825 constant, 16,777,216. Both SDK copies
use that same numeric bound. The lower bound, strict schema, execution work
limits, pricing, final native validation and signing contract are unchanged.
The actual fee continues to use simulated gas consumed; a larger submitted
limit still increases the wallet's maximum fee reservation.

The user explicitly authorized expedited deployment before broader tests. SDK
boundary checks, formatting and the release Docker builds preceded the handover;
the Rust suites and process regressions ran afterward. The old deployment's
Kurtosis results were not used to claim validation of these new binaries.

## Operations and identity

The operator checkout's unrelated edits and older protocol pin were preserved.
Implementation used `/tmp/rollupx-l2-fees-e2e.8oFEZzNX`; the dashboard used
`/tmp/eez-ui-l2-fees.Qm8H2JyN`. The deployment retained operator configuration,
policy, genesis and existing contracts. Only the image references and separate
runtime database paths changed.

Public HTTP and WebSocket transaction routes were paused, the accepted queue
was confirmed empty, and node/signer exited cleanly. Complete stopped databases
and configuration were retained before starting copies with the new images.
Signer health, synchronized L1, advancing L2, canonical safe-block correlation,
static discovery and usable quotes were checked before reopening public routes.
The dashboard was healthy and returned HTTP 200. No contract deployment, wallet
funding or public test transaction was performed during this correction.

[results.json](results.json) records exact image and executable identities plus
sanitized checks. The signer and follower executable hashes equal the previous
release; the Composer executable changed. Local images were built explicitly
from the recorded source; no public deployment tag was promoted or rebuilt.

## Validation

The live endpoint returned quotes at limits 200,000, 6,050,247, 6,037,754 and
16,777,216 for both legacy and type 2. It rejected 16,777,217 with the specific
updated range error. Public-route checks repeated the cap/cap-plus-one cases.
All eight successful quote snapshots passed SDK validation at their issuance
time. For the same nested call, source gas stayed 99,149 and required settlement
contribution stayed 365,720,401,459,038 base units across all four limits.

| Check | Result |
| --- | --- |
| SDK: `node sdk/l2-fees.test.mjs` | 13 passed; legacy/type-2 bounds, recorded Safe limits, app fee ceiling |
| Composer: `cargo +1.95.0 test --locked -p eez-composer --lib` | 346 passed; one existing manual timing profile ignored |
| Node: `cargo +1.95.0 test --locked -p eez-node --lib` | 85 passed |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| Native `node_e2e`, `fee_quotes:: --test-threads=1 --nocapture` | Six passed; no failures or ignored selections, 138 unrelated tests filtered out |
| `cargo +1.95.0 fmt --all --check`, `git diff --check` | Passed |
| `forge build --root contracts` | Passed; existing artifacts current |

Process tests used the exact deployed Composer, follower and signer executables
extracted from their containers and hashed in `results.json`. The independently
built harness came from the recorded process regression source. It was prepared
with `cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e --no-run`, then
run with `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` pointing to those
extracted executables, and `EEZ_TEST_PROOF_SIGNER_BIN` naming the matching signer.
Explicit log/datadir directories retained diagnostics. There were no diagnostic
retries or retry-only passes.

The updated enforced nested-call cases submitted and canonically settled a type-2
source at 16,777,216 and a legacy source at 6,050,247. They checked underpayment
rejection, target state, receipt-backed tip accounting and independent follower
replay. The other four cases covered advisory quotes, entirely-L2 Safe behavior,
and nested Safe initialization on Ethereum and Gnosis L1 profiles. These Safe
cases use the existing Safe 1.4.1 tests, not Falcon signature verification.

## Evidence and limits

Raw evidence is local-only under `artifacts/l2-fee-gas-cap-20261015/` in the
operator checkout, including baseline/live responses, build/test logs, executable
hashes and process diagnostics. It is ignored by Git and unavailable to another
checkout. Small sanitized results and hashes are committed in `results.json`.
Private configuration backups and complete databases are retained separately.

The live higher-limit checks used disposable nested-call simulations, not a new
Falcon signature or a new Falcon Safe deployment. They prove the quote profile
accepts the recorded limits, not that every Falcon invocation finishes within
the unchanged execution time/frame limits. No new Kurtosis topology/DA/PBS run
was performed for this cap-only correction. The older measured-pricing audit
remains separate evidence for that earlier release, not this one.
