# Protocol PR 54: latest fee-client changes

Status: latest-delta review complete; local checks passed after building their
required ABI artifacts. No new blocker found in the latest commit. Protocol
PR 54 can be merged with the classic-runner follow-up below; RollupX PR 91 still
needs its combined integration gate. No protocol changes, pin updates,
merges, broadcasts or deployments were performed for this review.

## Reviewed identities

- RollupX checkout: `6442b908a521dc64216c88916be2365bc7c8b9f5` on
  `feat/pr54-kurtosis-fees`.
- Current protocol gitlink / comparison baseline:
  `f46af7c9833ac12a54f14d52f4da1f318a0f6525`.
- [Protocol PR 54](https://github.com/eez-association/eez-core-protocol/pull/54)
  head: `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`;
  base: `143d87e62cade98bfc122d612638dcaaf1c85633`.
- The deployed Composer nonce/snapshot fix is `de4209a5e956b432db7a7d37517d9d44d68dfaa4`;
  see its [qualification and deployment record](../2026-10-19-composer-nonce-snapshot-fix/README.md).
- Local protocol review used a detached, unmodified worktree at the exact PR
  head, initialized submodules, Foundry 1.7.1 / Solidity 0.8.34, Node 26.7.0
  and Python 3.14.4. Decoder dependencies were installed from its npm lockfile.

## Findings

The latest commit fixes the temporary-error classification that stopped the
[earlier fee-enabled integration run](../2026-10-19-kurtosis-fees/README.md).
The same `-32004` / `quote_unavailable` queued-predecessor response is terminal at
`f46af7c` and retryable at `abcf9d8`, in both full JSON-RPC and Cast formats.
Replaying 11 retained error cases passed, including the captured mixed/pure
wave-three error. Disabled quotes and unknown errors remain terminal.

Temporary admission refusals retry identical signed bytes under the original
deadline, lock and quote expiry. Only an explicit underfunded-tariff refusal
requests a fresh quote and signature, preserving the prepared nonce and payload.
Tests cover these distinctions and the corrected deterministic timeout clock.

The nine-file delta also adds separate L1/L2 gas summaries to the decoder.
It sums disjoint physical subtrees, labels incomplete attribution and distinguishes
flow gas from whole settlement-transaction gas. It reports gas units, not a fee
or profit calculation. There are no contract, ABI, dependency or compiler changes
relative to the currently pinned protocol.

The earlier classic-preparation setup/reorg guard is still absent from
`script/e2e/lib/E2EBase.sh:713`. A controlled test at the new head retains setup
nonce 1, returns nonce 0 from both mocked RPCs, and observes signing at nonce 0
instead of refusing a nonce below 2. No transaction was broadcast. This is a
pre-existing runner robustness gap, not a regression in this delta or evidence
that it caused the historical Composer snapshot mismatch. The default staged
plan path derives trigger nonces from its deployment plan instead. Track this
classic-runner guard separately.

## Checks and coverage

[GitHub run 39248296533](https://github.com/eez-association/eez-core-protocol/actions/runs/39248296533)
passed both jobs at the reviewed head:

- Contract tests: 780 passed, three skipped. The skipped tests are
  `testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`, and
  `testForkRejectsWrongSigningKey`.
- Local Anvil: 34 selected scenarios passed, zero failed or skipped within that
  selection. The 22 new call-pattern scenarios retain `E2E_EXCLUDE_FROM_ALL` and
  are not covered by that default run. The PR description's claim that they
  participate in default/all selection is stale.
- The workflow does not execute the Python runner or Node decoder suites.

Local commands, run from the exact protocol worktree:

```bash
git submodule update --init --recursive
npm ci --prefix trace-decoder --ignore-scripts
forge build
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s script/e2e/tests -v
npm --prefix trace-decoder test
bash -n script/e2e/lib/network-fees.sh
git diff --check f46af7c9833ac12a54f14d52f4da1f318a0f6525 HEAD
```

The first runner/decoder attempts preceded the fresh contract build: 178 of 187
runner tests passed, including all 57 fee-client tests; nine evidence tests failed
because `out/EEZ.sol/EEZ.json` was missing. The decoder passed 226 tests, with its
ABI-dependent test file failing to load for the same reason. These setup failures
are retained. After `forge build` passed, the same nine failing runner tests all
passed (36.543 seconds), and the complete decoder suite passed 240 tests with no
skips. Thus all 187 selected runner tests have passing results; there was no
second full 187-test invocation. The first failures were prerequisite errors,
not unexplained retry-only passes. Shell syntax and diff checks also passed.

The runner rerun used `PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=script/e2e/tests
python3 -m unittest -v` with the exact nine test IDs retained in
[validation.json](validation.json). These are mocked runner/decoder tests;
they do not execute a live Composer or independently replay a settled chain.

## Remaining integration gate

RollupX PR 91 still points to `e5e1d97ff49e69821ba403a9ae723ab4afa2db21`, is draft,
and has a failed Kurtosis check. The separately qualified Composer fix and this
new protocol fee client have not yet passed the complete fee-enabled Kurtosis
lane together. A green protocol workflow does not close that RollupX merge gate.

Raw review logs and controlled-test results are local-only at
`/tmp/rollupx-pr54-fees-20261019/artifacts/pr54-latest-review-20261019/`;
they are not available in a fresh clone. [validation.json](validation.json)
retains sanitized results, reproduction inputs and log digests. The source
worktree remained unmodified and the PR head was rechecked after testing.
