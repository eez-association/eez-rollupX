# PR 56 integration and local validation

Status: the full local Kurtosis lane and final manifest-guard validation passed
on 2026-10-19. Hosted CI on the final PR head remains required. The user authorized the migration
and required local Kurtosis testing before publication. The historical baseline is
[CI run 39159493785](https://github.com/eez-association/eez-rollupX/actions/runs/39159493785):
33/34 protocol scenarios passed, but `staticReadWriteL2` prepared nonce zero
after setup had consumed nonces zero and one. The original nonce RPC responses
were not retained, so a stale read, speculative reorg or swallowed RPC failure
remains a possible cause. The deterministic regression proves the swallowed-error
path; it does not establish which cause occurred in that historical run.

| Identity | Revision |
| --- | --- |
| RollupX baseline | `e5e1d97ff49e69821ba403a9ae723ab4afa2db21` |
| PR 91 base / target branch | `2c19e36bed2720c00f89841ccfca47ec7c921e06` / `feat/compact-fee-quotes` |
| PR 56 base | `58a83b724308c1abb8672949fb877684a473aea0` |
| PR 56 source head | `bcd77d8b0d23ed9086f12ac8a0c2e65c6a2c2688` |
| Previous protocol gitlink | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Full-lane integration candidate | `f8d6eb0343d8d6616a42319581c03ae67d06a51b` |
| Full-lane protocol candidate, based on PR 56 | `c4b9885aa7e1a38cae0a2f6d718c022aa96c821c` |
| Final runner implementation / protocol | `d6d4ef9fc29c3c9648c43b76944f76e8b32a0c29` / `0264ab1be8986093ad786782e60a28f14103a0fd` |
| Runtime source (unchanged Rust) | `110f952143853d0edbc51d6d8c2cb67be4d828f3`, tree identical to baseline |
| Runtime protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Local enclave | `eez-pr56-validation-20261019` |

## Changes and evidence

The protocol runner retains isolated Foundry deployment broadcasts and checks
original receipt/header identities, deployed code and nonce floors before
preparing a trigger. Failed nonce queries no longer become zero. A rollback
between checking setup and signing cannot make the trigger replace its own
prerequisites. The prepared nonce is included in fee quotes; typed option errors
retain their diagnostics and cannot authorize signing or node-fee fallback.
Same-account bursts retain consecutive nonces without waiting for each trigger.

RollupX consumes PR 56's table-load/delivery labels and all L1 posting summary
rows. Those are discovery hints: complete canonical transactions still undergo
independent calldata, causal/event and blob checks. The static L2 collector uses
the current receipt instead of the early optimistic block number and no longer
requires a range-scan message when exact transaction links supply the settlement.
Its actual table, receipt and blob assertions remain required.

Before fixing the fee client, the new tests reproduced the missing nonce and
lost typed diagnostic; a malformed error attached to an otherwise valid option
also incorrectly allowed signing. Before changing the collector, the new labels
and multiple posting summaries failed its actual script. The baseline nonce
failure probe and original CI diagnostics remain local-only in the review
checkout under `docs/audits/2026-10-19-protocol-pr56-readiness`. Additional new preparation
checks cover missing receipts, changed block hashes, absent code, nonce floors,
worker isolation and the preserved burst sequence. The same signing regression
was also run with the original PR 56 libraries: it signed nonce zero despite
retained setup at nonce one. The corrected runner rejects that attempt before
signing. This is a controlled regression, not proof of the historical RPC failure.

Executed checks:

- Protocol Python: 176 distinct tests passed across scoped invocations (98
  network tests, including eight preparation tests; 78 other runner tests).
  The original network invocation ran 95 tests; the additional preparation
  regressions passed in later focused runs.
- Protocol Solidity: 778 passed, three fork tests skipped. The documented
  `script/*`, `test_Doc_OriginalValue_SeedInSetup` and
  `test_PostCost_Incremental` exclusions were retained. The skipped tests were
  `testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`, and
  `testForkRejectsWrongSigningKey`.
- Actual collector regressions: nine test methods passed, including the new
  role labels and split-settlement cases; static/lookup suite: 22 passed.
- Genesis consistency and changed shell syntax/whitespace checks passed.
- SDK fee tests: six passed.
- RollupX tooling: 374 selected, 366 passed, eight skipped. Six are opt-in
  infrastructure probes (Docker payload/runtime, Kurtosis resources and native
  archive relocation); two require unavailable Gitleaks. These are exclusions,
  not covered by the passing exit status.
- All 62 protocol Anvil scenarios passed once, including the explicit-only
  scenarios that remain excluded from the network suite. See
  [per-scenario results](protocol-anvil.json).
- The full local Kurtosis driver passed: 34/34 enabled protocol scenarios,
  complete result accounting, independent causal verification and authenticated
  semantic blobs for all 34. All four wave modes passed (40 cross-chain
  operations, including mixed ordinary traffic), as did state chaining,
  safe-head convergence and the proof path (51 signed windows and 51 remote
  attestations, zero root divergence). See the [driver result](kurtosis-result.json),
  [protocol results and exclusions](kurtosis-protocol.json), and
  [phase timings](kurtosis-timings.json). `staticReadWriteL2` used nonce two after
  its two setup transactions and passed both upstream and independent checks.

The live suite still excludes 28 of the 62 discovered scenarios: 22 PR 54 call
patterns, four local-static cases and two missing-proxy cases. Their Anvil passes
do not qualify them for live execution. The local network run had no failed
scenario or diagnostic retry. Relay registration initially waited for the
validator cache and recovered within the existing readiness timeout, without
manual service intervention.

## Runtime and operational boundary

No Rust, production Solidity, dependency pin or genesis content changed.
Protocol `foundry.toml` adds verifier file-read permissions only. This local run
reuses the baseline's verified CI node/signer/tool bytes and builds a new deploy
image with the candidate's protocol and tooling. It does not claim that runtime
binaries were rebuilt from the candidate root commit. The archive SHA-256 is
`20fbcc55d6ed4694041c1ac2e7c1b7b5cc20f82bc48a3df5212412cb67082ac4`;
all seven extracted binaries were compared with that verified archive.
Immutable image and executable identities are in
[runtime provenance](runtime-provenance.json). The final guard changes only a
host-side shell reader; it uses the same retained network and deploy image.

The local driver is a copy of `testing/kurtosis/run-ci.sh` with only its source
path and cleanup changed: it retains the enclave on exit for diagnosis. Test
assertions and scenario inventory are unchanged. `EEZ_PROTOCOL_MAX_PARALLEL=2`
matches hosted CI. Anvil uses the separately checked, unused port range starting
at 28545. Existing enclaves and the operator's Chiado stack are untouched.

Full logs and the retained driver are local-only under
`artifacts/pr56-local-validation` in the isolated RollupX checkout. Other test
logs are under `/tmp/rollupx-pr56-readiness-20261019`; these paths have no durable
retention guarantee and are not available in another checkout. The checked-in
results are sanitized summaries; [log digests](local-check-logs.json) identify
the local full records. No public-network workload, new runtime build, Falcon
test or public deployment is claimed by this migration.

The standard CI enclave has no fee policy configured: the migrated client
requests quotes and takes its documented node-fee fallback on `quotes_disabled`.
Thus this live run qualifies execution, setup and correlation, not enabled-fee
funding. Prepared quote nonce and typed-error behavior are covered by the Python
regressions. Fresh workspace Rust builds, Clippy, native process suites and
standalone follower replay were not repeated for these shell/collector changes;
the verified runtime bytes are unchanged. The local collector still requires
native execution-prefix replay for applicable lookups. Remote CI must build and
validate the final PR head before merge.

## Final manifest-read regression

Protocol commit `0264ab1be8986093ad786782e60a28f14103a0fd` adds a focused
follow-up: a missing or malformed setup file must fail instead of becoming an
empty nonce selection through Bash process substitution. Both baseline cases
reproduced against `c4b9885`; all eight preparation tests pass with the fix.
After the full lane completed, RollupX pinned this guard in `d6d4ef9`. Its three
affected live scenarios (`bridge`, `counter-multi-tx`, `staticReadWriteL2`) all
passed, including independent causal and semantic-blob verification; the eight
preparation tests also passed in that checkout. See the
[final-candidate results](final-guard-kurtosis.json). This was a focused
follow-up, not a second full 34-scenario run. Total unique protocol runner
regressions passed: 176.

## Executed entry points

Commands ran from the isolated RollupX root unless a protocol root is specified.
The local driver and its environment are identified in
[runtime provenance](runtime-provenance.json); its node/signer/tool artifacts
were verified before reuse. Follow the [Kurtosis guide](../../../testing/kurtosis/README.md)
for artifact setup and environment requirements. The full run used the copied
driver solely to retain its owned enclave after exit:

```bash
BASE_PORT=28545 BASE_CHAIN_ID=51337 bash scripts/ci/run-protocol-anvil.sh
export KURTOSIS_ENCLAVE=eez-pr56-validation-20261019
export EEZ_PROTOCOL_MAX_PARALLEL=2
bash artifacts/pr56-local-validation/run-ci-retain.sh
```

The full lane selected its default `all` inventory. For the final guard, the
same enclave and verified tools were retained, `EEZ_CI_RESULT_DIR` was changed
to a separate `artifacts/pr56-local-validation/final-guard` directory, and the
protocol entry point was invoked with:

```bash
export KURTOSIS_ENCLAVE=eez-pr56-validation-20261019
export EEZ_PROTOCOL_SCENARIOS='bridge counter-multi-tx staticReadWriteL2'
export EEZ_PROTOCOL_MAX_PARALLEL=2
bash testing/kurtosis/scripts/run-protocol-e2e.sh
```
