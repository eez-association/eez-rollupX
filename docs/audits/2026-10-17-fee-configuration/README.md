# Explicit cross-chain fee configuration

| Field | Value |
| --- | --- |
| Date and status | 2026-10-17 UTC; local validation passed; exact release-artifact CI remains the merge gate; no deployment |
| Scope | Replace historical JSON policies with scalar settings and independent direction switches, preserving fee economics and quote schemas |
| Fork baseline | `eez-association/eez-rollupX`, PR 91 head `b5b34b8bcc8e8e50d639e6bc2dd4cb89bfcd47d8`; base `2c19e36bed2720c00f89841ccfca47ec7c921e06` |
| Implementation | `5eed3e00ca77a2bcdb9d65f3996058224b102613` |
| Protocol | Initialized and clean `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |
| Environment | Rust 1.95.0, Foundry 1.7.1, LLVM 22; locked local debug runtime and harness builds; serial native process tests; exact release-artifact qualification remains in CI |
| Owning contract | [Fee configuration and migration](../../configurable-settlement-policy.md#cross-chain-fee-configuration) |
| Accepted scope | [Configuration cleanup](../../proposals/cross-chain-fee-recovery.md#fee-configuration-cleanup-authorized-on-2026-10-17) |

## Findings

`FeeConfig` parses explicit `EEZ_FEES_*` settings once. Both direction switches
are false by default. Valid prepared refund terms do not activate L1 quoting,
price filtering, admission or refund dispatch. A complete cost model still
supports advisory L2 quotes. Runtime binding supplies chain IDs, the one owned
L2 rollup and the existing L2 beneficiary; mismatched measured receipts remain
ineligible. Partial models, invalid terms and obsolete settings fail startup.

The refund percentage remains configurable from 1 through 99 and fixed until
restart, with one percentage for the configured relays. It is independent of
the profit markup. Quote gross-up, EOA payout allowance, largest-payer selection,
signed cost/risk enforcement and observation-only refund reconciliation are
unchanged. This change does not qualify another builder, dynamically choose a
percentage or alter the existing Chiado 90% refund / 20% profit assumptions.

Compose and Kurtosis transport new and obsolete inputs unchanged to the node;
obsolete names are transported only for rejection. The old binary/configuration
must be preserved as a pair: old binaries ignore the new names. Migration
requires draining in-memory accepted work and replacing the effective settings,
including explicit Compose overrides. Compact/debug RPC schemas, signer wire,
contracts, genesis, historical observations and follower replay are unchanged.

## Reproduction and evidence

| Check | Result |
| --- | --- |
| Composer / L1 / node unit suites | 661 passed; one existing manual profiling test ignored |
| SDK | Six passed |
| Native fee and Safe suite | 17 passed, zero failures or ignored tests; semantic assertions and independent follower replay |
| Nested source attribution | One exact selected test passed |
| Chiado configuration | 18 passed; all three rendered Compose profiles passed |
| Tooling | 374 selected; 366 passed initially and eight skipped; the two missing-Gitleaks tests passed separately, leaving six opt-in exclusions |
| Workspace Clippy, Rust formatting, contract build | Passed |
| Full-history Gitleaks | Passed through the implementation commit; final documentation is checked before publication |

The native suite covers both transaction formats, internal estimation, ordinary
L2 treatment, disabled L1 quotes despite prepared refund terms, underpayment and
fee-sensitive rejection, caught reverts, posting-price changes, L1 missing-refund
continuation, pending-nonce replay, lower-profit acceptance and Safe initialization.
Startup observations show all four combinations of the independent enable
switches reaching the real node. These observations supplement the behavioral
assertions; startup alone does not establish fee enforcement.

Commands ran from the feature checkout root, with
`CARGO_TARGET_DIR=/home/edu/Development/rollupX/target`,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, LLVM's bin directory on `PATH`, and
`CARGO_BUILD_JOBS=2` for the initial builds/checks. The harness retry uses one
job after the resource failure described below. Local process tests clear all
prebuilt-runtime overrides and retain their logs and datadirs. No live network
or operator wallet is used.

The sanitized [validation record](validation.json) records counts, commands,
source/binary identities and log digests. Raw artifacts under
`artifacts/fee-config-cleanup-20261017` are
**local only**, unavailable in another checkout; the checked-in summary does not
make those artifacts durable or downloadable.

Initial iteration failures are preserved: an example-file edit temporarily
removed the required coinbase line and failed the static Compose fixtures;
a moved value prevented compilation of one unit test; Clippy rejected one test's
empty-string spelling. These were corrected before the successful checks.
The first broader unit run also failed 22 L1 loopback tests because the sandbox
forbade local TCP servers. The same suite passed with loopback access. These are
recorded changes/environment corrections, not silent test retries.
The only Rust edit after that unit run changed one test's empty-string
construction to `String::new()` for Clippy; the runtime code was unchanged.
The final Clippy and process builds include that spelling change.

The initial harness build was killed before any process test ran. Kernel logs
confirm an out-of-memory kill of `rustc` at 11,551,768 KiB anonymous RSS while
Cargo allowed two jobs. The one-job retry passed in 17m 55s and preserves the
first build log and kernel summary. Read-only inspection confirmed that the live Composer
and signer remained running with their original October 16 start times; neither
was OOM-killed or restarted.

After the 17-test fee/Safe suite passed, a separate Cargo invocation began
rebuilding dependencies following the standalone signer build. That compile was
interrupted before the attribution test ran. The follow-up uses the same
SHA256-verified source-built harness directly, with its exact one-test filter.
The first direct invocation exposed an environment omission: Cargo was pinned,
but the host's default `rustc` was 1.96.1. It was stopped during signer setup;
the successful invocation pins `RUSTUP_TOOLCHAIN`, `CARGO` and `RUSTC` to 1.95.0.
Both interrupted logs are retained. Prebuilt overrides remain unset, and the
testkit still builds its signer from source. These setup interruptions are not
reported as transaction-test passes or failures.

Harness compilation relinked the runtime executables, and the testkit built the
standalone signer before execution. The validation record distinguishes initial
runtime-build hashes from the actual fee-suite and final attribution-test hashes.
The attribution test reused the identical harness and runtime source; no Rust
source changes were made after the implementation commit.

## Limits

No fee economics or performance claim is inferred from configuration checks.
The full CI release-artifact/Kurtosis matrix is the merge gate. Opt-in resource
and artifact probes are exclusions, not coverage. This task does not redeploy
Chiado, change operator configuration, rerun the live stress matrix, demonstrate
durable queue recovery or qualify builder payouts. Existing in-memory queue and
refund attribution limits remain.
