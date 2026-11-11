# Compact fee quote API validation

Date: 2026-10-15 UTC. Initial local validation is recorded below; the later
Docker deployment and qualification are recorded in the follow-up section.
This records implementation evidence, not an independent security review.

| Field | Value |
| --- | --- |
| Backend baseline | `4fc21ca100392daebd2852bedc345aa5fad46620` |
| Backend implementation | `149968a373696a5f01cfb1ef8d6aae176f122a42` |
| Initialized protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |
| Dashboard baseline / implementation | `d2e85905864ec88de493829d763a8cfeb9a45b99` / `6cc89033064276843325af5607bb3bfd724cc44c` |
| Local branch in both repositories | `feat/compact-fee-quotes` |
| Toolchain | Rust 1.95.0, LLVM 22, Foundry 1.7.1; locked Cargo dependencies |

The accepted scope and API contract are in the [proposal](../../proposals/compact-fee-quotes.md)
and [settlement policy](../../configurable-settlement-policy.md#compact-fee-quotes).
The previous [transaction-reference validation](../2026-10-15-transaction-references/README.md)
does not establish validation or deployment of this API.

## Implementation and boundaries

`eez_quoteFee` and `eez_debugQuoteFee` replace the old RPC, HTTP quote route and
SDK compatibility helpers. Missing gas, nonce and chain ID are resolved inside
the service. Legacy and EIP-1559 variants are priced independently against the
same pinned context, with final gas/fee-sensitive native replay. The diagnostic
method adds calculation details to the identical compact result. Unknown or
changed context cannot return partially successful variants.

The SDK validates the intent binding, selected format, network, expiry, maximum
cost and wallet pending nonce before one normal wallet request. The reference
dashboard and known protocol/browser test drivers use the new methods. Wallet
quantity normalization remains for supported wallet observations; this does not
preserve any old quote API.

Measured settlement pricing, signed tariff admission and final dispatch funding
checks are preserved. Ordinary L2 execution receives no settlement surcharge.
L1-origin quotes explicitly report `unsupported_direction`; a builder-refund
recovery policy remains outside this implementation. No deployed contracts,
genesis, protocol pin, dependency versions, signer RPC or storage changed.
`FeeQuoteProbe.sol` is a local process-test fixture only.

This is a coordinated server/client API replacement. Old clients fail on the
new Composer and new clients fail on old servers. Upgrade or roll back both;
no data migration or historical follower replay migration is needed. See the
[operator runbook](../../runbooks/chiado-deployment.md).

## Reproduction and evidence

Work used `/tmp/rollupx-l2-fees-e2e.8oFEZzNX` and
`/tmp/eez-ui-l2-fees.Qm8H2JyN`. The operator checkouts' existing work and settings
were preserved. Raw logs, frozen executables and failure datadirs are local-only
under `artifacts/compact-fee-quotes-20261015/` in the operator backend checkout;
they are ignored and unavailable from another checkout. [results.json](results.json)
retains small sanitized results, executable hashes and log hashes.

The runtime was built and copied before building the process-test harness.
Its source was the code subsequently committed as the implementation above:

```bash
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
export CARGO_BUILD_JOBS=2
cargo +1.95.0 build --locked --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-node -p eez-follower -p eez-proof-signer --bins
cargo +1.95.0 test --locked --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-node-e2e --test node_e2e --no-run --message-format=json
```

The command-line optimization override applies only to the local `eez-node`
development package. It limits compiler resource use and changes no dependency
or feature selection; these are not shipping release/Docker binaries. The test
runner used `EEZ_TEST_PREBUILT=1`, the frozen `runtime-final` directory for
`EEZ_TEST_RUNTIME_BIN_DIR`, its signer for `EEZ_TEST_PROOF_SIGNER_BIN`, and
separate retained log/datadir directories. Thus it could not select stale host
executables. Tests ran serially on disposable local Anvil chains.

The first new ordinary-call process test failed because the code checked for
empty actions after protocol finalization, which already rejects empty calls.
Both gas discovery and pricing now inspect the recorded call count before
finalizing and replay ordinary execution natively. The identical test passed
after rebuilding the runtime and harness. The first failure, datadir and
executable identities are retained; this is a code correction, not a retry-only
pass. Initial node unit socket failures under the filesystem/network sandbox
passed once the tests had permission to create local sockets.

All **12 selected process tests passed**: the new ordinary/boundary regression,
eight nested/Safe fee scenarios, and three wallet estimator regressions. No
selected process tests were skipped or ignored. The latter cover both chain
profiles and the post-batch wrapper. Native selectors were:

```bash
artifact_dir=/home/edu/Development/rollupX/artifacts/compact-fee-quotes-20261015
"$artifact_dir/node_e2e-final" fee_quotes::compact_ordinary_calls_boundaries_and_fee_sensitive_rejection --exact --nocapture --test-threads=1
"$artifact_dir/node_e2e-final" fee_quotes:: --skip compact_ordinary_calls_boundaries_and_fee_sensitive_rejection --test-threads=1
"$artifact_dir/node_e2e-final" cross_chain::wallet_gas_estimates_ --test-threads=1
```

The executables in these commands reside in the local artifact directory; the
working directory was the backend clone and the environment was as above.
Composer units passed **351**, with one existing manual backlog timing test
ignored; node units passed **86**. Contract tests passed **86**. The SDK passed
**5**, dashboard payment checks **4**, discovery **27** and debugger **78**.
Mocked browser suites passed **16 fee scenarios** and **27 bridge scenarios**.
The dashboard production build passed with its existing large-chunk warning.
Workspace Clippy (`--locked --workspace --all-targets -- -D warnings`), Rust and
contract formatting, edited-link checks and `git diff --check` passed.

The final checks and log identities are recorded in `results.json`.
Nested and Safe process tests compare native receipts, tariff rejection, target
state and independent follower replay. Browser tests use mocked EIP-1193
providers, not actual MetaMask/Rabby extensions, and broadcast no transactions.
The UI production build, SDK tests, payment-boundary tests, discovery/debugger
regressions, contract tests, Rust formatting and workspace Clippy are included.

## Initial validation limits

At the end of the initial local validation, no new Docker image, Kurtosis topology/PBS run, public-network transaction,
operator restart or live deployment was performed. Kurtosis scripts were
migrated and syntax-checked, not executed. The full protocol staged/stress
commands and complete CI release matrix were not run. Actual Falcon signature
workloads remain unqualified: the larger gas limits in the tests validate the
request/admission profile, not Falcon execution cost. Quotes remain advisory;
this evidence does not establish durable queue recovery or future-price coverage.

## Docker deployment follow-up, 2026-10-15

The user authorized building local images, deploying first, and running Kurtosis
validation in parallel. The runtime source is
`a9313f9ad4eb54e79d628172381866accb3f7da7`; its production code is the implementation
`149968a373696a5f01cfb1ef8d6aae176f122a42` above, with validation documentation added.
The dashboard source is `6cc89033064276843325af5607bb3bfd724cc44c`.
The protocol remains `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`.

The repository's node/signer Dockerfiles produced locked `release` profile
images; the Kurtosis deploy image used their genesis helpers. The matching
dashboard image was built from its clean checkout. These are locally built
development images, not a promoted CI release. No registry tag was published.
[deployment.json](deployment.json) records immutable image IDs, executable
hashes and source identities. Both live containers and Kurtosis were checked
against those exact IDs; live executable hashes match the extracted candidates.

The existing Chiado runtime was upgraded under temporary gateway maintenance.
The accepted queue was empty before both runtime writers stopped cleanly.
Complete stopped L1/L2 databases were copied to new isolated paths, with the
originals retained. Only the runtime images, candidate database paths and UI
image changed in the resolved Compose configurations. The matching UI was
upgraded in the same window. Policy, keys, deployed contracts, genesis, bindings
and posting-history storage were preserved. The pre-upgrade safe block remains
canonical and settlement advanced after restart. Public routing was restored
after local API and canonical-canary checks.

The live canary reused an existing owned nested fixture, held both protocol
checkout wallet locks, and capped the maximum fee reservation at 0.005 native
units without funding or deploying another contract. It omitted gas for internal
estimation and submitted one paid legacy transaction. An underpaid signed
variant was rejected before ownership/execution. The paid transaction reached
canonical consumption, updated the expected L1 state, and had successful L1/L2
receipts and matching references on both Composer fronts. Source gas was 99,149;
quoted and actual posting gas were both 173,707. The source paid
365,734,801,552,714 wei in tips against a complete L1 posting/blob cost of
304,779,001,215,949 wei and a required 20%-margin tariff of
365,734,801,459,139 wei.

Read-only public checks passed for compact/debug methods, both transaction
formats, omitted gas/nonce, the explicit 16,777,216 gas cap on ordinary L2,
ordinary-L2 zero settlement surcharge, removed old RPC, unsupported L1 direction
and absence of fee methods on the plain node RPC. The supplied `cast` syntax was
executed against the public API. A browser loaded the deployed dashboard assets
and fee controls without page errors. This smoke check did not use a real
MetaMask or Rabby extension.

The initial local API smoke script tried the 16M limit on the small canary
wallet's nested call. Native validation rejected that option: its full
`gas * gasPrice` reservation exceeded the wallet balance. The ordinary-L2
comparison accepted and preserved 16M. The checker was corrected to use that
funded comparison; this is not evidence of a successful 16M nested execution.
The first response is retained. The initial local Kurtosis orchestration attempt
also stopped before any workload because its deployment-download helper required
`ENCLAVE`; the corrected caller sets it from the explicit `KURTOSIS_ENCLAVE`.
Its first diagnostic is retained. Neither required a product-code change.

Raw build, handover and test logs, snapshots, blobs and datadirs are local-only
under `artifacts/compact-fee-quotes-deployment-20261015/` and the recorded ignored
operation directory in the operator checkout. The UI migration prompt and
read-only `cast` examples are retained there as `ui-migration-prompt.txt` and
`manual-cast.sh`. These ignored artifacts are unavailable from another checkout.
The committed result file retains small sanitized evidence and hashes.

The isolated `eez-compact-fees-20261015` enclave used private chains 7331/696990,
native blobs, a real builder/relay/validator topology, enforced fees, 1:1
conversion and a 20% allowance. The four `EEZ_SKIP_*_BUILD` flags selected the
already built images. Every operating shell exported its explicit enclave name
and argument file. Validation waited for the fresh relay's first canonical
settlement before submitting the workload.

Executed checks all passed:

- `verify-eezl2-deployment.sh` verified genesis, proxy, implementation, owner and
  deployment bindings.
- `verify-l2-fees.mjs` quoted through the UI's Composer route, rejected an
  underpaid source before ownership/execution, and settled two independent
  nested sources in the same native block. Receipt costs, beneficiary, target
  state and canonical disposition passed.
- `verify-l2-bridge-fees.mjs` funded actual L1 escrow through an inbound deposit,
  then verified EIP-1559 and legacy withdrawals with internally estimated gas.
  Quoted/actual L1 gas was 492,775/492,799 and 182,933/182,945. Tip-to-complete-
  posting-cost ratios were approximately 1.2006 and 1.2002; both satisfied
  coverage and the existing 2% tolerance above the 20% allowance. L1/L2 user
  balance changes matched the transfer value and source receipt fee.
- The dashboard's `tests/l2-fees-kurtosis.cjs` completed a real browser-to-
  settlement transaction with one compact quote and one wallet request. It
  checked intent binding, receipt fees, beneficiary, canonical posting and L1
  target state. The wallet was an injected fixture adapter, not an extension.
- Canonical reference checks covered five source groups and nine physical L2
  transactions; both fronts agreed and unknown-source lookup remained null.
- Five native blob postings were authenticated, including the browser posting
  and inbound deposit, with semantic tables and outbound bytes bound to their
  settlements. The separate live Chiado canary blob was also authenticated.
- One intended `kurtosis_policy_replay::` process test passed with zero ignored
  selected tests, using the extracted Docker follower and the frozen harness
  described above. It independently replayed canonical L1 data and checked 60
  L2 blocks and five source receipts. This invocation rebuilt no runtime.

The task's enclave was stopped after evidence capture, retaining its state.
Unrelated enclaves and live Chiado services were left running. No product-code
changes followed the image build.

The full protocol staged/stress matrix, complete CI promotion lanes, actual
wallet extensions and actual Falcon signature workloads were not run.
Independent replay was performed on Kurtosis, not the entire Chiado history.
These results do not establish durable queue recovery or future-price coverage.
L1-origin recovery remains deferred. A rollback requires matching clients and
reconciliation of canonical progress after the stopped snapshots; those copies
cannot reverse settlements already posted on L1.

## API cleanup follow-up (2026-10-15, not deployed)

The next draft revision makes the normal quote response minimal and replaces
Ethereum-ambiguous intent terminology. Backend implementation
`854ad0d7b3f2dfbf9ee110a1c4cbc36faf7a628e` follows `dbf53ef2`; dashboard
implementation `0650ec83ea16fcf927d67877bad4ebac63b6e90d` follows `6cc8903`.
The initialized protocol remains `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`.

`eez_quoteFee` now contains only `version`, `chainId`, `nonce`, `expiresAt` and
`options`. `eez_debugQuoteFee` exposes enforcement mode only at
`diagnostics.recoveryMode`. The SDK takes `transaction`, option bindings use
`requestHash`, and detailed unsigned inputs use `request`. Input error codes
are `invalid_transaction` and `unsupported_transaction`. Binding bytes, fee
arithmetic and native enforcement are unchanged; no old aliases are accepted.
Upgrade Composer and clients together. L1-origin recovery remains deferred.

[api-cleanup.json](api-cleanup.json) records the exact implementation, executable
hashes and results. Raw evidence is local-only under
`artifacts/compact-fee-api-polish-20261015/`; it is ignored and unavailable from
another checkout. Runtime binaries were built and frozen before compiling the
process harness, using the same locked development-profile commands above.
The serial `fee_quotes::` filter selected six fee tests and three Safe tests.
All nine passed with zero ignored or failed tests, including nested settlement,
underpayment rejection, ordinary-L2 behavior and independent follower replay.
The tests check the minimal root schema, debug-only metadata and new binding
names against real Composer processes.

Composer units passed 351 tests with one existing manual timing profile ignored;
node units passed 86. The SDK passed five tests. Dashboard checks passed four
payment, 27 discovery, four gas-estimation and nine bridge-MAX tests, plus 16 fee
and 27 bridge mocked-browser scenarios. No real transactions were sent by those
browser suites. The production UI build passed with the existing chunk-size
warning. Workspace Clippy with warnings denied, Rust formatting, JavaScript
syntax, identical SDK copies and documentation checks passed.

No new Docker image, Kurtosis run or live deployment was performed for this
cleanup. The earlier deployment evidence above remains valid only for its
recorded runtime/UI revisions. The complete CI/staged/stress matrix, actual
wallet extensions and actual Falcon signature workloads remain unverified here.

### CI secret-scanner correction

The first draft-PR lint job stopped before Rust checks with eight scanner
findings. Reproducing its pinned Gitleaks 8.30.1 command over fetched history and
PR merge diffs found four nonsecret values twice: the public fee-policy variable
name, a verified Git commit ID and two SHA-256 digests verified against retained
RPC evidence. [ci-scan.json](ci-scan.json) records the failed job, reproduction,
classification and evidence hashes.

The correction adds exact-value AND exact-path exceptions only. The same history
scan then found zero issues. A separate synthetic control introduced new fake
credentials into all three affected paths; all three remained detectable while
the known nonsecret values were ignored. Default rules, full-history scanning and
merge-diff coverage remain enabled. No runtime code changed; this scanner check
does not stand in for the remaining CI lanes or a deployment.
