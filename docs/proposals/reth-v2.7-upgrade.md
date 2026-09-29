# Reth v2.7.0 on the native transaction implementation

Status: accepted scope requested by the repository owner on 2026-09-29;
implementation and local validation in progress. No deployment is authorized.

## Scope and baseline

Update PR #44 from `3f95f1848576cb19f9bf6e7ae4151c2e5eb3757c`, on
`backport/native-system-txs-issue-35`, to unmodified Reth v2.7.0 commit
`3d592ece6de8c4559987416a544fc215fd6d6921`. The dedicated worktree is
`artifacts/native-system-backport/rollupX`. Its base includes merged PR #42
and the [PR #41 migration](reth-v2.6-validation.md).

Keep protocol pin `9d13218a355e7d543ca8a9927eea2962600b716d`, native
transaction type/nonce/gas rules, checked reconstruction, independent Stateless
proof validation, and persistence configuration. Align every vendored Reth
consumer. Update only dependencies required by this release; retain Rust 1.95.0.
Fix the relocated signer fixture paths responsible for PR #44's release-unit
CI failure, without changing production validation.

## Acceptance

| ID | Requirement | Check |
| --- | --- | --- |
| R1 | One exact Reth release revision across runtime and Stateless | Locked graphs, shipping builds, vendor patch checks |
| R2 | Native transaction identity and independent validation unchanged | Native, protocol vector, signer, Stateless checkpoint and replay tests |
| R3 | Release archives execute away from their compilation checkout | Reproduce both failing fixture helpers before the fix; run the actual release archive after it |
| R4 | Persistence, reorg, Gnosis and settlement behavior retained | Embedded/cross-chain process suites and local Kurtosis with verified candidate artifacts |
| R5 | Local validation precedes pushing | Formatting, Clippy, dependency/tooling/contract/Anvil checks, release producer and all CI test lanes; record failures and exclusions |

## Compatibility

This remains a fresh-network change under the
[native transaction compatibility contract](native-system-transactions.md).
Do not add legacy decoding or migration paths. The Reth release enables persistent
block-access-list storage; historical database migration and downgrade are not
claimed. Use matching Composer, follower and signer versions with the PR's fresh
genesis/datadirs. Do not reuse candidate-written databases with older binaries.
Restart still loses in-memory unsettled request ownership; clients may need to
resubmit. Public Chiado services and operator data are outside this validation.

Reth v2.7.0's Sepolia fork schedule does not authorize extending the signer's
supported execution profile. Its existing rejection of unsupported forks stays
in place. Genesis fixtures and protocol contracts should remain byte-identical
unless a demonstrated incompatibility requires an explicit follow-up.

## Gaps exposed by local validation

The real process suites exposed an ordinary historical-drain liveness bug in
PR #44's native proof integration. After one preparation overrun, the older
policy estimator selected a future execution height whose parent hash was
unknown. Native proof authorization correctly rejected every repeated attempt.
Use the existing complete-pipeline deadline for ordinary windows, retain known
canonical parents, and remove that incompatible estimator. Preserve all parent,
settlement-provenance, capacity and submission checks. The Ethereum/Gnosis burst
process tests and controlled target/deadline tests are acceptance regressions.

A fresh Kurtosis L1 also lacked a finalized genesis reference at deployment.
Wait for finality before the first broadcast, bounded to 30 minutes for the
mainnet-preset epoch cadence. Do not substitute `latest`. An initial 15-minute
bound proved insufficient locally; preserve that failed run in the audit.
