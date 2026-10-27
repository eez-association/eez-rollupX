# Final runtime and replay artifact verification

Tracking [CI run 40104687331](https://github.com/eez-association/eez-rollupX/actions/runs/40104687331)
for PR head `319ebc2dbfdace3d5e708a2c9881cc741529aec4`. The producer passed:
shipping compilation took **17m07s**, release-test archiving **11m08s**, and its
**31 selected tests passed** in 1.288 seconds. The archive dependency guard
passed. Both published payloads were verified at 15:33:55 UTC and then handed
to the live-validation agent. This verifier installed nothing into the network.

The exact synthetic checkout is `08e163331c4935d3d4e8c1b657df87812ba50862`,
with parents `a50c326d99f7cd1c21406eedbf370a344015a6b5` and the PR head. Its tree,
`91e8b0b1914dfe290195bc9cd30a4f126c1af0fb`, equals the PR head's tree. A new
detached worktree under `artifacts/pr-readiness/ci-candidate/40104687331/checkout`
has the recursively initialized protocol pin
`84a5b5207880f192a6a5896cbfe0bac5249db47f` and a clean initial status.
The Actions workflow references and completed viewer checkout log independently
identify the same synthetic revision.

Consumption used `ci-payload.sh` and `prepare-runtime-artifacts.sh` from that
checkout. Runtime and tests came from this run's immutable payload digests.
Source, protocol, lockfile, platform, profile, archive and image identity checks
passed. The existing native verifier checked all **30 inventoried test files**;
the test manifest's runtime archive digest and three frozen runtime hashes equal
the seven-binary runtime package. The host satisfies the producer's glibc floor.
The checkout stayed clean. Extraction containers never executed or published
ports; no compilation or node lifecycle operation occurred.

The producer selected six protocol/example binaries. Its one excluded binary,
`node_e2e`, runs in the downstream process lanes; the ignored Kurtosis replay
requires separate explicit live execution. All eight Embedded/Cross-Chain
partitions completed, and the Kurtosis job passed. Debug passed **1,569/1,569**
with three explicit utility/profile skips; its separate Reth shutdown regression
passed **1/1**, with 206 tests outside that selection.

Embedded completed **42 passes and one failure across 43 selected tests**.
`fullness_overshoot_posts_all_fitting_blobs_without_waiting_for_other_rules`
passed its two-blob and follower checks, then rejected an observed
`historical_chunk` event. The event belongs to that same target batch and records
only `retained_ordinary`, with zero capacity rejections. The current sequencer
allows that path to settle an already committed checkpoint while L2 continues;
the test's blanket prohibition conflates it with capacity splitting. The first
failure remains preserved. The reviewed correction and three focused passes
are recorded separately in [fullness-regression.md](fullness-regression.md);
those runs do not rewrite this CI result or newly cover `historical_chunk`. CI preserved 51 log files, but no
datadirs, in artifact `12565298606`.

Cross-Chain completed **130 passes and one failure across 131 selected tests**.
The multipart expired-source fixture asserted that all sequential application
writes landed before a fixed height that local production had already passed.
That failure preceded its cold proofs; it does not count as proof/replay
coverage. See [the diagnosis and correction](multipart-terminal-regression.md).
Artifact `12565941455` retains 130 logs, without datadirs.

The eight partitions cover **174 unique tests: 172 passes and two failures**.
Their event inventories match the verified binary's `--list` output, with no
duplicates or unexplained omissions. The other five tests are explicitly
ignored: three cold-history checkpoints, the optional four-day stress run and
the live Kurtosis replay. Inventory commands executed no tests or services.
Later focused corrections do not rewrite these first-run results.

Kurtosis CI passed at **16:32:57 UTC** with the verified candidate images.
All four wave modes and the state-chaining checks passed. The standard protocol
selection passed **34/34**, including its independent receipt, causal and semantic
blob checks. The exact 34 result names match all eligible scenarios at the pin;
**30 of 64 discovered scenarios were excluded by upstream annotations**, with
names and reasons in the JSON record. The two additional local static scenarios
are separate coverage. Cold Solidity builds took 457 and 50 seconds, network
startup 173 seconds, waves/state chaining 1,152 seconds and the protocol phase
1,425 seconds, including 55.883 seconds for independent verification.

This successful lane publishes no diagnostic bundle under the current workflow.
Its job log is retained, but its individual receipt/table bundles and full service
logs are unavailable for further inspection. Its standard-suite pass does not
erase the separate live `revertCounter` helper failure on another execution path,
nor substitute for the live track's cold follower replay.

Lint failed at dependency policy on the inherited `RUSTSEC-2026-0386` and
`RUSTSEC-2026-0387` advisories. Its later checks were skipped; the producer and
debug-unit jobs passed independently. Artifact verification does not clear
that integration gate. The aggregate CI gate failed, and no tags were promoted.

The previous [run 40102561259](https://github.com/eez-association/eez-rollupX/actions/runs/40102561259)
was cancelled by this essential test correction: the diagnostic replay had
reached a native `0x45` receipt that its Ethereum-only receipt decoder could not
decode. Shipping had passed in **24m57s**; archive compilation had just begun.
The dependency guard, producer tests and payload publication were not reached.
Debug was cancelled, viewer passed, and lint failed the inherited dependency
advisories. This cancellation is not a newly observed producer failure. No
payload from that run was consumed. The corrected diagnostic replay and the
required final replay remain distinct evidence.

Exact identities, completed gates, hashes and limits are maintained in
[ci-runtime-consumption.json](ci-runtime-consumption.json). Previous failures
remain in [ci-runtime-archive.md](ci-runtime-archive.md),
[ci-final-runtime.md](ci-final-runtime.md), and
[ci-prefix-fixture.md](ci-prefix-fixture.md). Raw logs and downloaded artifacts
are local-only. The handoff records image identity, while image/archive byte
comparisons, genesis/profile comparisons and final replay belong to the live
track. No final replay or promotion is claimed by this verifier.
