# Measured L1 settlement quotes and Chiado deployment

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; implementation, isolated validation and authorized Chiado deployment complete |
| Scope | Replace admission gas reserves in L2-to-L1 pricing with measured complete L1 posting gas plus physical blob cost and the existing 20% trial allowance |
| Runtime source | `eez-association/eez-rollupX`, clean `246514ed1da11eb9fe51616f0b65eddaccbaea4d`; implementation `94569105`, receipt regressions `246514ed` |
| Protocol | Initialized, clean `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`; unchanged from the existing deployment |
| Dashboard source | Clean `d67d16ba8a539a7806fe8702c4796907704d45f2` |
| Environment | Rust 1.95.0, Foundry 1.7.1, LLVM 22; locked dependencies; fresh private Kurtosis network and the existing ROG Chiado L2 |
| Related contracts | [Accepted correction](../../proposals/cross-chain-fee-recovery.md#accepted-correction-measured-l1-settlement-pricing), [quote specification](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes), [deployment runbook](../../runbooks/chiado-deployment.md#recovery-and-upgrades) |
| Earlier evidence | [Original bridge reconciliation](../2026-10-15-l2-fee-deployment/README.md), including its retained receipt analysis |

The operator checkout had unrelated changes and an older protocol gitlink. Work
was performed in `/tmp/rollupx-l2-fees-e2e.8oFEZzNX` and dashboard work in
`/tmp/eez-ui-l2-fees.Qm8H2JyN`. Neither the operator working tree nor its protocol
pin was replaced. The runtime revision above is the tested and deployed source;
the later commit adding this record changes documentation only.

## Findings

The old bridge quote priced 1,035,395 L1 gas from admission reserves. Actual
posting receipt gas for the historical transaction was 182,945. Including its
physical blob charge, the collected tip was about 4.4576 times actual L1 cost,
or 3.7147 times the intended cost plus 20%. This was excessive pricing, not
the intended risk allowance.

The corrected path executes the complete supported outbound posting in
disposable L1 state and prices its receipt gas, including nested effects,
storage transitions, verification execution and gas refunds. Unsigned intents
lack their final block commitments and proof, so private placeholders and one
exact, independently bound proof-call acceptance assumption are used. The
recognized verifier still executes and consumes gas. Unknown code or unsupported
effects fail closed. This assumption never reaches the signer or final signed
posting validation. The response declares
`isolated_native_l2_and_l1_settlement` and `metered_assumed_valid`.

Unsigned quotes, signed ingress and cumulative multi-user admission use the
same measured tariff. Admission reserves and transaction gas-limit headroom
remain available for execution capacity, but are no longer billed as gas used.
For this trial, conversion remains 1:1, risk is 2,000 bps, and fixed service and
profit charges remain zero. Thus the required contribution is the measured
execution and blob estimate plus its rounded-up 20% allowance. Ordinary L2 gas
is separate; settlement of ordinary L2 traffic remains subsidized as authorized.

| Comparison | Before | After |
| --- | ---: | ---: |
| Public quote for the same repeated bridge intent: L1 gas | 1,035,395 | 182,957 |
| Public quote: settlement contribution, xDAI | 0.001399760408697318 | 0.000376834801536839 |
| Quote reduction | — | 73.0786% |

This is a before/after unsigned quote at different chain heads, not replay of
the historical transaction at its original state. Sender, nonce, destination,
value, calldata and gas limit were unchanged; L1 execution and blob prices were
also unchanged. No transaction from the user's wallet was signed or submitted.

| Receipt calibration | Quoted L1 gas | Actual L1 gas | Collected tip / actual execution + blobs |
| --- | ---: | ---: | ---: |
| Kurtosis first bridge withdrawal, type 2 | 492,835 | 492,835 | 1.2000000 |
| Kurtosis repeated bridge withdrawal, legacy | 182,957 | 182,957 | 1.2001696 |
| Live Chiado nested-call canary, legacy | 173,707 | 173,683 | 1.2000945 |

The live canary cost 304,755,001,215,781 L1 base units including blobs and
collected 365,734,801,552,714 L2 base units in tips. Target state, completed
nested calls, beneficiary, receipt and canonical consumption were checked.
The deliberately underpaid variant was rejected before queue ownership or
execution. Amounts and transaction hashes are in [results.json](results.json).

Standalone quotes still reserve a complete posting for each independent user.
The two-user Kurtosis case settled both users in one native block and collected
1,149,237,708,879,100 base units against a pooled cost-plus-20% requirement of
1,004,491,043,635,712. Shared batching can therefore produce additional surplus.
This release does not allocate shared savings or add automatic refunds.

## Evidence inventory

[results.json](results.json) contains sanitized source/image identities, binary
hashes, receipt comparisons and hashes of selected supporting artifacts. The
local evidence root is `artifacts/l2-fee-measured-20261015/` in the operator
checkout. Raw logs, datadirs and full network dumps are **local only**, ignored
by Git and unavailable to another checkout; no external retention guarantee is
made. Operator secrets and private host configuration are excluded from this
record. The original failed runs and their logs/datadirs were retained.

| Evidence | Contents | Availability |
| --- | --- | --- |
| `results.json` | Exact sources, final image IDs, executable hashes, fees, bounded results and artifact hashes | Checked in |
| `baseline-quote.json`, `deployed-bridge-quote.json` | Before/after unsigned quotes | Local only; hashes in results |
| `fee-quotes-e2e-prepared-executables.log`, `settlement-cost-e2e.log` | Native process results; source-matched frozen executables | Local only |
| `kurtosis-bridge-fees.json`, `kurtosis-mixed-fees.json`, browser evidence | Candidate image receipts and semantic assertions | Local only |
| `kurtosis-blob-*.verified.json`, `independent-replay.json` | Five KZG/semantic checks and canonical L1-only follower replay | Local only |
| `live-canary.json`, `deployment-public-checks.json`, `config-preservation.json` | Canonical live results, public routes, installed assets and preserved configuration | Local only |
| `data/l2-fee-measured-upgrade-20261015.qcifc1tm/` | Private handover scripts, before/after service/configuration snapshots and candidate datadirs | Local only; do not publish wholesale |

## Executed checks

Commands ran from the task checkout unless noted. Cargo used `+1.95.0` and
`--locked`; local native builds used the operator checkout's shared target
directory, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, and LLVM on `PATH`.

| Check | Evidence type | Result |
| --- | --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | Unit | 345 passed; one manual profiling test intentionally ignored |
| `cargo +1.95.0 test --locked -p eez-node --lib` | Unit/local sockets | 85 passed on the network-enabled rerun; first restricted run had nine bind failures |
| `node sdk/l2-fees.test.mjs` and dashboard fee wrapper tests | SDK/unit | 12 SDK and two wrapper cases passed |
| `cargo +1.95.0 fmt --all --check`; workspace all-target Clippy with `-D warnings` | Static | Passed after the retained first Clippy findings were fixed |
| Integration `forge build --root contracts`; dashboard production build | Build | Passed |
| Native `node_e2e`, `fee_quotes:: --test-threads=1` | Process | Six passed using frozen exact-source binaries; see initial failure below |
| Native `node_e2e`, `settlement_policy_cost:: --test-threads=1` | Process | Two passed, Ethereum and Gnosis |
| Browser fee scenarios against the final Docker dashboard | Mock wallet/transport | 16 passed: type 2, legacy, quantity normalization, native/ERC20 and failure/cap cases |
| Fresh Kurtosis deployment verification | Candidate Docker/process | Correct protocol, deployment, genesis and actual image identities |
| Mixed two-user nested workload | Candidate Docker/live private network | Underpayment rejected; both paid users settled in one native block; states, beneficiary, receipts and cost coverage checked |
| `node testing/kurtosis/scripts/verify-l2-bridge-fees.mjs` | Candidate Docker/live private network | Real inbound escrow funding followed by first type-2 and repeated legacy withdrawals; exact principal and debit reconciliation; gas/tariff upper and lower bounds |
| Browser type-2 and legacy submission to Kurtosis | Real private-network transactions through fixture wallet adapters | One paid transaction each; canonical settlement and target state verified |
| `verify-semantic-settlement-blob.sh` and fresh `verify_blob_evidence` | Independent KZG/semantics | Six source transactions in five postings verified |
| `kurtosis_policy_replay:: --ignored` with the follower extracted from the final node image | Independent replay | 84 derived blocks and six receipts checked from canonical L1 only; passed |
| Chiado legacy nested canary | Live | One paid transaction, one rejected underpayment; complete settlement fee and target assertions passed |
| Public routes, discovery at `latest`/`pending`/`0x1`, fresh SDK quote, installed UI asset hash | Live/read-only | Passed; all four RPC/front routes restored; SDK check signed and broadcast nothing |
| Operator environment, bindings, genesis/profile and gateway configuration | Live/read-only | Preserved; only intended Compose image/datadir overlays added |

The composer/node unit runs followed the implementation commit `94569105`.
Final `246514ed` adds the bridge regression and behavior-preserving Clippy
cleanup; native process, final Clippy, Docker, Kurtosis and live evidence apply
to that final revision. Frozen native and shipping Docker binaries have separate
hashes recorded in results; they are not represented as identical bytes.

The native process harness used `EEZ_TEST_PREBUILT=1`,
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` only after building,
freezing and hashing this revision's executables. This avoids spending minutes
building a signer after a fixture genesis timestamp has already been chosen.
It did not substitute an earlier checkout's binaries. Kurtosis used its own
explicit, unused `KURTOSIS_ENCLAVE=eez-l2-fee-measured-20261015`; logs were dumped
and that owned enclave was stopped after validation. Other enclaves were retained.

### Retained failures and procedural corrections

- The first native build was killed during concurrent compilation and Docker
  work under memory pressure; no tests ran. Builds were subsequently serialized.
- The first complete native fee run had five passes and one failure:
  `enforced_nested_quote_rejects_underpayment_and_replays`. A signer compilation
  lasting 4m51s aged the already-created fixture past the 256-block quote work
  limit. The logged error was `unsettled history exceeds 256 blocks`. The later
  exact-binary six-case pass is a rerun, not evidence that the first run passed.
  A further rebuild attempt was interrupted before tests. This fixture-timing
  limitation was not changed in this fee correction.
- Restricted node tests could not bind nine sockets; network-enabled execution
  passed all 85. The original errors remain in `unit-tests.log`.
- Initial Clippy reported two cloned-slice suggestions and a deprecated gas
  method. Those were corrected in `246514ed`; workspace Clippy then passed.
- Standalone dashboard setup initially lacked upstream variables, then supplied
  upstreams without URL schemes. No browser cases ran in those setups. The
  configured final image passed all 16 cases.
- The bridge exercise first stopped before transactions because demo bridge
  addresses were missing. Loading `eez-ui-demo/demo.env` resolved it; the guide
  now identifies that artifact.
- The live readiness wrapper initially stopped on the normal `starting_up`
  response after installing the candidate. Maintenance remained active. It was
  resumed with candidate identity and wallet-lock checks, without another
  restart/copy, then passed the canary and restored public routing.
- A read-only postdeployment asset probe assumed `/assets/`; the configured
  `/dashboard/assets/` probe subsequently matched the deployed image exactly.

## Operational changes

The user explicitly authorized fixing, testing and deploying this correction.
The existing deployment's protocol, genesis, contract bindings, keys, fee policy,
ports and unrelated services were preserved. No live contract deployment or
genesis regeneration occurred. Local development images were built and tested;
this was not published CI promotion.

The dashboard was upgraded first to accept both quote bases. Public transaction
and WebSocket routes were put into maintenance, and accepted queue counts were
all zero before stopping the old node and signer. Both exited cleanly. Their
complete stopped databases were retained; independent copies were used by the
new runtime. The matching node/signer image IDs were pinned in a local Compose
overlay. Existing shared-wallet locks were held for the canary, using the owned
workload account, zero transferred value and a 0.005 xDAI maximum reservation
cap; its actual quoted reservation was checked against the available balance.

After canonical canary verification, static discovery, L1 synchronization and
L2 progress checks, the original gateway configuration was restored. Public
verification at 10:54 UTC found the tested node, signer and UI images, zero
container restarts, healthy signer/UI, an empty accepted queue and the exact
tested dashboard JavaScript. The fresh public SDK quote also passed validation.

Rollback was prepared, not exercised. Stop candidate writers before restoring
the prior matched images/configuration and retained original databases; do not
open candidate-modified state with old binaries. Reconcile new canonical L1 work
before rollback, and requote clients against the restored tariff. Already paid
tips are not reversed. Queue persistence/recovery guarantees are unchanged;
an empty queue at this handover does not prove durable queue recovery.

## Limits and remaining work

There is no unmet acceptance criterion for this bounded correction/deployment.
Quotes remain advisory: later prices/state, proof/calldata byte differences and
multi-user sharing can change actual costs. Final signed settlement validation
remains authoritative. This does not qualify production conversion/service
prices, guarantee zero operator loss, implement L1-origin collection, or rebate
historical overpayment.

The full CI release/Kurtosis matrix, full protocol suite, installed MetaMask or
Rabby extension tests, full historical live-network replay, and a live rollback
rehearsal were not run. Protocol contracts/genesis were unchanged. Independent
replay ran for native scenarios and the exact Docker follower on Kurtosis; the
live deployment was checked through bounded canonical semantic assertions.
Fixture wallet adapters test dashboard behavior but do not prove all extension
routing behavior. These limits do not expand the reported test coverage.
