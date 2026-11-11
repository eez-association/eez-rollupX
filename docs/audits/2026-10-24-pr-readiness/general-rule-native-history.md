# General settlement threshold and native history transactions

The frozen pre-R03 process suite failed
`settlement_policy::general_block_rule_consolidates_empty_history_before_the_pure_interval`
at `settlement_policy.rs:2596`, after its 180-second wait. The preceding fee-RPC
outage/retry test passed. The failure is separate from the earlier Docker port
collision. [Evidence and executable identities](general-rule-native-history.json)
retain the first failure and the relevant canonical events.

The predicate requires an applied cost report with at least 30 settled blocks
and **zero included L2 transactions**. Instead, the node successfully settled
endpoints 30, 60, 90, 120, 150 and 180. Every report covered 30 blocks. The first
included 12 transactions; subsequent reports included six, while the policy
reported zero pending pure L2 transactions. Each general-rule decision was due
at a 30-second settlement age, below the configured 900-second pure interval.
Both node and signer were alive at the timeout. The negotiated and configured
block limit was 65,536; the separate live network's 24-block cap does not apply.

`composer/costs.rs:483–495` collects **all** transaction hashes from canonical
block bodies, and `coverage_from_hashes` counts them, including only additions
at partial endpoints. The normalization at lines 200–202 therefore remains
defined when native history transactions are present. This matches the
[cost-reporting contract](../../proposals/configurable-settlement-policy.md#631-posting-fees-and-cost-on-the-active-network).
By contrast, the pure-activity index explicitly excludes native transaction
type `0x45` (`composer/policy.rs:808–818`). A range without application activity
is no longer a range with zero transactions: the
[history contract](../../proposals/l1-origin-and-offline-proving.md#schedule-and-public-system-transaction)
requires a transaction at every scheduled update, including repeat imports.

The correction is confined to this test. It uses the existing
`setup_native_history` fixture without application deployments, and requires a
general-rule decision with zero pure activity. The applied L1 receipt and its
`L2ExecutionPerformed` event identify the exact posted transition from genesis;
the test follows that endpoint's parent hashes and recomputes each transaction
root. Every body is empty or contains its one scheduled history import, decoded
exactly with the existing native envelope and bounded import decoder. The test
checks its destination, chain, gas, cutoff and position-bound source hash.
It compares the metric count to those bodies and normalized costs to the actual
L1 receipt. The 30-block condition, successful canonical application, nonzero
blob charge and settlement before the pure interval remain. Production metrics,
triggers, capacity limits and timeouts are unchanged.

The initial review did not compile or rerun anything. The source and events establish
the impossible zero predicate and actual settlement progress. The specific
six-history-transactions interpretation follows the enforced schedule and
activity classification. The corrected regression now checks bodies directly.
Bulk logs and datadirs are local-only; the JSON records the digest of an
immutable failure excerpt captured while the original suite was still running.

## Corrected regression

The test-only build passed with Rust 1.95.0, `--locked --offline`, LLVM 22, one
Cargo job and a private `/tmp`; no production executable changed. Its first
execution passed **1/1 in 37.23 seconds**, with 178 unrelated tests filtered out.
Formatting, edited links/hashes and `git diff --check` also pass. The JSON records
commands and executable identities; the test is retained under the local
`artifacts/pr-readiness/general-rule-correction/` directory.

The process used the same Composer and signer bytes as the original failure,
with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1` enforced by the existing seccomp launcher.
It settled 30 blocks containing six validated history imports and zero pure
application transactions. The general trigger fired at 30,000 ms. The receipt
charged 131,072 blob gas and 65,354,851,794,782 wei in total; the tested integer
normalizations were 10,892,475,299,130 wei per included transaction and
2,178,495,059,826 wei per block. These values were checked against the canonical
bodies and receipt, rather than installed as expected fixture constants.

This is a corrected-harness comparison, not a retry-only pass of unchanged
code, and it does not erase the original full-suite failure. It does not validate
new R03 production bytes or independent follower replay; the follower path was
verified/configured but this case starts only the Composer and signer. Successful
fixture datadirs were removed by normal teardown; diagnostic logs remain retained.
