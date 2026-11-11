# Fitting batches and retained ordinary settlement

CI run [40104687331](https://github.com/eez-association/eez-rollupX/actions/runs/40104687331)
failed the fullness test after its two-blob post and follower convergence had
succeeded. The test rejected every `historical_chunk` event. The runtime also
uses that path for a complete retained ordinary range: the recorded event covered
blocks 1–40, had only `retained_ordinary` as its reason and no capacity rejection.

The test now permits that reason only with zero capacity rejection. The three
fitting-range tests retain their canonical range and single-post assertions.
The fullness case additionally checks the exact posted root update and successful
execution event, inclusion of the retained checkpoint, both successful application
receipts on the follower, and `blob_fullness` as the sole scheduling trigger.
No production code or settlement policy changed.

An independent review of the final source found no further defect. Workspace
format and Clippy passed; the target build recompiled only `eez-node-e2e`.
Three focused process runs passed with the unchanged, verified CI production
binaries and outgoing signer connections denied:

| Case | Result | Test duration |
| --- | --- | --- |
| Fullness overshoot, two fitting blobs | 1 passed, 178 filtered | 54.27 s |
| More than six Prague blobs | 1 passed, 178 filtered | 55.01 s |
| More than 300 blocks | 1 passed, 178 filtered | 325.20 s |

None of these three focused runs emitted `historical_chunk`; they validate the
stronger canonical settlement/replay assertions but do not supply new live
coverage of that internal path. Its original CI event and the source review are
separate evidence. The first CI failure and the initial local Clippy type error
remain recorded. Commands, executable/source identities, memory minima and log
digests are in [the portable record](fullness-regression.json); raw logs and
retained datadirs are local-only under `artifacts/pr-readiness/fullness-regression/`.
