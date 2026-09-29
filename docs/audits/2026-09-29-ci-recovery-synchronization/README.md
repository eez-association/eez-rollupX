# PR #42 recovery test synchronization

| Field | Value |
| --- | --- |
| Date / scope | 2026-09-29 UTC; fix the formatting and ordinary-burst recovery checks blocking PR #42 |
| Baseline PR head / base | `0b7beed5ca48157d720d6685435ba2121c1e9c1e` / `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`; unchanged |
| Runtime | Frozen local release binaries from `ba4cbabca056302875715e9e060e7b803c5c282c`, used for both baseline and corrected harnesses |
| Runtime identity | [Prior release/image hashes](../2026-09-28-consecutive-skips/evidence/candidate-identity.json) |
| Failed CI | [Run 36466529369](https://github.com/eduadiez/rollupX/actions/runs/36466529369) |

## Failure mechanism and correction

Both CI ordinary-burst cases failed the immediate assertion that RPC recovery
had rebuilt an empty historical Sync boundary. The test restored reader RPC,
waited for transaction receipts and an L2 height, then inspected the recovery
log once. With four cached replacement opportunities, the receipts and height
can already exist before the reader sees recovery. They do not synchronize the
test with rebuilding the deliberately missing historical interval.

The corrected test waits, within its existing timeout, for an Empty Sync event
after the recovery cursor and within the observed outage gap, then for its
canonical block to exist. It still requires that exact timestamp and an empty
block. Boundary inventory is read afterward. It retains per-interval blob
capacity checks, the no-premature-posting assertion, ordinary receipt/nonce
checks, independent follower replay and a cross-chain source after the burst.
No timeout was increased or assertion removed. Runtime scheduling is unchanged.

`dynamic_admission.rs` also had a line-wrap difference rejected by Rust 1.95
rustfmt; the local baseline format check reproduced it and formatting fixes it.

## Validation

Diagnostics are local-only under `artifacts/pr42-ci-fix-20260929/`, excluded from
Git. Original CI logs, test output, retained node logs/datadirs, harness hashes
and runtime hashes are preserved there. The frozen release binaries are local
development builds, not a substitute claim of CI artifact provenance.

- Built baseline and corrected `settlement_policy` harnesses using
  `cargo +1.95.0 test --locked -p eez-node-e2e --test settlement_policy --no-run`.
- `forge build --root contracts` passed with Foundry 1.7.1.
- Ran both baseline cases serially against the verified release executables:
  **0 passed, 2 failed, 0 ignored**. Ethereum reproduced the CI recovery assertion.
  Gnosis failed earlier at the unchanged fixture slot-alignment assertion
  (timestamp difference modulo slot length was 1); this additional failure is
  retained, not counted as reproduction of the later recovery assertion. CI's
  Gnosis run did reach that later assertion.
- Corrected process results: **2 passed, 0 failed, 0 ignored**, 29 filtered out,
  in 298.15 seconds. Both profiles exercised the recovery assertion, multi-blob
  settlement, independent follower receipt/state replay and subsequent inbound
  progress. Gnosis also passed the unchanged slot-alignment guard in this run;
  that does not erase or fix the separate baseline alignment failure above.
- `cargo +1.95.0 fmt --all --check`, workspace/all-target Clippy with `--locked`
  and `-D warnings`, and `git diff --check` passed after the initial correction.
- New-head CI: pending push and validation. Previous-head successful runtime,
  Cross-Chain and Kurtosis lanes do not validate a newer harness.

See [result counts and executable/artifact identities](evidence/results.json).

Process tests use independent local ports/data and public fixture wallets. No
Chiado deployment, restart, operator configuration or live spam was performed.
The earlier local comparison audit and unrelated operator edits remain separate.
