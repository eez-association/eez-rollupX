# Streaming deadline characterization: preparation and blocked measurements

**Initial partial investigation for [#148](https://github.com/eez-association/eez-rollupX/issues/148).
Subsequent runtime/release correctness passed; actual quiet-window timing remains
pending in [the follow-up](followup.md).** No latency samples,
deadline success rate, performance improvement or supported functional defect
has been established by this work. This record does not complete #148.

The [measurement-driver follow-up](followup.md) records subsequent implementation
and validation after resource recovery; the initial results below remain historical.

## Identity and scope

| Field | Value |
| --- | --- |
| Implementation baseline | Association `main`, `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b` |
| Protocol gitlink and initialized checkout | `84a5b5207880f192a6a5896cbfe0bac5249db47f`; recursive initialization completed; no protocol edits |
| Branch | `issue148-streaming-deadlines`; initially clean, dedicated worktree |
| Runtime change | None; documentation and static evidence only |
| Runtime/test executable identity | Unavailable: no Rust/native binary built or executed for this investigation |
| Installed tools | Rust 1.95.0 (`59807616e`), Cargo 1.95.0 (`f2d3ce0bd`), Foundry 1.7.1 (`4072e487`), protoc 3.21.12 |
| Current identity contract | Validation version **9**, offline profile 2, operations tag 1; signing domain remains 7 |
| Environment | Shared Linux host, 22 CPUs in command affinity; existing background services remained running |

This continues **N14**, rather than restarting the backport audit. Its frozen
source remains `1b66d9bda675e6f8e83f514153a97136909acc06`, protocol
`143d87e62cade98bfc122d612638dcaaf1c85633`; frozen target remains
`31e148f50dd79730b17d113ac78795eb377eb5c5`, protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. The later issue check at
`716a8a3bad4415ade7e9a6bc3af4a1b35419b232` is not that frozen target.
**The original review's input provenance was not independently checked.**

Overlaps inspected: #144 owns missing-blob observations, #146 conservation,
#147 tracing probes, and #142's startup/retry implementation is PR #163.
None of those files, other worktrees or their outputs was changed. The existing
main CI run `43237662996` was not cancelled, modified or rerun. No production
restart, live fault injection, wallet operation, deployment or production
transaction was performed. No additional agents were launched.

## Accepted contract and production path

The owning sources are the [streaming contract](../../../crates/eez-control-rpc/STREAMING_SPEC.md),
[RPC contract](../../../crates/eez-control-rpc/SPEC.md),
[signer contract](../../../crates/eez-proof-signer/SPEC.md),
[request lifecycle](../../../crates/eez-proof-signer/docs/request-lifecycle.md),
[signer contributor guide](../../../crates/eez-proof-signer/CONTRIBUTING.md),
[validation identity](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md),
[settlement policy](../../configurable-settlement-policy.md) and pinned Core
[multi-prover specification](../../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md).
The [historical streaming proposal](../../proposals/streaming-prover-backport.md)
explicitly defers to the current offline profile.

Source inspection at this baseline establishes the following paths, not fresh
runtime results:

| Stage | Production path and boundary |
| --- | --- |
| Optional preparation | `eez-node/src/lib.rs::run_preparation_worker` coalesces targets, loads durable ancestry in a blocking worker and calls `RemoteProver::prevalidate_range`. Failure defers optional work; final proving retains backfill responsibility. |
| Prevalidation | `eez-prover-client/src/streaming.rs` divides ranges into chunks of at most 64 blocks and releases the exchange lock between chunks. Each exchange has a timeout covering lock acquisition, transport, responses and EOF. |
| Recovery and upload | The client opens a transport, tries Resume and reconciles number **and hash**. Missing sessions require Begin/backfill. L1 segment joining removes an identical shared endpoint; it cannot omit intermediate headers or proof-relevant inputs. |
| Signer Block | `eez-proof-signer/src/service/sessions.rs::validate_block` checks identity, continuity and quotas, then bounds shared-worker admission and blocking validation. A worker owns its permit until exit. |
| Finalize | The same module selects retained checked execution, admits the final-proof permit and shared worker, validates full settlement, then checks deadline/epoch around signing. ACKs do not authorize signatures. |
| End-to-end attempt | `eez-composer/src/prover_retry.rs` and RPC section 6.3 retain one exclusive cutoff, monotonic bound and post-proof reserve across preparation, recovery and retries. Planning allocations do not independently discard an accepted candidate. |

Default signer limits are 65,536 blocks, 536,870,912 bytes, 1,000,000 witness
items, a decode ceiling of at most 268,435,456 bytes, 120-second idle timeout
and 600-second ordered-operation timeout. There are eight streams/sessions,
one shared validation worker and one final-proof permit; the client pipeline
is at most eight operations. These are configuration/resource bounds, **not
latency guarantees**. CLI flags override environment values; limits and timeouts
must be nonzero. Current production settings were not read from operator secrets.

[#43](https://github.com/eez-association/eez-rollupX/issues/43) and
[#45](https://github.com/eez-association/eez-rollupX/pull/45) concern the old
`PrepareOrdinary` ingestion deadline and its preparation permit/cache publication.
That API no longer exists in this profile. The present investigation neither
reproduced nor rediscovered that fixed defect. Current prevalidation, exchange,
ordered-operation and settlement deadlines must be tested on their own terms.

There is documentation drift: the supplied/root agent guidance and conformance
overview refer to validation version 8, while the owning identity specification
and `identity.rs::VALIDATION_VERSION` select **9** for native operations V1.
Use the actual version-9 identity for these binaries; do not downgrade it to
match historical prose. This record changes no identity or validation semantics.

## Defined workload and actual input sizes

[workloads.json](workloads.json) records SHA-256 identities, public deployment
configuration, exact per-block frame sizes, L1 segment charges and a split
after block 15. Inputs are the three existing complete native captures under
`crates/eez-proof-signer/tests/fixtures/`, with the genesis, identity and response
files in each capture's directory. Their READMEs preserve original process
provenance, Core proof-policy adaptation and lookup witness supplementation.
They are representative of these small native scenarios, not a production
traffic census or a maximum-size witness workload.

| Capture | Blocks | Delimited file bytes | Protobuf payload bytes | Block frame bytes | Witness data bytes / items | L1 headers / raw bytes | Finalize frame bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `native-offline-bidirectional` | 30 | 372,458 | 372,390 | 238,098 | 208,965 / 999 | 7 / 4,387 | 134,218 |
| `native-osaka-lookup` | 45 | 495,704 | 495,605 | 362,241 | 321,084 / 1,649 | 10 / 6,277 | 133,290 |
| `native-osaka-changing-static` | 30 | 375,974 | 375,906 | 240,814 | 211,321 / 1,024 | 7 / 4,387 | 135,018 |

Each has a 74-byte Begin and one physical 131,072-byte blob. Block frames range
from 4,176 to 25,624 / 43,416 / 28,340 bytes respectively. Every capture starts
at block 1 and includes all blocks through its terminal. Witness items count
state nodes, code, key preimages, L2 ancestry and L1 headers. Witness data bytes
sum the raw array elements; they exclude the block body and protobuf framing.
Session admission charges known encoded Begin and newly accepted Block frames;
Finalize must fit alongside retained bytes. These are distinct from allocation,
HTTP/2 traffic, the five-byte gRPC prefix, and the file's varint delimiters.

The first 15 blocks in all three captures cost 138,924 Block-frame bytes,
522 witness items, and four L1 headers / 2,500 raw header bytes. Remaining
suffixes cost respectively:

| Capture | Missing blocks | Block-frame bytes | Witness items | Additional L1 headers / bytes |
| --- | ---: | ---: | ---: | ---: |
| Bidirectional | 15 | 99,174 | 477 | 3 / 1,887 |
| Lookup | 30 | 223,317 | 1,127 | 6 / 3,777 |
| Changing static | 15 | 101,890 | 502 | 3 / 1,887 |

These are exact **capture** charges. A production client may regenerate envelope
IDs, reconcile an ACK lost after validation, resend a terminal bridge or join
L1 segments. The measurement driver must count actual transmitted frames and
bytes as well; it cannot report the census as measured network traffic.

The census decoded every length-delimited frame with a small one-off standard
library wire-field traversal and separately required `protoc --decode=prove.v2.ClientFrame`
to accept all **111 frames** against the pinned schema. It asserted one Begin,
one Finalize, and contiguous 1..N block numbers. This is static accounting,
not execution or settlement validation. No new persistent parser was added.

## Runtime experiment to execute after prerequisites pass

**Not executed; the measurement driver, builds and runtime checks remain work.**
Use the real production `RemoteProver`, signer TCP service and Stateless backend,
with local public fixture configuration. A wire transcript replayer alone does
not exercise Composer preparation or the production client's recovery path.
Use one client and one fresh signer/session per repetition, loopback ephemeral
ports, and owned temporary witness/datadir paths. Do not contact a production
signer or submit fixture proofs to a production chain.

The bounded selection is each capture above under these four scenarios:

1. Cold: complete Begin, all blocks, exact Finalize, verified Proof and EOF.
2. Prepared: prevalidate blocks 1..15 through the production preparation path,
   then prove the unchanged full window. Record preparation and proof separately
   and their sum; preparation is real work, not free cache time.
3. Retained disconnect: disconnect after validating block 15, Resume with
   number/hash reconciliation, then complete the exact original request.
4. Lost session: perform the same preparation, restart only the owned test
   signer, require missing-session recovery and full Begin/backfill. Count both
   the discarded preparation and retransmission of all L1 evidence.

Use explicit standard limits above for the workload experiment, and record the
actual advertised capabilities, independently computed version-9 validation
identity, loaded genesis/configuration digests, proof-system/vkey/attester,
source/protocol revisions, Cargo.lock hash, feature set and every executable's
SHA-256 before running. Pin a release build with the shipping feature selection
from `Dockerfile.runtime`; do not mix debug unit-test duration with release
daemon latency. Build with `--locked` and at most two jobs, one under competing
work; finish all builds and correctness validation before quiet-window admission.
Never reuse another task's mutable target directory or unverified cached binary.

For each fixture/scenario, run three warmups then 20 measured repetitions.
Exclude warmups from distributions but retain their outcomes. Fresh service
state means cold **session/execution** state, not a claim of cold OS page caches;
record KZG initialization/readiness separately. Do not drop host caches.
Retain every attempt, timeout, fault, recovery and semantic failure; no silent
retry or success-only timing. Bound each group to 15 minutes; a partial group
reports its actual count instead of claiming 20 observations.

Use monotonic `std::time::Instant` for elapsed preparation, witness-store read,
capability/connect, Begin/Resume, Block upload/ACK, Finalize/Proof/EOF, recovery
and complete-attempt durations. Record UTC only for host/log correlation.
Capture server worker admission/execution separately where instrumentation
permits; otherwise report combined intervals without inventing a decomposition.
Record actual blocks/bytes/header items sent, accepted and retransmitted.
An external process timer includes startup and must be labelled separately.

Report raw per-attempt durations in milliseconds, success/error counts, min,
median, p90, p95, max and interquartile range per scenario. Use nearest-rank
percentiles and disclose N=20; p95 is the 19th ordered observation and the
maximum is the 20th. Do not infer p99, an SLA, saturation capacity or general deadline
guarantees from this sample. Keep fault/deadline tests out of successful latency
distributions; report their actual status and completion/recovery times separately.

With no runtime change, there is no candidate speedup claim. If a reproducible
defect subsequently justifies a fix, run the **same** driver/workloads on
`601ebfa9...` and the candidate, with identical dependencies, features, profile,
configuration and host conditions. Preserve baseline failure and candidate
regression evidence. Alternate baseline/candidate ordering over 20 pairs after
the same warmups; report variability and regressions, including preparation and
retransmission costs. Instrumentation must be identical on both sides.

### Logical deadlines and semantic assertions

Use controlled synchronization and Tokio's virtual clock where supported for
before/at/after deadline checks, rather than treating a scheduler delay as a
wall-clock performance defect. The existing 50 ms admission/exchange tests are
logical regressions, not a workload-specific 50 ms promise. Add any missing
boundary test only after identifying the exact owning gate. Include worker
admission, progressing input, response backpressure, exchange mutex, lost ACK,
Cancel/Resume/Rewind fencing, recovery and original-cutoff exhaustion.

Each positive workload run must check every ACK's correlated binding, number,
hash and post-state root; compare the final digest with the independently
constructed expected input and recover the public fixture attester's signature;
require exactly one Proof then EOF. Preparation alone must not sign. Rejection
must preserve the prior validated prefix, make no usable proof available and
permit the specified same-epoch retry or full backfill. A timed-out running
worker must retain its permits until exit, and old epochs must not publish.
The retry allowlist, exact cutoff, post-proof reserve and original request
ownership must remain unchanged. ACKs, relay acceptance and equal heights cannot
count as canonical consumption.

Run the native process checks separately to establish real rejection/status,
network-denied signing and independent follower/settlement behavior. A frozen
capture replay can establish fresh independent execution only when actually
run; it does not itself establish fresh on-chain acceptance or follower replay.

## Quiet-window gate and observed blocker

Prospective practical eligibility criteria for this experiment (not product
requirements or a preregistered policy):

- No active Rust/native compiler, linker, Cargo/Clippy/Nextest, Foundry/Anvil,
  process suite or other identified CPU/IO-heavy validation. Check all host
  processes, including `clippy-driver`, before, throughout and after sampling.
- At least 120 consecutive seconds of stable observation at 10-second intervals:
  host CPU idle at least 70%, iowait below 5%, and one-minute load no greater
  than half the available CPU count. These are coarse contamination screens;
  record affinity and host-counter scope separately.
- At least 12 GiB `MemAvailable`, no interval swap-in/out, and memory PSI
  `some avg10` below 1%. Existing swap occupancy alone is not proof of current
  pressure; retain it and its trend. Reject OOM, quota or allocation failures.
- At least 5 GiB free on root and the owned evidence volume, with no ongoing
  storage/quota failure. Builds need separately adequate owned target capacity;
  these minima do not establish that a full native build fits.

Observe `/proc/stat`, `/proc/meminfo`, `/proc/vmstat`, load, CPU affinity,
`/proc/pressure/{cpu,io,memory}`, `df`/disk usage and command names from `ps`.
Keep monitoring during the complete measurement group and for 120 seconds
after it. Process checks must run at each interval during actual timing, not
only at the three checkpoints used for this rejected preflight. If heavy work
starts or a criterion fails, retain the diagnostic group as contaminated,
postpone and repeat in a comparable eligible window. Never interrupt others,
prune their storage, stop normal services or treat an isolated worktree as an
isolated host.

[host-observation.json](host-observation.json) retains the compact, sanitized
preflight: **2026-11-12 16:04:46–16:06:46 UTC**, 13 snapshots / 12 intervals.
Root free space was **483–560 MiB**, below the gate throughout. Idle CPU was
61.07–70.82%; one-minute load 8.44–9.28. Available memory was 16.06–16.55 GiB;
swap occupancy was about 7.23 GiB, with no observed interval swap traffic.
Memory/IO PSI avg10 was zero at the before/during/after checkpoints. Rustc,
Cargo and Clippy activity was observed at those checkpoints; the middle
top-process list also contains `clippy-driver`. The host was ineligible.

Normal `beam.smp`, Composer, Caddy and PostgreSQL services were observed and
left running. Their CPU/IO/cache contention remains part of this shared host's
environment even in a later eligible window. No host-isolation claim is made.
The raw local-only snapshot file is identified and hashed in the committed
summary; it is not needed to read the retained interval results.

A later check at 16:10:51 UTC still showed only about 398 MiB free on root,
load 12.75/10.95/10.51 and active `du` processes. No new build or measurement
was admitted by that check.

Before this observation, the first ordinary sandbox baseline command failed
at startup with `synthetic bubblewrap mount ... Quota exceeded (os error 122)`.
The first sandbox GitHub read failed to connect. Approved ordinary execution
outside that sandbox allowed read-only inspection and later branch/submodule
operations. Another sandbox source read hit the same mount quota failure.
Those are environment failures, not failed signer tests. The submodule
initialization completed; transient root free space afterwards was about
162 MiB, reinforcing the decision to defer builds. No failed build was retried
or disguised as a passing test.

## Validation and remaining work

Executed at the requested baseline, with no runtime source edits:

| Check | Result |
| --- | --- |
| Fork HEAD, gitlink, status and recursive submodule verification | Exact requested revisions; clean before editing |
| `python3 crates/eez-control-rpc/conformance/check_vectors.py` | Exit 0; 29 nonempty frozen cases, fixture/schema hashes verified; **static**, not replay |
| Native workload census plus independent protoc syntax decoding | 111 frames in three complete captures; input totals above; **static** |
| `cargo +1.95.0 fmt --all --check` | Exit 0; no compilation |
| Documentation whitespace, JSON consistency and local links | See [validation record](validation.txt) |

The optional Python protobuf import probe failed with `ModuleNotFoundError`.
No dependency was installed; the census used standard-library traversal and
installed protoc instead. This was a tooling probe, not runtime validation.

Pending after adequate build resources are available, staggered before timing:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -- --test-threads=1
cargo +1.95.0 test --locked -p eez-composer --lib prover_retry:: -- --test-threads=1
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
```

Require nonzero selections: the signer package includes the 29 native TCP
transcripts, `finalize_deadline_preserves_prefix_and_allows_retry_in_the_same_epoch`,
native cold requests and worker lifecycle checks; client streaming tests include
deadline phases, restart, cancellation and backfill. Inspect ignored utilities
as exclusions; do not regenerate frozen oracles or use blanket `--ignored`.
Follow the signer contributor guide for its additional check/doc/Clippy commands.
The full release/Kurtosis matrix remains CI-owned.

For source process validation, follow the repository's source-build procedure,
clear prebuilt overrides, build integration contracts and retain owned logs and
datadirs. Select `cross_chain::` serially for signing/rejection/cold replay, and
`streaming::` separately for native shared-signer reorg recovery. Check the
selected tests are nonzero. Coordinate disjoint local ports and owned resources;
do not run against existing networks. These process suites, semantic assertions
and independent follower replay have **not run** in this investigation.

No Rust package tests, workspace Clippy, signer Rustdoc, native process tests or
fresh runtime workload measurements were started under the recorded storage
and competing-validation conditions. Therefore no test-binary hashes, actual
validation-identity handshake, timing distributions, baseline/candidate runtime
comparison, or fresh settlement/replay correctness result can be supplied.
The next step is adequate owned build capacity, completed correctness validation,
the real preparation/recovery measurement driver, then an eligible quiet window.
Keep #148 open and its measurement criterion pending until those results exist.
