# Protocol PR 56: live fee quoting and settlement-link verification

Status: tested on 2026-10-16. The original PR completed 33 of 34 enabled
scenarios. A local correction to include the prepared nonce completed the
remaining scenario; the resumed run revalidated all 34. The correction is
committed locally and has not been published to the protocol repository.

| Identity | Revision |
| --- | --- |
| [Protocol PR 56](https://github.com/eez-association/eez-core-protocol/pull/56) base | `58a83b724308c1abb8672949fb877684a473aea0` |
| Tested PR head | `bcd77d8b0d23ed9086f12ac8a0c2e65c6a2c2688` |
| Local correction | `86b4989ce6cbe2133f8f8a8dbd670d7c657ebf93`, branch `codex/pr56-explicit-quote-nonce` |
| Fork checkout | `d69cccf299275276c1873d100c0928228e32a7ec` |
| Deployed runtime source | `aa8d8a5894c69502c67dcae35d5184bc9a79b036` |
| Deployed protocol pin | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |
| Toolchain | Foundry 1.7.1, Solidity 0.8.34, optimizer runs 1, via IR |
| Live network | Chiado L1 chain 10200, existing L2 chain 906969 |

The exact runtime images, results and retained artifact digests are in
[evidence.json](evidence.json). The preceding runtime qualification is recorded
in [posting-price feedback](../2026-10-16-posting-price-feedback/README.md).

## Reproduced integration gap

### Attribution clarification after engineer feedback on 2026-10-16

The runner did not send a duplicate nonce: it omitted the field, and Composer
selected the canonical nonce already owned by the first source. This matches
the then-documented default but is a Composer API limitation for same-account
bursts without explicit nonces. Describing it solely as a protocol-client bug
was too strong. The local client patch remains appropriate when quoting an
already prepared transaction with a known nonce; its successful run establishes
that explicit predecessor replay works, not that automatic selection works.
The [PR 91 follow-up](../../proposals/cross-chain-fee-recovery.md#omitted-l1-nonce-follow-up-from-pr-56-feedback-on-2026-10-16)
adds automatic selection using the same owned predecessor replay and separately
qualifies that behavior. The original measurements and failure below remain
unchanged.

The PR's `network_fee_quote` sends sender, target, calldata and value, but omits
the nonce from its prepared transaction. For the second source of
`counter-multi-tx`, the deployed Composer selected the canonical nonce already
owned by the first pending source and returned `-32602` / `invalid_transaction`:
`source nonce is already pending`. The runner correctly stopped without sending
the second or third transaction. This is an incomplete test, not a successful
three-transaction scenario or a tariff rejection.

A read-only comparison against the same unsent candidate reproduced the error
without a nonce and returned both quote options when `nonce: 0x67` was included.
The Composer used in that trial supports replaying its owned contiguous L1
predecessors when given an explicit source nonce; no backend change or permissive
fee policy was required for the patched runner. Its implementation is in
[compact quote preparation](../../../crates/eez-composer/src/composer/fees/quote/compact.rs)
and [predecessor replay](../../../crates/eez-composer/src/composer/fees/quote.rs).

The [local patch](explicit-nonce.patch) adds the prepared nonce to the Composer
request and node-fallback evidence, documents the requirement, and extends the
real offline-signing regressions. The new L1 assertion failed against the
original implementation for both legacy and type-2 transactions before the fix.
It passed afterwards. Invalid nonce responses remain terminal; the patch adds
no blind retries, nonce changes or wait-for-mining workaround.

The PR contracts, compiler settings affecting bytecode, and dependency gitlinks
match the deployed protocol pin. Differences from that older pin include
scenario/tooling/documentation work inherited through the PR base. No protocol
contract, genesis, runtime image or parent-repository gitlink was changed.

## Executed checks

| Check | Result |
| --- | --- |
| Changed shell syntax / PR diff whitespace | 10 scripts passed; diff check passed |
| PR Python regressions | 166 passed: 126 fee/configuration/preparation/recovery tests and 40 evidence tests |
| Protocol Solidity unit/integration suite | 778 passed, 3 fork tests skipped; explicit repository exclusions below |
| New Solidity link-verification tests | All 6 passed, included above |
| New nonce assertion against original code | Failed for both `0x0` and `0x2`, as expected |
| Corrected fee regressions | 44 passed |
| Corrected recovery regressions | 34 passed |
| Unmodified live `all` | 33 passed, 0 semantic failures, 1 incomplete; exit 1 |
| Corrected live resume | 34 passed, 0 failed, 0 incomplete; exit 0 |
| Final canonical evidence | 36 verified source-link records; empty queue; runtime identities unchanged |

The isolated protocol checkout was
`/tmp/eez-protocol-pr56-jq7juxjt/protocol`. Dependencies were initialized at their
exact recursive pins. Commands, from that checkout:

```bash
git submodule update --init --recursive
FOUNDRY_ISOLATE=false RAYON_NUM_THREADS=2 forge test \
  --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s script/e2e/tests -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s script/e2e/tests -p test_network_fees.py -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s script/e2e/tests -p test_staged_recovery.py -v
```

The full Python discovery command was initially started before the build
completed. Its new link tests require `out/EEZ.sol/EEZ.json`; missing artifacts
caused failures, and that premature invocation was stopped with its log retained.
The completed coverage was then split into the 126 other tests and the 40
`test_network_evidence.py` tests after compilation. This is a corrected build
prerequisite, not a retry-only behavioral pass. The three skipped Solidity tests
were `testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`, and
`testForkRejectsWrongSigningKey`.

## Live procedure and preservation

The trial used the PR's staged runner, fee helper and semantic verifiers. A
local wrapper adapted the existing Chiado preflight to 34 jobs, holding the
shared-wallet locks in the operator, fee-feature and isolated PR checkouts for
the full run. Existing faucet/worker accounts were reused. Preparation/send/
verification concurrency was 4/4/2. `E2E_TX_LINKS=required` prevented compatibility
fallback from qualifying the exact-link path. The effective commands were:

```bash
bash script/e2e/run/network/staged.sh all
bash script/e2e/run/network/staged.sh --resume tmp/e2e-staged-net/20261016-144340
```

These commands record the execution; another live run must use the repository's
wallet-lock/preflight procedure rather than invoking the upstream runner around
that lock. See [the staged Chiado helper](../../../scripts/run-chiado-staged-performance.sh).
Private network profiles and keys are excluded from this record.

An initial invocation stopped before funding/deployment because the copied
profile exceeded the PR's 300-second monitor maximum. Only the private trial
profile was adjusted. The substantive run deployed 153 setup transactions and
prepared all 34 jobs. The unmodified runner sent 34 of the intended 36 sources.
All 34 submitted sources settled; the 33 fully submitted scenarios passed exact
receipt/calldata/table/event and application checks.

The full original run directory was preserved before resuming. Resume added
exactly two hashes, both for `counter-multi-tx`, retained all 34 original hashes,
and resubmitted none of them. The two new sources settled in L1 block 23549132
through the same posting transaction
`0x7d17beda2f023f3a00f0853fb855c9b7941c51902b1683ad3385f317b45c750e`.
All 34 scenarios were reverified, including all three counter sources across
their original and resumed settlements.

The final 36 sources comprised 20 L1 and 16 L2 transactions. All used type-2
Composer quotes, with zero node-fee fallbacks and zero tariff rejections.
Both formats were covered by the real offline-signing tests; this live run
does not add live legacy coverage. The monitoring and verification phases of
the original run took approximately 196 and 77 seconds. Resume took 94 seconds,
including 81 seconds revalidating all jobs. These are observed test timings,
not a comparable throughput benchmark or capacity claim.

## Limits and retained evidence

Default `all` excludes 28 scenarios: the six older exclusions plus the 22
call-pattern scenarios inherited from PR 54. They were not enabled here. The
770-job stress matrix, Falcon Safe workloads, independent follower replay and
native blob authentication were not rerun for this runner-only change. Their
earlier results apply to the deployed runtime, not to this PR's runner.
No Rust/runtime code changed, so Cargo, Docker and Kurtosis rebuilds were not
applicable to this trial.

Raw logs, the original failed run, private profiles, final link records and
read-only nonce comparison remain under the local-only ignored directory
`artifacts/protocol-pr56-20261016.yavu66yg` in the operator checkout. They are
not available in another checkout and have no retention guarantee. The small
patch and sanitized manifest accompany this record. The local fix is ready
for inclusion in PR 56; the trial did not publish a branch, post a review, merge
the PR or update the deployed protocol pin.
