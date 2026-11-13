# Completed bounded streaming workload timings

The actual [#148](https://github.com/eez-association/eez-rollupX/issues/148) campaign completed on one GitHub-hosted Linux VM under the unchanged predeclared quiet-window criteria, with the pressure observations disclosed below. [Run 43349501992](https://github.com/eez-association/eez-rollupX/actions/runs/43349501992) passed all 13 normal checks and the additional measurement job. All 276 attempts succeeded: 36 planned warmups and 240 measured results, 20 per case in all 12 cases. No new runtime defect or performance improvement was established. This is not an SLA or live-L1 latency result.

The [compact JSON record](timing-results.json) retains every stage and operation distribution, exact charges, source/artifact identities, validation and limits. [Method](followup.md) and [frozen workload census](workloads.json) remain authoritative for the experiment. The initial resource failures and original N14 input-provenance limitation are preserved.

## Actual sources and environment

Hosted runtime/test bytes were built at `cd612739e5e519100b06e233d80193b993cf3276`, protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`, in producing run 43349501992 / attempt 1. The same-run consumer used immutable tests payload `sha256:0cda798c1c59daa9437d5643d0aec64d715428fc2da2b37be15565223e81de24`; test SHA-256 `764855f11a8731c3d7999ebf3fb29533168abb62a462347b6968933338c98796`, signer `0aefba198e48e2e114ad1ca8fa68f459b4bfba4d48c07074b81d76eecabd4866`. Input hashes were unchanged afterwards. The observer has the corrected 04bbb115 code; its checkout was CD.

These are separate from the retained local `ca76bb87` glibc 2.43 binaries and from the final feature/main commits. The final feature locally merges reviewed repair #168 and tested main c30a7fff; main adds accepted L1 blob-reader/RPC-spec/test changes. Relevant signer/client/RPC-code/testkit/streaming/Cargo/protocol inputs match the measured source. The whole crates tree does not. No native bytes are relabelled after documentation or integration changes.

The observed guest was Ubuntu 24.04 image 20261111.370.1, Linux 7.0.0-1017-azure, 4 CPUs in both affinity and host count, Python 3.12.3. The producer glibc was 2.36, consumer 2.39; Rust 1.95.0, nextest 0.9.143, default shipping release features and `-C link-arg=-fuse-ld=mold`. Cargo release settings are recorded verbatim in JSON. No affinity override, service/swap change, fixture replacement or consumer rebuild was used. This was not an isolated physical host; the CPU model was not recorded. Different local/hosted environments are not an improvement comparison.

## Quiet eligibility and retained evidence

All 97 full snapshots have no rejected reason: 13 before, 72 during, 12 after, with additional 1-second process probes. The clean pre-period was 121.18 s; the full post period 121.09 s. Across the 96 observed CPU intervals, minimum CPU idle 75.6877%, maximum I/O wait 1.811%, maximum load 1.3076 (limit 2), minimum available memory 14.269 GiB, zero swap traffic, no heavy detections, and at least 153 visible processes. Minimum free space was 84.235 GiB on root/tmp/evidence and 7.805 GiB on shared memory. Every unchanged declared criterion passed.

CPU/IO PSI was recorded, not gated; memory PSI was gated and passed. I/O PSI some avg10 began at 21.71, fell to 13.28 and 7.52, then stayed around 5.06–5.75 through 03:22:38 UTC. The cumulative I/O PSI some stall counter grew 5,950,021 microseconds over the 121.18 s baseline and was nearly flat only in its final interval (73 microseconds). This was sustained pressure, not just a first-snapshot transient, despite the gated interval I/O-wait maximum of 1.811%.

During sampling at 03:25:30 UTC, CPU PSI some avg10 reached 18.19; its cumulative stall counter increased 2,870,677 microseconds over 10.104 s. The same interval had 76.152% CPU idle, no heavy detection and no rejected gate; the next two CPU PSI avg10 values were 7.59 and 2.79. Causes were not isolated. Per-attempt absolute timestamps were not recorded, so this spike is not assigned to particular repetitions. The run is eligible under its unchanged predeclared criteria, not an assertion of zero pressure or an uncontended physical host. No new PSI threshold is introduced after observing the data, and no normal service was stopped.

[Artifact 14008340286](https://github.com/eez-association/eez-rollupX/actions/runs/43349501992/artifacts/14008340286) ZIP SHA-256 `7ee4b473c5e6a79bf686488da0f97e5aee6fa6f632848ef2947355bcd101099b` matches GitHub metadata. The archive/raw files were downloaded and retained in owned evidence storage; per-file hashes are in JSON. GitHub retention is 3 days (expiry recorded there). Raw attempts/logs/binaries are not committed. Independent coordinator raw-record/hash/resource/distribution checks agree. Neither record audit separately reran native execution or decoded proof crypto; those assertions remain source-bound process evidence.

## Observed distributions

Values below are milliseconds, 20 measured samples per row. “Complete” starts at capability negotiation **after initial signer startup/readiness**. It includes capability negotiation, preparation, any owned restart, prove/verification/EOF and the pre-proof signer-log check. The restart interval includes the old signer's blocking drop, new spawn/readiness and proxy upstream update. Proxy creation is in no recorded interval. Later assertions, trace extraction, final log read and teardown are outside Complete. Capability alone contributes a roughly 41 ms median, now shown explicitly. Medians of separate intervals need not sum to the median of Complete; the log read and small gaps are also outside individual stages. Fixture loading and initial readiness are separate distributions in JSON. Initial readiness medians were 1827.004–1930.011 ms; lost-session restart medians 1938.515–1939.408 ms, included in Complete. Do not read the cold Complete column as process-start-to-finish latency.

| Fixture | Case | Capability median | Prep median | Restart interval median | Prove/verify/EOF median | Complete min | Complete median | Complete p95 | Complete max | Complete IQR |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Bidirectional | cold | 40.733 | 0.000 | 0.000 | 96.979 | 97.691 | 137.757 | 139.475 | 139.691 | 2.074 |
| Bidirectional | prepared | 40.911 | 89.003 | 0.000 | 90.767 | 179.299 | 220.811 | 224.148 | 229.749 | 41.957 |
| Bidirectional | disconnected | 41.144 | 85.021 | 0.000 | 92.961 | 179.335 | 219.349 | 221.495 | 221.586 | 2.335 |
| Bidirectional | lost_session | 41.070 | 85.050 | 1938.515 | 179.037 | 2142.237 | 2243.160 | 2246.246 | 2246.414 | 40.471 |
| Lookup | cold | 40.909 | 0.000 | 0.000 | 108.752 | 108.354 | 149.562 | 151.256 | 152.585 | 2.373 |
| Lookup | prepared | 40.858 | 90.995 | 0.000 | 90.942 | 181.398 | 222.301 | 224.437 | 227.208 | 1.107 |
| Lookup | disconnected | 40.743 | 84.230 | 0.000 | 135.079 | 219.471 | 260.160 | 261.824 | 261.947 | 40.635 |
| Lookup | lost_session | 40.805 | 85.015 | 1939.408 | 191.088 | 2214.015 | 2256.140 | 2261.989 | 2265.265 | 40.311 |
| Changing-static | cold | 40.838 | 0.000 | 0.000 | 97.041 | 96.792 | 137.923 | 138.449 | 138.506 | 0.333 |
| Changing-static | prepared | 40.780 | 88.109 | 0.000 | 90.020 | 178.459 | 219.385 | 220.707 | 222.268 | 1.171 |
| Changing-static | disconnected | 40.652 | 85.011 | 0.000 | 92.022 | 177.445 | 217.802 | 219.222 | 234.069 | 39.694 |
| Changing-static | lost_session | 40.884 | 85.009 | 1938.767 | 178.518 | 2202.616 | 2243.033 | 2246.326 | 2246.950 | 2.398 |

Nearest-rank p95 is the 19th of 20 ordered values; min/max and IQR show the small-sample variability. Capability timing is bimodal in all twelve cases: per-case minima 0.744–0.833 ms and medians 40.652–41.144 ms. Complete's roughly 40 ms bands track that stage repetition by repetition in reviewed groups. Lookup/disconnected additionally has one 94.656 ms prove/verify/EOF result and nineteen 134.696–137.612 ms results. Bidirectional/lost-session restart has three 1837.385–1839.177 ms results and seventeen 1936.719–1940.294 ms results, coinciding with a roughly 100 ms Complete band. These identify stage locations, not root causes. A low IQR does not imply one mode: its rank 5/rank 15 may land in different bands as fast-mode counts change. No outliers or failures were removed; only the three planned warmups per case are excluded from these distributions. Operation-level intervals in JSON include proxy queueing and are correlated within each repetition, not independent population-tail estimates.

## Work and semantic assertions

The corpus contains 30 / 45 / 30 blocks and 999 / 1649 / 1024 witness items, with 208,965 / 321,084 / 211,321 witness-data bytes. The original captures are 372,390 / 495,605 / 375,906 protobuf-payload bytes. Actual client-transmitted protobuf bytes and header charges vary with controls/replay; the table includes discarded preparation and full backfill. These bytes exclude gRPC/TCP framing and server responses.

| Fixture | Case | Block transmissions | Client protobuf bytes | L1 headers | Header bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| Bidirectional | cold | 30 | 373506 | 7 | 4387 |
| Bidirectional | prepared | 31 | 399278 | 7 | 4387 |
| Bidirectional | disconnected | 30 | 373544 | 7 | 4387 |
| Bidirectional | lost_session | 45 | 513082 | 11 | 6887 |
| Lookup | cold | 45 | 497261 | 10 | 6277 |
| Lookup | prepared | 46 | 540825 | 10 | 6277 |
| Lookup | disconnected | 45 | 497299 | 10 | 6277 |
| Lookup | lost_session | 60 | 636837 | 14 | 8777 |
| Changing-static | cold | 30 | 377022 | 7 | 4387 |
| Changing-static | prepared | 31 | 405510 | 7 | 4387 |
| Changing-static | disconnected | 30 | 377060 | 7 | 4387 |
| Changing-static | lost_session | 45 | 516598 | 11 | 6887 |

All 276 repetitions passed the native process assertions: ACK identities/roots, expected digest/public attester, one signature/Proof/EOF, prevalidation never signing, scenario controls and expected NOT_FOUND. Independent record checks confirm exact block/transport sequences, per-block header replay charges, byte totals, capabilities/config/request hashes and every scalar/operation distribution. This is not separate signer-side quota accounting or independent crypto execution.

Preparation is real client serialization/streaming/signer work over captured witnesses, not Composer watermark work or fresh witness collection. Disconnection is ordinary prevalidation EOF after 15 ACKs, not midflight/lost-ACK recovery. Lost-session restarts only the owned signer behind the same proxy, not the client channel. Expected identity uses the shared identity library and local configuration; the frozen expected digest comes from response.json. Finalize equivalence is by digest, not bitwise capture equality.

The fixed order, unflushed OS caches, fresh signer per repetition, 100 ms readiness polling and instrumented loopback hop constrain interpretation. Separate controlled tests cover worker permits, cancellation, retries and deadline boundaries. The utility's 3-minute attempt and 15-minute group caps differ from the 600,000 ms request and 120,000 ms idle policy. These quick successful samples neither introduce a deadline guarantee nor reproduce the already fixed #43/#45 defect.

## Rejected local windows retained

The local controller stopped gracefully after window 12 ended. All twelve windows, including the single contaminated launch in window 03, remain separately retained with raw hashes in JSON. No local timing is reported or pooled with hosted results. Ordinary background services and other tasks were not stopped. Some windows contain this task’s own marked preparation or evidence reading, as shown below.

| Window | Outcome | Starts | Completed warmups | Completed measured | Longest clean admission (s) | Own preparation |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| quiet-window-01 | admission unavailable | 0 | 0 | 0 | 30.55 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-02 | admission unavailable | 0 | 0 | 0 | 81.46 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-03 | contaminated | 4 | 3 | 0 | 122.17 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-04 | admission unavailable | 0 | 0 | 0 | 91.60 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-05 | admission unavailable | 0 | 0 | 0 | 81.42 | Owned full tooling validation in part of the window. |
| quiet-window-06 | admission unavailable | 0 | 0 | 0 | 61.10 | Brief owned frozen export preparation. |
| quiet-window-07 | admission unavailable | 0 | 0 | 0 | 40.72 | Owned full tooling and controlled CI regressions in part of the window. |
| quiet-window-08 | admission unavailable | 0 | 0 | 0 | 91.55 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-09 | admission unavailable | 0 | 0 | 0 | 50.91 | No owned heavy validation/evidence-reading marker recorded by this task in this window. |
| quiet-window-10 | admission unavailable | 0 | 0 | 0 | 81.45 | Brief owned finished-producer evidence retrieval. |
| quiet-window-11 | admission unavailable | 0 | 0 | 0 | 71.27 | Brief owned finished-cross-chain evidence retrieval. |
| quiet-window-12 | admission unavailable | 0 | 0 | 0 | 71.23 | Owned accepted-cloud download/record verification; graceful stop requested after validation. |

The original quota/sandbox and setup failures remain historical environmental/setup evidence, not reproduced deadline defects. Zero-start driver logs are empty; their SHA-256 values are empty-input digests. No per-failed-window source commit was pushed. At this record's publication boundary, read-only PR/main CI tracking continues at thirty-minute cadence until the authorized merge/release goal is resolved.

## Validation and remaining limits

Fresh hosted evidence includes 70 producer release tests (zero skipped), 607 tooling selections (600 passed, 7 explicit opt-ins skipped; both scanner tests passed), 161 cross-chain tests across 4 nonzero shards including the ordinary twelve-case test, all 4 embedded partitions, debug units, Kurtosis and the aggregate. No native build or settled source review was repeated for this record. Earlier failures/skips remain in [historical local validation](followup-validation.json) and [hosted source validation](hosted-validation.json).

The original frozen N14 input provenance remains unverified. No production traffic, live L1 posting, durable queue recovery, saturation capacity or V1 capacity was measured. Optional signal-cleanup/static-profile-label caveats remain; actual seal/hash provenance is retained. No new runtime/config/deadline change is supported by this characterization.

The final bounded Claude Fable 5.2 evidence review and correction verification passed with no blocking method/evidence issue. Both actual model metadata records identify only `claude-fable-5-2`, with no fallback or delegation. The review used operator copies and did not independently hash, rerun native execution or decode proof crypto. Required doc-only corrections disclosed the pressure observations and stage bands; no measurement, gate or runtime changed. Review hashes/scopes are recorded in JSON.

Fresh exact-final-feature-head normal CI remains required before #165 is merged. Normal optional measurement skip is allowed; no required lane may skip. Final current-main CI must then pass and promote only its tested node/signer/deploy digests, with anonymous source/digest verification. #148 remains open; no version release/tag or deployment is authorized by these results.
