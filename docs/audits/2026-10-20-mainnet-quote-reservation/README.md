# Mainnet fee-quote posting reservation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-20 UTC; live failures reproduced, correction implemented; follow-up validation below; no deployment performed |
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
passing tests. Build logs and original process failures are retained; native
candidate validation remains pending at this point in the record.

Completed frontend checks: viewer model tests, two Chromium browser tests and
all seven SDK tests passed. The browser tests cover a partial-error envelope,
but importing new candidate process exports remains pending. Integration
contracts built successfully. Rust unit/process checks and release CI are
pending; this entry does not establish production readiness or deployment.

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
