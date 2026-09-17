# Derivable ChainOperation Kurtosis validation

| Field | Value |
| --- | --- |
| Date and status | 2026-09-17 UTC; local disposable-network validation passed |
| Scope | Mandatory-policy deployment, inbound, outbound, mixed, mixed-pure, protocol semantic scenarios, physical blob publication, canonical L1/L2 commitments, clean node restart, and partial consumption/recovery |
| Source | `eez-rollup0` branch `feat/derivable-chain-operations`, base and freshly fetched `origin/main` `5469c88860408daf776ee812156dd280c1a9e262`, plus the working-tree implementation enumerated by the implementation source manifest |
| Protocol | Gitlink `3c0cdf4955941a5c23639346932b55b33babebbd`; no protocol working-tree changes |
| Executables | Locally built `eez-node:ci-local`, `eez-proof-signer:ci-local`, and `eez-deploy:ci-local`; partial-consumption builder `sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308` |
| Environment | Isolated Kurtosis enclaves on the local Docker engine; Rust 1.94.0; mandatory interval settlement policy; all owned enclaves removed after collection |
| Related records | [Implementation contract](../../proposals/derivable-chain-operations.md), [coverage ledger](../../proposals/derivable-chain-operations-coverage.md), [final validation](../../research/2026-09-16-blob-efficiency/implementation/final-validation.md), and [focused signer soundness review](../../research/2026-09-16-blob-efficiency/implementation/soundness-review.md) |

## Findings

The fresh full run passed all four traffic modes, state chaining, canonical-root
convergence, and all 28 protocol scenarios. It produced 25 signed windows and
25 remote attestations with zero L1/L2 root divergence. The independent
collector authenticated the semantic blob evidence for every scenario; the two
lookup-only cases correctly required no destination effect.

The first full run executed all 28 upstream scenarios successfully but its
independent collector rejected `revertCounter` and `topLevelStaticCounter`.
The collector assumed every block record carried a hash. Derivable intermediate
records intentionally carry replay inputs instead; their canonical result is
committed by the terminal block hash. The verifier now requires a sequential
window, valid derived-record fields, exactly one final terminal record, and
canonical equality for every explicit full or terminal hash. Negative tests
cover gaps, a misplaced or missing terminal, and unknown representations. A
fresh enclave then passed 28 of 28 with complete causal and semantic evidence.

The pure L2 smoke published the deployment and both calls in a Beacon blob,
verified the sidecar/KZG-derived versioned hash, read value 42 at safe block 198,
and matched the L1 registry root to that block hash. After a clean node restart,
RPC and both correlation directions recovered in 10 seconds and the same safe
commitment remained canonical.

The partial-consumption canary forced three outcomes in one attempt:
`Committed`, `SourceNonceSpent`, and `SourceReverted`, all at attempt zero. The
safe head subsequently advanced to block 714 after a healthy transaction. Live
execution exposed three harness assumptions, which were fixed and regression
tested: a mandatory-policy Sync boundary does not require an empty postBatch,
the preliminary dispatched hash can change during repricing, and replacement
submission must reach the pinned builder directly rather than rely on devp2p
propagation.

The final repository tooling sweep found that the standalone
`derivable_operations` target had not been classified by the native artifact
runner. The target is now archived and executed in the cross-chain lane; the
inventory regression requires every auto-discovered integration target to have
exactly one execution suite.

These results validate a disposable local deployment and the tested failure
paths. They are not a public or production deployment, a maximum K=6 exercise,
or a largest-window follower crash-injection result.

## Evidence inventory

The compact result is checked in as [live-results.json](live-results.json).
The full artifacts below are ignored local files and are available only in this
worktree unless copied to durable storage.

| ID | Contents / provenance | Location and digest | Availability / retention |
| --- | --- | --- | --- |
| E1 | Initial full run; all upstream executions passed, collector exposed stale hash assumption | `artifacts/kurtosis-derivable-0917/result.json`, `6f65df79d595f7cdd50ecd624e1b1c79ed85715538ee516a7b1d531c1c30db1e`; protocol summary `44a5f76a767d074f86ddac41ae8e4cbfe96a13326cf71d3c745ae2f937d40242` | Local only |
| E2 | Fresh clean full run after verifier correction | `artifacts/kurtosis-derivable-0917b/result.json`, `c78a662ccf2b5c77c0f24bff69a1eb35265ed38beb1161025223e09282b98282`; protocol summary `8832c9f14696c4b8b6ce48b029117ec96a014fa5399445800282d8b8eacc51d1` | Local only |
| E3 | Pure L2 physical blob and canonical commitment smoke | `artifacts/kurtosis-derivable-extra-0917/blob/20260917T200817Z/summary.json`, `12e42b48a372b6d1ed3146560ed0c53237146f65a38fc5343ce7afe86a86fdc3` | Local only |
| E4 | Clean node restart and correlation recovery | `artifacts/kurtosis-derivable-extra-0917/restart/20260917T201400Z/summary.json`, `872a18954913fb2e3e96aa18bdde32e13c36558ee58eade78554fdc0fe2de7e3` | Local only |
| E5 | Partial-consumption/replacement canary | `artifacts/kurtosis-derivable-partial-0917/partial-consumption.json`, `04755b7d0a13fbc45e1911635d926a3acc60136a9868b21997d369d4a93d1311` | Local only |

## Reproduction and executed checks

Commands ran from the feature worktree. Each command used a unique owned
enclave. The runners removed the first two enclaves automatically; the extra
and partial-consumption enclaves were removed after evidence collection.

| Requirement / question | Production path or check | Evidence type | Result and evidence |
| --- | --- | --- | --- |
| Deployment uses the mandatory settlement policy | `KURTOSIS_ENCLAVE=eez-derivable-0917b EEZ_CI_RESULT_DIR=$PWD/artifacts/kurtosis-derivable-0917b EEZ_PROTOCOL_MAX_PARALLEL=all bash testing/kurtosis/run-ci.sh` | Live disposable network | Pass; genesis, exact local images, signer readiness, and policy wiring, E2 |
| Inbound, outbound, mixed and mixed-pure behavior | Same full runner | Live protocol behavior | Pass; four modes, 25 signed windows/remote attestations, state chaining and safe-head convergence, E2 |
| Complete protocol suite and independent semantic checks | Same full runner | Live upstream execution plus independent collection | 28 selected, 28 passed, zero failed/skipped; accounting, causal coverage, and semantic blob coverage complete, E2 |
| Physical pure-L2 blob and on-chain commitment | `EEZ_VERIFY_BLOBSCAN=false ... bash testing/kurtosis/scripts/verify-blob-da.sh` | Beacon sidecar/KZG/canonical RPC | Pass; three mapped L2 transactions, value 42, safe hash equals L1 registry root, E3 |
| Node restart recovery | `... bash testing/kurtosis/scripts/verify-node-restart-recovery.sh` | Live process restart | Pass; outage observed, RPC and correlation recovery in 10 seconds, pre/post correlation evidence identical, E4 |
| Partial consumption and later liveness | `python3 testing/kurtosis/scripts/verify-partial-consumption.py ...` | Live PBS replacement and settlement | Pass; all three expected dispositions and subsequent safe progress, E5 |
| Verifier and deployment-profile regressions | `python3 -m unittest discover -s scripts/ci/tests -v` plus syntax/YAML checks | Unit/static | Pass; final counts are recorded in final validation |

Blobscan was not present in the extra smoke profile, so that optional explorer
cross-check was disabled. Physical Beacon sidecar and KZG/versioned-hash checks
remained enabled there, and the full protocol run independently authenticated
the semantic contents of all 28 scenario blobs.

## Operational changes

No public or production resource was changed. The tests created only uniquely
named local Kurtosis enclaves. All owned enclaves were removed. Existing shared
or stopped enclaves and unrelated Docker resources were left untouched.

## Open work and superseding evidence

| Unmet criterion / uncertainty | Impact | Next concrete check or action |
| --- | --- | --- |
| Release K=6 live profile | Local runs do not prove the deployment profile's K=6 maximum geometry | Run the retained maximum-capacity workload with the exact release configuration |
| Largest-window follower crash injection | A clean restart proves persistence/recovery for the observed safe window, not interruption at early/middle/late points of the maximum window | Inject crashes at those three replay points and compare final hashes and recovery time |
| Release pruning/state-history/resource envelope | Disposable defaults do not establish long-running retention or capacity | Run the release profile with intended pruning, disk, memory, and witness limits and retain portable artifacts |
