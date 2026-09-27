# Upstream Reth #27429 candidate

Status: validation in progress for draft [PR #41](https://github.com/eduadiez/rollupX/pull/41).

The user authorized continuing with the upstream fix commit. Reth is pinned to
`444e867155c2a7a4c028e479ca7201b0b6871579`, the merge of
[#27429](https://github.com/paradigmxyz/reth/pull/27429), with no local Reth patch.
Compared with v2.6.0, it contains 78 main-branch commits and excludes only the
release branch's version-bump commit. Its Cargo package version remains 2.5.2;
the SHA identifies the tested source. This is not a claim of a released v2.6.1.

The EEZ baseline is PR head `69af59f1e6dd03829f84fe7f98768e6c10b09ff0`, based on
main `3346d966f3c8611be790db484abc63b3b259d367`. Protocol gitlink
`9d13218a355e7d543ca8a9927eea2962600b716d` is unchanged. Operator work, services,
wallets and databases remain untouched. Fresh networks remain the scope.

## Adaptations

Reth core crates move to 0.8.1 and Alloy to 2.5.0; REVM remains 43.0.3 in the
root lockfile. Upstream separates EVM reads from full state providers. EEZ and
Gnosis use its transparent EVM adapter while retaining full providers for roots
and proofs. A two-method snapshot interface erases Ethereum/Gnosis primitive
types for shared simulations; the Gnosis adapter forwards the new executed-block
state method with the original Gnosis primitives. No chain rules, signer gates,
protocol commitments or production persistence defaults change.

## Build and unit evidence

The shipping source is `4e9486cf859d38f7348dbca192d552a3ddb97aac`. The later
`d5f92cb` commit changes only the crash fixture. Seven release ELF files were
frozen after building the production package/feature selection; all seven were
hashed again inside the node, signer and deploy images. See [candidate.json](candidate.json)
and the [dependency delta](dependency-diff.json). These are local builds, not
proof that the complete GitHub release-artifact gate has passed.

| Check | Result |
| --- | --- |
| Locked workspace check, all targets | Passed |
| Workspace Clippy, all targets, warnings denied | Passed |
| Rust formatting and vendor patch reverse-application checks | Passed |
| Workspace libraries, signer binary and four independent vector targets | 1,195 passed, 3 ignored |
| Standalone Stateless and tries libraries | 8 passed |
| Stateless transaction checkpoints against a full-state root oracle | [1 passed](stateless-checkpoints.json) |
| Exact upstream engine library | 205 passed, including both #27429 regressions |
| Exact upstream forkchoice RPC tests | 3 passed, including persisted-head restoration and invalid-state handling |
| Cargo deny advisories/bans/sources | Passed with unmatched-source exception warnings |
| Cargo audit | Passed with 9 allowed warnings |
| Python tooling | 261 passed, 4 opt-in tests skipped |

The three Rust ignores are the manual held-pool profile, native proving-fixture
inventory, and cold-span feasibility utility. The Python opt-ins are data-only
Docker payload, real Kurtosis relay resource probe, native archive relocation,
and runtime Docker fixture. These are exclusions, not executed coverage. The
previous revision's explicit Docker fixture result remains historical evidence.

Commands used Rust 1.95.0, Foundry 1.7.1, `--locked`, the initialized protocol
and separate local diagnostic directories. Shipping compilation used the retained
bookworm tools container, `RUST_MIN_STACK=16777216`, six Cargo jobs and mold:

```bash
cargo build --locked --profile release \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --bin eez-composer --bin eez-follower --bin eez-proof-signer \
  --example genesis_state_root --example genesis_block_hash \
  --example e2e-causal-verify --example verify_blob_evidence
cargo +1.95.0 test --locked --workspace --lib --bin eez-proof-signer \
  --test public_inputs_hash_vectors --test rolling_hash_vectors \
  --test causal_evidence --test ethereum_kzg_compatibility
cargo +1.95.0 test --locked --manifest-path vendor/stateless/Cargo.toml \
  -p stateless -p tries --lib
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 fmt --all --check
cargo deny check advisories bans sources
cargo audit
python3 -m unittest discover -s scripts/ci/tests -v
```

The first shipping-build invocation used a login shell that lost Cargo's PATH;
it failed before compilation. The corrected non-login invocation passed. The
first native test-build invocation named a nonexistent `resumption` target;
the corrected selection compiled the six intended test executables. Neither
invocation failure is counted as runtime evidence.

## Crash comparison

A first attempt to force an intermediate L2 prefix used a graceful setup restart.
That also stopped the isolated L1 before its recent proxy deployments persisted.
The baseline failed before reaching the intended crash; no disk-frontier event
was emitted. Adding the L1 disk barrier and asserting that the source proxy still
had code corrected the setup. Both v2.6.0 and the new shipping candidate then
passed the identical test, with L2 block/state frontiers **31/31** and settlement
height **46**. Receipt, nonce, request accounting, resubmission and follower
assertions were retained. The [comparison](graceful-prefix-comparison.json)
records both executable identities and the original setup failure.

That comparison does not reproduce the earlier failure from block 16 and does
not establish that #27429 fixed it. A graceful flush makes block and state
frontiers equal. The next fixture instead waits for normal upstream persistence
to leave a nonzero state frontier below both the block frontier and the
settlement endpoint. Persisted block bodies may already reach the endpoint;
Reth still needs to rebuild the missing state. An initial overly restrictive
fixture predicate required the block frontier below the endpoint too and timed
out before SIGKILL on both versions; these are setup failures, not recovery
results. The corrected pair retains the same state-frontier requirement. It reads
a fresh read-only MDBX transaction in a blocking worker, closes it before SIGKILL,
and records both frontiers again after death
and before reopening. Production persistence defaults and validation guards
remain unchanged.

The corrected shipping comparison passed on both versions using the same frozen
test executable. Both recorded block/state frontiers **46/16**, reopened at
**16**, and reconstructed the partial settlement through **45**. The original
nonce, receipt, accounting, resubmission and fresh-follower assertions passed.
See [masked-prefix-comparison.json](masked-prefix-comparison.json). This confirms
that the shipping candidate can recover from masked persisted state. It does
not attribute the earlier optimized-build failure (16 to 46) to #27429.

On September 27, the same frozen test also passed with the original v2.6.0
optimized host executables from the failing run and with newly built candidate
host executables. Both again persisted **46/16**, reopened at **16**, and replayed
through **45**. Their identities and results are in the same comparison file.
Thus the controlled regression passes in both build profiles; the historical
16-to-46 failure remains recorded and unattributed. This is not evidence that
changing the build profile or applying #27429 caused that failure to disappear.

Other local candidate process checks passed: four non-crash partial-consumption
cases, one queue-reorg case, four settlement gas cases (including two Gnosis
cases), and 24 cross-chain cases. Including the corrected crash case, this is
34 distinct local native process cases. The separate 31-case settlement-policy
invocation was interrupted after one passing case and is not counted as a
completed suite; its full coverage comes from the CI shards below.
See [native-results.json](native-results.json) for the selected cases and hashes.

## Kurtosis validation

The owned enclave is `eez-reth27429-20260926`; every lifecycle command exports
that name. Candidate node, signer and deploy images contain the frozen source
above. The canonical L1 retains the comparison topology's official v2.6.0 image
at digest `sha256:8ce703acf113b2a20705b6e76adebed20f74ba8591bc7c9407203b2968aca70d`.
Lighthouse is v8.1.2 and the separate rbuilder fixture reports Reth v2.2.0. Thus,
not every execution component is the new upstream revision. The new forkchoice
behavior is checked separately against the exact upstream source.
The [container image record](topology-images.json) includes the actual running
services and completed setup containers.

The repository CI runner is copied locally with only its location resolution
and cleanup trap adjusted to retain the enclave for follow-up checks. Its
assertions and workload selection are unchanged. Initial relay startup had an
empty validator cache. The owned housekeeper was explicitly restarted at
22:10:21–22:10:30 UTC; the runner also performed its ordinary refresh at
22:11:57–22:12:01. First logs are retained. This does not establish unassisted
cold relay startup.

All 32 eligible protocol scenarios passed, with complete accounting,
independent causal verification and authenticated semantic blob evidence. The
four exclusions from the pinned runner's `all` selection are explicit in
[protocol-summary.json](protocol-summary.json). The four wave modes and state
chaining passed; the real signer produced 29 signed windows and the Composer
accepted 29 remote attestations. No L1/L2 commitment divergence was reported.
See [kurtosis-result.json](kurtosis-result.json).

Generated genesis artifacts were checked with the release candidate before
deployment; the expected state root remained
`0x465f8a5fe77e19d7e12db1c29ca2b983f62b4e63a61386d8a8514fcd1440c84f`.
The clean restart preserved settled block 768 and its hash, both settlement
correlation directions, and L1-registry convergence. RPC recovered in 10 seconds
and correlation in 20 seconds; [restart.json](restart.json) records the check.
This is SIGTERM restart evidence, not forced-crash recovery.

The additional blob-DA check verified a Counter deployment and two mutations,
with value 42 at safe L2 block 834. Both correlation directions and the canonical
blob batches authenticated all three receipts; [blob.json](blob.json) retains
transaction/endpoint identities. Blobscan was explicitly disabled because this
lean topology does not include it. A fresh follower then reconstructed the same
safe block/hash using only canonical L1 and Beacon data, checking 66 blocks
(64 derived) and all three receipts. The selected replay test passed with zero
ignored tests; see [independent-replay.json](independent-replay.json).

## CI follow-up: September 27

[Run 36275978196](https://github.com/eduadiez/rollupX/actions/runs/36275978196)
tested PR head `d5f92cbb454c7fcb5e13678e50070dbf5ae9f816` through merge
commit `e82857e8a341abef7d974c7048ae2c742473b877`. Both have the same source
tree; the downloaded node/signer images carry the merge revision. Lint, runtime artifact
production, and all eight native E2E shards passed: 63 cross-chain and 36 embedded
cases, **99 selected process tests** in total. Partition-excluded tests are not
counted as executed. These CI binaries predate the corrected masked-state crash
fixture, which has the separate local comparisons above.

The CI Kurtosis job failed one protocol scenario, `revertFromOtherChainL2`;
31 passed. Its L1 posted-table field comparison passed, but the L2 table verifier
found zero entries instead of one at receipt block 852. That scenario's enclosing
rollback intentionally removes its consumption event; the separately loaded
table must still be verified. The local full suite passed the same scenario.
The CI failure remains a failure while investigated; local success does not
establish a passing CI gate. [ci-d5.json](ci-d5.json) records job identities,
transaction/block references and log hashes.

GitHub rejected the diagnostic artifact upload because the repository's artifact
storage quota was reached. The retrievable job console contains bounded failure
diagnostics, but the full CI scenario logs/databases are unavailable. Local raw
evidence is retained separately.

The failed scenario then passed locally in isolation, including independent
causal and semantic blob verification; see
[ci-scenario-diagnostic.json](ci-scenario-diagnostic.json). A second local full
parallel suite also passed **32/32**, with complete independent verification;
see [ci-parallel-diagnostic.json](ci-parallel-diagnostic.json). These diagnostic
runs reused the local enclave. They do not erase the CI failure or establish
that it was a test-only issue.

The initial investigation found that the pinned runner reads the trigger receipt
before waiting for settlement. Its L2 content scan depends on surviving call
hashes, which this rollback scenario does not have. Receipt relocation was then
a hypothesis: the original CI log lacks a second receipt observation.
The wrapper now samples the first trigger's current
receipt for up to four failed scenarios, with one three-second RPC request per
scenario, and prints only its identity, position and status. This never changes
the original failed verdict or retries verification. An offline regression
checks a moved L2 receipt with the failure retained; a read-only live probe
against the successful isolated scenario checked the same diagnostic function.
The final full Python tooling run passed 262 tests, with the same four opt-in
skips; shell syntax, Markdown file links, JSON parsing and whitespace checks
also passed. [checks.json](checks.json) retains the corrected diagnostic test's
initial setup failure and subsequent full-suite result.

## Local reproduction and correction: September 27

The fresh enclave `eez-reth27429-local-20260927` used the exact public node and
signer image digests from the failed CI job. Their merge revision is `e82857e`,
whose tree equals PR head `d5f92cb`. The CI payload and deploy image could not be
pulled anonymously. Deployment and independent verifiers therefore used the
retained local shipping build, with matching contract/script/genesis sources.
The [identity and results](local-ci-reproduction.json) distinguish these inputs.
An observational loopback proxy recorded receipt/log RPC responses unchanged;
its additional request latency and closed-client connections are recorded.

The first complete run passed all four workload modes, state chaining and all
32 eligible protocol scenarios, including independent causal and semantic blob
verification. It recorded 34 signed windows and 34 accepted attestations. None
of the 15 observed L2 trigger receipts moved in that run. Relay startup required
manual intervention; an additional API restart reset its validator cache and
delayed payload reveal. This is not unassisted cold-start evidence.

A controlled follow-up paused only the enclave's builder for 28 seconds when
the rollback trigger was submitted, then resumed it automatically. This
reproduced the missing-table failure with the unchanged protocol runner:

| Runner | First receipt block | Settled receipt block | Result |
| --- | --- | --- | --- |
| Pinned runner | 882 | 900 | Failed: verifier still checked 882 |
| Corrected runner | 1104 | 1128 | Passed, including independent causal/blob checks |

The same unchanged field-level verifier failed at block 882 and passed at block
900 with the same expected table. The original receipt hash was orphaned, the
L1 counter remained zero, and the L2 enclosing-revert flag was true. Thus the
controlled failure is a stale test observation. The historical CI signer also
approved a replacement from range 847–852 to 847–858, which is consistent with
this mechanism; its missing final receipt prevents direct attribution of that
particular historical transaction.

The [test-runner patch](protocol-safe-receipt.patch) makes full and staged L2
verification wait for a current canonical receipt covered by the safe head.
It retains the existing table fields, event policy, settlement checks and failed
verdicts. Six mocked polling regressions passed, including moved/orphaned
receipts, missing/malformed responses, reverted status and unavailable safe
state. The corrected full live suite then passed **32/32**, with independent
causal and semantic blob verification. The core Solidity suite passed **517**
tests with the repository's explicit `script/*` and two named exclusions. All
**36 Anvil scenarios** passed on the dedicated 28545–28616 port range. These
include four cases excluded from the live EEZ runner; their Anvil success does
not extend the live signer's supported profile.

Local protocol commit `7a55be9d54f75eabad6a876f1fd0309c50cb4e7e` contains only the
runner, tests, CI step and documentation. Contract sources and compiler settings
are unchanged. Its patch applies cleanly to the pinned revision and merges
cleanly with `feature/events` at `edf592f`; the latter's additional contract
changes are outside this migration. GitHub rejected both an organization-branch
push and personal fork creation with HTTP 403. The patch is attached for review;
the root protocol gitlink remains unchanged until the fix can be published.

Separately, [CI run 36299583785](https://github.com/eduadiez/rollupX/actions/runs/36299583785)
passed all lanes for implementation head `3893cfd`: lint, runtime production,
**99 selected native tests** across all eight shards, and **32/32** Kurtosis
scenarios with independent evidence. See [ci-3893cfd.json](ci-3893cfd.json).
That run uses the original protocol runner and is a later passing observation;
the earlier failure remains recorded.

## Evidence and remaining acceptance

The enclave dump completed, then only `eez-reth27429-20260926` was removed.
Inspection confirmed that the enclave and its containers are gone. Frozen
binaries/images, native failure datadirs and the dump remain available locally;
see [cleanup.json](cleanup.json).

The September 27 reproduction enclave was also removed after its databases,
rejected-bundle captures, service logs and enclave artifacts were saved. Its
compressed database archive and checksum are recorded in
[local-ci-reproduction.json](local-ci-reproduction.json); these raw artifacts
remain local-only under the reproduction checkout.

Local-only commands, logs, failure datadirs and frozen executables are retained
under `artifacts/reth27429/` in the isolated PR checkout. They are ignored and
unavailable in another checkout; committed summaries provide compact portable
evidence. Earlier v2.6.0 evidence remains in the
[previous audit](../2026-09-26-reth-v2.6-pr/README.md) and does not validate this
candidate.

Local and CI runtime validation passed. Publishing and adopting the separately
validated protocol test correction remains pending because of repository access.
No existing network was upgraded and no release tag was promoted. Public
Gnosis/Chiado operation and historical database migration/rollback are outside
this fresh-network validation.
