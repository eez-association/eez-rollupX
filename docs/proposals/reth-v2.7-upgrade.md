# Reth v2.7.0 on the native transaction implementation

Status: accepted scope requested by the repository owner on 2026-09-29;
implementation and local validation in progress. Public-network deployment is
outside this scope; isolated local devnets are part of validation.

## Scope and baseline

Update PR #44 from `3f95f1848576cb19f9bf6e7ae4151c2e5eb3757c`, on
`backport/native-system-txs-issue-35`, to unmodified Reth v2.7.0 commit
`3d592ece6de8c4559987416a544fc215fd6d6921`. The dedicated worktree is
`artifacts/native-system-backport/rollupX`. Its base includes merged PR #42
and the [PR #41 migration](reth-v2.6-validation.md).

Keep protocol pin `9d13218a355e7d543ca8a9927eea2962600b716d`, native
transaction type/nonce/gas rules, checked reconstruction, independent Stateless
proof validation, and persistence configuration. Align every Reth consumer. Use
upstream Gnosis #138 unchanged, as requested by the owner, and pin the published
Stateless compatibility branch to replace the vendored dependency copies. Update
only dependencies required by this release; retain Rust 1.95.0.
Fix the relocated signer fixture paths responsible for PR #44's release-unit
CI failure, without changing production validation.

## Acceptance

| ID | Requirement | Check |
| --- | --- | --- |
| R1 | One exact Reth release revision across runtime and Stateless | Locked graphs, shipping builds, upstream revision comparison |
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
The subsequent startup checks must retain successful registration and canonical
settlement evidence even after it leaves Kurtosis's default 200-line log tail.
Keep their existing success requirements and bounded waits.

Upstream Gnosis #138 uses Reth's default JIT feature selection. Install LLVM 22,
Polly, Clang 22 and `m4` consistently in Docker producers and source-build CI.
The custom L2 execution remains interpreted. Pin `revmc` in Cargo.lock to the
revision already tested by Reth v2.7.0 and Gnosis #138.

The complete cross-chain lane exposed an authenticated-seed race: reconciliation
could inspect the head before previously dispatched sequencing finished, and
sequencing could capture a seed before acquiring the reconciliation lock. Drain
issued engine work with an actor barrier; capture attributes and enqueue under
the lock. Prebuilt blocks must still match both parent and authenticated context.
Controlled synchronization regressions must fail before the fix; rerun the real
thirty-request admission scenario and the full cross-chain lane afterward.

Lookup-only CI evidence still expected the removed operations block-header
format. Bind V0 counts to calldata endpoint claims and canonical L2 headers,
retaining trace, table, receipt, KZG and absence checks. Keep supplied settlement
claims separate from execution evidence. Decode native transactions and receipts
when reconstructing prefix headers. The independent follower check consumes V0
and compares ordinary bytes, metadata, ancestry and terminal commitments.

The witness-based CI prefix helper must retain the deployed genesis, not only
RPC chain configuration: L2 block positions depend on its timestamp and L1
reference. Use the existing profile validator and bind the supplied genesis to
the node's canonical genesis hash and configuration. Reject missing or
inconsistent inputs before replay; do not supply synthetic genesis defaults.
