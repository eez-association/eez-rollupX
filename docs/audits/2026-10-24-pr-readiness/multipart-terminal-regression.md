# Multipart fixture terminal

[CI run 40104687331](https://github.com/eez-association/eez-rollupX/actions/runs/40104687331),
PR head `319ebc2d`, failed
`expired_sources_use_recent_anchors_with_cold_offline_signers_and_public_replay`
before reaching its cold proofs. The fixture required each of 32 sequential
application receipts to land within block 2,000. Ordinary production had already
caught up: block 2,000 committed at 15:42:27.447 UTC, followed by blocks through
2,014. This test depended on its RPC writes outrunning the local block producer.

The failed receipt was above 2,000; its exact index, hash and number were not
printed. The last observed production height was 2,014. Artifact `12565941455`
preserves 130 log files, but no datadirs, so those missing receipt details cannot
be recovered from it. The original failure is retained independently of directed runs after correction.

The correction treats the configured genesis age as a minimum historical span.
After every application receipt succeeds, it fixes one terminal at or beyond
the last receipt, rounded to the existing five-block grid. Later production
does not move this endpoint. This keeps every application inside the proved
and independently replayed range without delaying production or discarding
transactions. To force the original race deterministically, the final application
is submitted only after the node has reached the minimum historical height,
and its successful receipt must be strictly later. The other 31 writes remain
concurrent with catch-up. Production continues throughout.

The first directed run passed **1/1 in 112.11 seconds**, using the initial
correction, the verified CI production binaries and network-denied signers.
It proved four parts, imported 401 headers and replayed all 32 applications;
the first terminal source was 304 L1 blocks old. It selected terminal 2,000,
with the last application at 1,759, so it did not validate extension past the
old boundary. The strengthened fixture then passed **1/1 in 123.27 seconds**. Its final
application landed in block **2,008**, and the single fixed terminal was
**2,010**. Five bounded parts (four historical and one actual-parent) imported
403 L1 headers; the first source was 304 L1 blocks old. All 32 applications
were included and independently replayed. Both runs used the same verified CI
production binaries and network-denied signers; only the test harness changed.
Neither pass replaces the original CI failure.

The 500-block proof cap, fresh network-denied signers, expired-source rejection,
complete ancestry bridge, rule transition, final actual-parent proof, canonical
settlement and all header/state/PREVRANDAO/application comparisons remain.
The short 120-block case still selects its original range; the optional four-day
case retains its configured minimum. The fixture now records the selected range
and every application inclusion height.

File formatting and `git diff --check` passed. The final build recompiled only
`eez-node-e2e` in 46.012 seconds. The optional four-day stress case was not rerun. Source hashes, first-failure identities and
evidence limits are recorded in
[multipart-terminal-regression.json](multipart-terminal-regression.json).
The retained original source, patch and raw CI logs are local-only under
`artifacts/pr-readiness/`.
