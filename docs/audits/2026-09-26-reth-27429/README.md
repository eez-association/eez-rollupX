# Upstream Reth #27429 candidate

Status: validation in progress for draft [PR #41](https://github.com/eduadiez/rollupX/pull/41).

The user authorized continuing with the upstream fix commit. Reth is pinned to
`444e867155c2a7a4c028e479ca7201b0b6871579`, the merge of
[#27429](https://github.com/paradigmxyz/reth/pull/27429), with no local Reth patch.
Compared with v2.6.0, it contains 78 main-branch commits and excludes only the
release branch's version-bump commit. Its Cargo package version remains 2.5.2;
the SHA identifies the tested source. This is not a claim of a released v2.6.1.

The EEZ baseline is PR head `69af59f1e6dd03829f84fe7f98768e6c10b09ff0`, based on
main `3346d966f3c8611be790db484abc63b3b259d367`. Protocol gitlink
`9d13218a355e7d543ca8a9927eea2962600b716d` is unchanged. Operator work, services,
wallets and databases remain untouched. Fresh networks remain the scope.

## Adaptations

Reth core crates move to 0.8.1 and Alloy to 2.5.0; REVM remains 43.0.3 in the
root lockfile. Upstream separates EVM reads from full state providers. EEZ and
Gnosis use its transparent EVM adapter while retaining full providers for roots
and proofs. A two-method snapshot interface erases Ethereum/Gnosis primitive
types for shared simulations; the Gnosis adapter forwards the new executed-block
state method with the original Gnosis primitives. No chain rules, signer gates,
protocol commitments or production persistence defaults change.

The partial-consumption crash fixture first flushes an ordinary L2 prefix, then
executes the original partial settlement and held retry. After SIGKILL it reads
both block and state frontiers from MDBX before reopening and requires them to
lie strictly between genesis and the settlement height. The L1 persistence barrier
and all existing nonce, receipt, accounting and resubmission assertions remain.

## Evidence

Local-only commands/logs and binaries are retained in `artifacts/reth27429/` in
the isolated PR checkout. Earlier v2.6.0 evidence is retained in the
[previous audit](../2026-09-26-reth-v2.6-pr/README.md); it does not validate this
candidate. The relationship between #27429 and the earlier crash failure remains
unproven pending the controlled comparison.

## Remaining checks

Shipping builds, exact-candidate process/Kurtosis/replay checks, controlled
crash comparison, final dependency/lint checks and CI acceptance remain pending.
