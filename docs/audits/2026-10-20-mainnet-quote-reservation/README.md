# Mainnet fee-quote posting reservation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-20 UTC; both failures reproduced and fixed, affected native regressions passed with infrastructure retries documented below; full CI still pending; no deployment performed |
| Scope | Fix oversized posting reservations on both quote routes and retain safe diagnostics/partial traces |
| Source reviewed | Initial investigation: `353be39dd369fe4da1935796dcc23cd729f81398`. Correction: branch `codex/quote-poster-reservation`, based on main `93cfc91ecc4bf296a38c8cbdab8ff23565b19304` |
| Protocol | Clean initialized gitlink `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Live environment | Public `https://mainnet.eez.dev/composer/l1` and `/composer/l2`; chain IDs 1 and 696990 |
| Executable identity | Public L1 RPC reports `reth/v2.7.0-3d592ec/x86_64-unknown-linux-gnu`; this does not establish the Composer image digest/source. Exact deployed Composer identity was not obtained. |

## Reproduced behavior

Read-only native bridge calls used sender and recipient
`0x7B2e78D4dFaABA045A167a70dA285E30E8FcA196`, `value: "0x1"`, and selector
`0x5d81310b` followed by the ABI words for destination rollup ID and recipient.
L1 bridge: `0x1d6a3bfe0a302bb2df5fdfdb8eaf5b78b91a5fba`, destination 1.
L2 bridge: `0xBEF1406984A3e7b2889376d29D6BbD9f6ed9695a`, destination 0.

| Read-only method | L1 front | L2 front |
| --- | --- | --- |
| `eth_estimateGas` | Success, `0x212cd` | Success, `0x1e2a5` |
| `eez_debugQuoteFee`, `{"trace":"calls","transactionTypes":["0x2"]}` | `-32004`, `quote unavailable: gas estimation` | `-32004`, `quote unavailable: L1 settlement execution` |

On L1, the quote also failed with explicit gas `0x30000` and with either legacy
or EIP-1559 selection. Direct gas estimates succeeded with explicit transaction
types and zero fee fields. Source nonces and balances were unchanged before and
after the paired bridge checks. No transactions were signed or broadcast.

The live registry, rollup, verifier, posting wrapper and proxy bytecode matched
the estimator's supported fingerprints after the source-defined normalization.
The wrapper's embedded registry references matched the public deployment.
Queue observations were empty and both RPCs reported `eth_syncing: false`.
These observations do not establish every internal runtime binding or state read.

## Reservation mismatch

The operator supplied INFO-level `selected network posting quote` logs. The
posting wallet from the previously included post
`0x4f8d0d551571424a13117c514aad3a6b7050e599079d410a6eba626c23b793f5` is
`0x4261d69f840cc550945def48e5f477737118d546`. Its balance was queried using
`eth_getBalance` with each logged block hash and `requireCanonical: true`.

| Sample block | Probe gas | Execution fee cap, wei/gas | Execution reservation, ETH | Poster balance, ETH |
| --- | --- | --- | --- | --- |
| 26235814 | 16777216 | 1303965850 | 0.021876916722073600 | 0.020425963658678704 |
| 26235820 | 16777216 | 1333966982 | 0.022380252193882112 | 0.020425963658678704 |
| 26235822 | 16777216 | 1316549806 | 0.022088040470020096 | 0.020425963658678704 |

Execution reservation alone exceeds the available balance in every sample.
For the last sample, one blob adds `0.000002501116166144 ETH` at the logged
blob fee cap. This one-blob example is not a measurement of the failed quote's
encoded blob count. Ignoring other costs, that balance supports 15,512,867 gas
with one blob, below the initial 16,777,216-gas probe.

At the investigated baseline, both priced quote paths submit the full posting gas budget to the
native EVM, before measuring and selecting a smaller gas limit:

- [Outbound settlement pricing](../../../crates/eez-composer/src/composer/fees/settlement.rs)
  constructs `gas_limit: posting.gas_budget` and executes it before `declared_gas`.
- [Inbound priced setup](../../../crates/eez-composer/src/composer/estimate/l1.rs)
  initially selects `gas_limit.unwrap_or(posting.gas_budget)`.
- [Final signed posting](../../../crates/eez-composer/src/composer/posting.rs)
  already subtracts the blob fee-cap reservation and limits its initial gas
  probe to the remaining balance divided by the execution fee cap.

The locked REVM transaction interface checks `gas_limit * max_fee_per_gas +
blob_fee_cap * blob_gas + value` against the caller's balance. The quote's initial
probe therefore cannot pass that check for the observed wallet and prices.
This establishes a concrete mismatch between quote and final posting behavior.
The underlying production exception itself was not captured: the two conversion
sites log it at DEBUG, and the operator's log searches returned no matching lines.
The pricing arithmetic and code path strongly explain the reproduced errors;
no controlled mainnet balance/configuration change was made to demonstrate recovery.

## Diagnostics and correction

At the baseline, the native error becomes a generic unavailable error. These error branches leave
`quote_fee` before the optional trace is finalized, so the debug request also
loses its partial trace. Regular contract-revert diagnostics do not fix this path.

The correction shares final posting's affordable-probe calculation with both
priced quote paths, including physical blob reservations at the sampled state.
Native balance checks, complete effects and validation of the selected gas limit
remain enabled. A genuinely insufficient posting wallet produces the distinct
`poster_insufficient_funds` error with the sampled balance and affordable bound.
This does not invent an exact top-up amount when execution could not finish.

Debug calculation failures after capture begins finalize the collector into
`error.data.trace`, marked `partial: true`, without returning any usable quote
options. The viewer imports this error envelope and distinguishes the quote
failure from individual call outcomes. Provider error text is not exposed.
The [API contract](../../configurable-settlement-policy.md#compact-fee-quotes)
documents compatibility and limits; no protocol/configuration change is required.

## Follow-up validation

The local checkout is `artifacts/issue93-review-20261020/source`; commands run
from its root with Rust 1.95.0, LLVM 22 and Foundry 1.7.1. Test artifacts are
retained under its ignored `artifacts/quote-reservation-validation/` directory,
so they are local-only. No operator container, wallet or datadir is used.

The first regression run against copied, unmodified main runtime binaries
selected two tests and both failed: the affordable nested L1 quote returned
`quote_unavailable: gas estimation`; the unfunded quote returned the same
generic category instead of a poster diagnostic. Both tests originally looped
over directions and stopped at their first L1 assertion, so this run did not
establish a local L2 failure. The tests were then separated by direction. The
read-only mainnet observations above independently reproduce the L2 failure.

| Baseline executable | SHA-256 |
| --- | --- |
| Composer | `c8b848fc9a63cde6de6e8a16ff0cbd866e2f34feae256a3866e2616222fef223` |
| Follower | `183daea16a94788133903767b8ade01baf3d44c005bdb2dc6807349f4d1a1df0` |
| Proof signer | `11c083d9304ad084cf087ba75a50be501038e5e606bcc14b2e624cb6cb75897e` |
| Initial regression executable | `6ae139efd730208ec115b061ec1c75e6c007fc8d510e2335874d178a953592b7` |

Two native candidate builds were killed under host memory pressure (`SIGKILL`,
the second confirmed in kernel OOM records). A third build with package-local
`eez-node` optimization/debug reductions was stopped to protect the shared host
as available memory fell below 2 GiB. These are build/resource failures, not
passing tests. The shared-host OOM also killed a Composer belonging to a separate
dashboard `treasury_owners` test. The deployed Chiado Composer remained running
and Docker reported no OOM kill for that container. No service was restarted.
A subsequent single-job build used the normal profile with a guard to terminate
its own process group if available host memory fell below 3 GiB. It completed
successfully in 17m25s. Build logs and original process failures are retained.

The candidate runtimes were built with `cargo +1.95.0 build --locked --offline
-j 1 -p eez-node -p eez-follower -p eez-proof-signer --bins`, then copied to
`candidate-runtime/` before compiling test targets. No test-feature binary is
substituted for these processes. The follower and signer have unchanged binary
hashes; the changed Composer hash is
`7d6383dbe624b2db0a529f52991222abddd1b8ac07d38c8399aa686a0c950279`.
Process tests explicitly select these copies with `EEZ_TEST_RUNTIME_BIN_DIR`,
`EEZ_TEST_PROOF_SIGNER_BIN` and `EEZ_TEST_PREBUILT=1`. They retain separate
`EEZ_TEST_LOG_DIR` and `EEZ_TEST_DATADIR_DIR` trees for each comparison.

While the updated regression executable compiled, the unchanged 21 existing
`fee_quotes::` / `safe_fee_quotes::` tests ran serially against the new runtimes
using the retained baseline test executable. The new `poster_funding` tests are
excluded from that run and use the newly compiled executable for the baseline
and candidate comparison.

The first existing-suite run ended with 14 passes and seven failures after the
shared `/tmp` tmpfs user quota was exhausted. The first failing node explicitly
reported `durable witness capture unavailable: Disk quota exceeded`; later
fixtures reported the same quota error or exited with SIGBUS during startup.
These failures are retained in `candidate-existing-process.log` and its log and
datadir trees. The remaining seven cases were rerun separately with `TMPDIR`
under this audit's local artifact directory on the main disk. A retry pass does
not erase that failed first run. No unrelated temporary files or services were
removed to recover space.

The updated harness's normal-profile build was stopped by the memory guard.
The retry reduced optimization and debug information only for the harness's
linked `eez-node` library (`--config profile.dev.package.eez-node.opt-level=0
--config profile.dev.package.eez-node.debug=0`). It completed in 13m02s with
`cargo +1.95.0 test --locked --offline -j 1 -p eez-node-e2e --test node_e2e
--no-run --message-format=json`; all executed runtime processes still use the
previously copied normal-profile binaries. The new harness SHA-256 is
`d719185afa53ee01dae666eef0855edd82cf0de14d19c4683f7c4cf3d00931fe`.
The separated baseline/candidate comparison selected the same four tests with
this harness and disk-backed temporary storage:

| Scenario | Original runtime | Fixed runtime |
| --- | --- | --- |
| Affordable nested L1 quote | Failed: `quote unavailable: gas estimation` | Passed |
| Affordable nested L2 quote | Failed: `quote unavailable: L1 settlement execution` | Passed |
| Unfunded L1 poster, error and partial trace | Failed: generic `quote_unavailable` | Passed |
| Unfunded L2 poster, error and partial trace | Failed: generic `quote_unavailable` | Passed |

The positive cases exercise both transaction types and explicit/omitted source
gas, including at least three cross-chain transitions. Both negative cases
independently request legacy and EIP-1559 through compact and traced debug RPCs.
Positive quotes leave the poster's canonical balance and nonce unchanged. With
the fixture's 0.005 ETH balance and sampled prices, the initial posting probe was
3,172,723 gas instead of 16,777,216; final validated posting limits were 612,578
for inbound and 290,174 for outbound. These are gas limits, not charged receipt
gas or a performance benchmark.

The real-export browser run imported all eight generated JSON responses (four
successes and four partial failures). Both browser tests passed; the partial
failure screenshot was also inspected.

The remaining seven existing cases then passed with disk-backed temporary
storage, including both lower-margin policies, trace equivalence, ordinary L2
Safe execution and nested Safe creation with independent follower replay on
Ethereum and Gnosis. Across the local runs, all 25 distinct fee/Safe scenarios
passed: 14 existing cases on their first run, four new regressions, and seven
infrastructure-related retry-only passes (flakes). The initial failed run remains
a failure; this is not a clean first-run suite result. No runtime code or runtime
binary changed between those attempts. The new harness selected exactly the
seven initially failed cases using the retained inventory, excluding the 14
already passed cases and the four separate funding regressions.

Completed checks at correction commit `57ab7fbc305fae4f0c9d26a3a8cbf0606dabe083`:

- `cargo +1.95.0 test --locked --offline -j 1 -p eez-l1 -p eez-composer --lib`:
  194 L1 and 397 Composer tests passed. One Composer test was explicitly ignored:
  `held_pool::tests::profile_blocked_backlog_discovery`, a manual timing profile.
- `cargo +1.95.0 clippy --locked --offline -j 1 --workspace --all-targets -- -D warnings`:
  passed, including the separated process regressions and node RPC projection.
- Viewer model tests, two Chromium browser tests, all seven SDK tests, formatting,
  whitespace checks and the integration contract build passed. The later
  keyboard-test correction and the real exports described above also passed both
  browser tests.

[PR CI run 39485938467](https://github.com/eez-association/eez-rollupX/actions/runs/39485938467)
passed its Debug Unit Tests job: 1,521 workspace tests passed, including the new
node RPC error projection, poster diagnostics, reservation boundaries and trace
redaction checks. Three explicitly ignored manual utilities/profiles were skipped:
`profile_blocked_backlog_discovery`, `cold_span_feasibility` and
`inventory_native_proving_fixtures`. The separate Reth shutdown regression also
passed (one selected test; 206 unrelated tests filtered out).

That CI run reported two initial failures. The viewer keyboard file-picker test timed out
with Enter; the same test passed locally without changes. The follow-up uses
Space on the explicitly focused file input and asserts focus. This does not turn
the original CI failure into a pass. Lint stopped at `cargo deny` on the unchanged
lockfile's rustls-webpki/webpki-roots/webpki-root-certs advisories
RUSTSEC-2026-0381, RUSTSEC-2026-0382 and RUSTSEC-2026-0383. No dependency upgrade or
advisory suppression is included in this quote correction. These findings block
CI image promotion until resolved. The release-artifact build and all four
embedded E2E shards also passed; Kurtosis and the complete cross-chain CI matrix
remain pending at this point. No local Kurtosis enclave was started on the
resource-constrained shared host. No deployment is established by these checks.

## Retained evidence and limits

Raw read-only results are local-only in `/tmp/eez-quote-unavailable-20261020/`;
they are not available in another checkout and may expire. This record retains
the small reproduction inputs and financial observations independently.

| Artifact | SHA-256 |
| --- | --- |
| `mainnet-bridge-reproduction.json` | `e9469fdce730592eb8adcc567a21f3ea7ecd34b0f7d3eb63989f5ef1b2511dfb` |
| `mainnet-l1-validation-isolation.json` | `95ce0e25e05d5de8df0c174677aaebbc71edec781efb37505619f387c8c49602` |
| `mainnet-deployment-fingerprints.json` | `667bbdea86cd3fe2432698674b0dcc42abc923630729ad9e921d4e24d56e9afb` |
| `mainnet-confirmed-poster-reservation.json` | `3fb6c31f4eee6e519c5259f978c57a13556623e82eae069089d4bdcec8155efc` |

The initial investigation used source inspection, read-only live simulation,
the operator's posting-price logs and canonical balance queries. Follow-up local
validation is recorded separately above. No mainnet settlement broadcast or
runtime deployment was performed. Ordinary `eth_call` and `debug_traceCall`
do not demonstrate native affordability: a separate poster transfer probe with
the high gas limit succeeded through those RPC methods despite the cap shortfall.
