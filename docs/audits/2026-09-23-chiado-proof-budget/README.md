# Chiado proof-budget change and skipped-slot retry assessment

| Field | Value |
| --- | --- |
| Date and status | 2026-09-23 UTC; 1,000 ms proof budget applied, skipped-slot retry remains unimplemented |
| Scope | Existing Chiado rollup's timing configuration and retry feasibility |
| Source | Fork `1832f970884ca9460646a4cde87cca70eaffb3e1` with the configuration patch recorded by this checkout's diff |
| Protocol | Pinned `3c0cdf4955941a5c23639346932b55b33babebbd`, initialized without edits |
| Runtime | Node `sha256:1de4b3f90648f49b2cfe749c5a6a84c51ffffa6a8178dd26ef4294f58fa4ac7b`; signer `sha256:1fe82bae33fd602f436c714e9e33c1776a05b5ca8bd855845907008b71a24778` |
| Related records | [Fourfold performance run](../2026-09-23-chiado-staged-performance-4x/README.md), [retry draft](../../proposals/skipped-slot-speculative-retry.md) |

## Findings

The fourfold Chiado run recorded five `prover_deadline` and two
`preparation_deadline` outcomes during setup with the former 500 ms proof
budget. This change increases the configured proof allowance to 1,000 ms.
With preparation 500 ms and submission slack 700 ms, the five-second Chiado
schedule opens the trigger 2.8 seconds after an L1 anchor and reserves two
Future L2 blocks rather than one. This gives more time to prove but does not
show that those seven setup outcomes are eliminated under a comparable load.

The skipped-slot gap is separate. Static review found that
`eez-driver::spawn_l1_anchored` arms Sync work from an observed L1 head, while
`Composer::compose_sync_slot` emits a minimal postBatch when an unresolved
attempt still owns cross-chain users. The pinned RPC contract permits a later
range but only a previously committed **empty** Sync as an intermediate block.
The [retry draft](../../proposals/skipped-slot-speculative-retry.md) records the
required branch replacement and dual-candidate reconciliation. No retry code
has been deployed.

## Checks and operation

The configuration checkout's Chiado Compose default and environment example
were changed to 1,000 ms. The Compose expression passes an explicitly empty
value to Rust startup validation rather than silently replacing it. The
configuration test covers absent, explicit 1,000/1,500 ms, empty and invalid
values. `python3 -m unittest discover -s scripts/ci/tests -p
'test_chiado_config.py' -v` passed 10 tests;
`python3 scripts/check-settlement-policy-config.py` passed Chiado and
Kurtosis; `git diff --check` passed. These are configuration/static checks,
not a full process or live-load proof benchmark.

For the already running `rollupx` Compose project, the operator checkout's
private `.env` was edited in place from 500 to 1,000 ms. The node alone was
recreated with its existing local image and mounted datadirs using explicit
Chiado node and postings Compose files, `--no-deps --no-build --pull never`.
No contracts were redeployed, no wallet was funded, and the signer was not
restarted. Read-only inspection after restart showed `eez-node-chiado` running
since `2026-09-23T09:58:55Z` with `EEZ_PROOF_TIME_MS=1000` and the signer still
running. Before restart the queue was empty; an immediate post-restart L2 RPC
check showed `eth_syncing=false`, advancing latest/safe heads, empty queue,
and a canonical settlement response for the safe block. This is a short
health check, not a load or independent replay test.

Rollback of only this timing change is to restore the operator checkout's
former 500 ms value and recreate only the node with the same explicit Compose
files; it has not been exercised. A restart can discard speculative blocks and
in-memory pending requests, so clients may need to resubmit unsettled work.

## Open work

Implement and validate the retry draft before claiming the skipped-slot gap
fixed. Run a comparable performance test under the 1,000 ms budget to assess
the setup failure rate and any secondary throughput effect. No such comparison
has been run yet.

## Later design correction

The user subsequently selected a single active speculative branch with retained
submission history. The earlier assessment above overstated the need to keep
two branches active. The [replacement design](../../proposals/skipped-slot-speculative-retry.md)
now describes that approach; its implementation and validation are recorded in
the [replacement audit](../2026-09-23-timeout-replacement/README.md). This does
not change the earlier deployed proof-budget operation or validate newer code.

The later [timeout-replacement record](../2026-09-23-timeout-replacement/README.md)
supersedes the earlier unimplemented-retry status: the accepted approach was
implemented, tested and deployed at 12:53 UTC, retaining the one-second budget.

The later [posting-stage timing update](../2026-09-23-posting-stage-timing/README.md)
reduced the active proof budget to 750 ms at the user's request. The earlier
1,000 ms observations above remain historical evidence, not the current setting.
