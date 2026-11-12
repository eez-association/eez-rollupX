# Hickory 0.26.3 implementation and validation

On 2026-11-12 the user approved the scoped update proposed in the
[initial assessment](README.md). The implementation starts from assessment
commit `25343a1f592ac9413cfa0b41dca7cd2d659e4976`, based on fork main
`20243f70839ec69df9312731e960cd311e060050`. The protocol remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. This follow-up supersedes the initial
assessment's pending-approval status without changing its frozen baselines.

## Change and compatibility

Both `Cargo.lock` and `crates/vendor/reth/Cargo.lock` now select Hickory
net/proto/resolver 0.26.3. Each lockfile changes only those three version and
checksum pairs; dependency edges, other package versions, Git pins and features
are preserved. Both workspaces accept their lockfiles with locked metadata.

Removed RUSTSEC-2026-0118 and RUSTSEC-2026-0119 from both audit policy files.
They no longer match any package in either lockfile, and the accompanying claim
that DNS discovery is always disabled was incorrect for embedded public L1.
No replacement exceptions, network-configuration changes or Reth patch edits
are included.

The [identity contract](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md)
retains validation version 9, capability profile 2 and signing domain 7. There
is no schema, genesis, key, storage or native encoding migration. Historical
decoding is unchanged. Existing coordinated artifact deployment guidance still
applies; binaries must be rebuilt to receive the fix. Mixed versions are
expected to retain protocol compatibility but differ in DNS behavior. Rollback
restores that older DNS behavior and its limitations; it is not a data migration.

## Validation record

The full workspace Clippy check passed with warnings denied. The complete
`eez-control-rpc` package passed six tests (zero ignored); its zero doc-tests
do not add coverage. Formatting and the 29-vector input/schema consistency
check passed. The integration-contract build passed with Foundry 1.7.1 and
Solidity 0.8.34; build warnings are not Solidity test coverage.

Both updated dependency graphs preserve all package features and edges after
normalizing only the three Hickory versions. Root policy scans pass. Unfiltered
scans retain the same five unrelated root vulnerabilities; the vendor test
lock retains those five plus libz-sys, and no longer reports any Hickory finding.

The affected package suites passed 530 tests with no failures: control RPC 6,
follower 9, node 103, node-common 33, signer 341 and prover-client 38. The signer
run includes all 29 frozen session replays. Five explicitly ignored utilities
regenerate or inventory fixtures; they are not counted as behavioral coverage.
Zero-test binaries and doc-test targets likewise add no coverage.

All five selected source-based `cross_chain` process tests passed, with no
retries. They cover bidirectional execution with and without an imported history
update, independent follower replay after outbound static reads and a write,
and real-signer rejection of tampered `PostBatch` calldata and witness input.
The replay case checks the follower's canonical head hash and resulting state.
The harness built Composer, follower and signer together from source with no
prebuilt overrides. Its first case includes that build time (947 seconds);
this is not a runtime performance measurement. The other 206 process tests
were outside this focused selection.

The signer documentation build also passed with warnings denied. The signer
manifest declares no selectable features, so the full workspace all-targets
Clippy run covers its contributor-guide feature scope. Both selected vendored
Reth regressions passed: engine termination waits for cleanup, and the exported
RPC guard shares the existing debug quota. No retries ran; 240 unrelated vendor
tests were outside this selection. The protocol checkout was initialized at the
unchanged gitlink for the integration-contract build and process tests.

Machine-readable identities, counts and local evidence hashes are retained in
[implementation-results.json](implementation-results.json). Raw logs and
executables are local-only evidence, not uploaded release artifacts.

The first two workspace Clippy builds exhausted the host's `/tmp` user quota.
Their compiler diagnostics are retained locally. Moving this task's build cache
to its own directory under `/run/user/1000` allowed the complete check to pass.
Only obsolete task-owned build output was removed; the initial assessment test
executable and failure logs were retained. This is an environment recovery, not
a change to dependencies or a retry-only behavioral pass.

The first process-test build subsequently filled that smaller filesystem while
archiving `eez-node`; no process tests ran in that attempt. The queued vendor
and documentation builds were interrupted to copy the cache to a larger,
task-owned `/dev/shm` directory. Completed unit executables were preserved and
hash-checked separately before removing the obsolete cache.

The final builds use Rust 1.95.0, LLVM 22, two Cargo jobs, no incremental cache
and `CARGO_PROFILE_DEV_DEBUG=0`. The root development profile retains its
configured optimization level. These are local development builds, not the
release artifacts produced by CI.

### DNS regression evidence

The root workspace cannot directly execute external Reth package dev-tests.
The retained [probe source](dns-probe.rs) and [manifest](dns-probe-manifest.toml)
close that validation gap without adding a production dependency or workspace
member. The probe calls pinned Reth's actual `DnsResolver::lookup_txt` against
owned loopback UDP servers on ephemeral ports; it does not replace the resolver
with a mock. All dependency package versions/source identities come from the
shipping lockfile, and the three Hickory feature sets match shipping exactly.

The same six tests ran with old net/resolver 0.26.1 + proto 0.26.2 and with all
three at 0.26.3. Only those three lockfile version/checksum pairs and the probe's
resolver version requirement differ. Each run selected six tests, none ignored.

| Case | Old versions | 0.26.3 |
| --- | --- | --- |
| Valid split TXT record | Pass | Pass |
| Negative answer | Pass | Pass |
| Silent server returns bounded failure | Pass | Pass |
| Four malformed wrong-ID datagrams before a valid answer | Fails: lookup returns `None` | Pass: expected TXT |
| Empty-question datagram before a valid answer | Fails: lookup returns `None` | Pass: expected TXT |
| Invalid resolv.conf line alongside a valid server | Fails: parsing returns error | Pass: valid server retained |

These local cases establish changed behavior, not a public exploit or a load
capacity claim. The test server knows the transaction ID and controls replies.
The timeout case tests eventual bounded failure, not a strict latency SLA.
Zero doc-tests were selected and do not add coverage.

The first five-case run could not bind sockets inside the sandbox: one parser
test passed and four tests failed with `Operation not permitted`. With explicit
loopback permission, all five passed on the candidate and three failed on the
baseline. Adding the timeout case initially hit the host's `/tmp` user quota
during compilation. Those failed builds ran no new test cases. After removing
only this task's obsolete scratch build, the final six-case comparison above
completed. These are recorded environment failures, not unexplained retry-only
behavioral passes.

Reproduce in a fresh task-owned directory, copying this revision's root lockfile
as the starting dependency closure. The first invocation registers the standalone
probe package in that copied lockfile; subsequent checks use `--locked`:

```bash
probe_dir=$(mktemp -d /tmp/hickory-probe.XXXXXX)
mkdir "$probe_dir/src"
cp docs/audits/2026-11-12-hickory-assessment/dns-probe.rs "$probe_dir/src/lib.rs"
cp docs/audits/2026-11-12-hickory-assessment/dns-probe-manifest.toml "$probe_dir/Cargo.toml"
cp Cargo.lock "$probe_dir/Cargo.lock"
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo +1.95.0 test --offline \
  --manifest-path "$probe_dir/Cargo.toml" -- --test-threads=1
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo +1.95.0 test --locked --offline \
  --manifest-path "$probe_dir/Cargo.toml" -- --test-threads=1
```

Inspect the probe lockfile and selected features against the production lock;
do not accept unrelated upgrades as part of this comparison. The baseline
requires the old three version/checksum pairs from `25343a1f`, with the resolver
manifest requirement changed to `=0.26.1`. Keep all other probe dependencies and
the test source identical.

## Commands and coverage limits

From the repository root, the completed package and static checks use:

```bash
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked --offline \
  -p eez-proof-signer -p eez-control-rpc -p eez-prover-client \
  -p eez-node -p eez-follower -p eez-node-common -- --test-threads=1
python3 crates/eez-control-rpc/conformance/check_vectors.py
cargo audit
cargo deny check advisories bans sources
forge build --root contracts
```

The full release-artifact and Kurtosis matrices remain CI work. No public L1
DNS deployment, live network workload, performance comparison or Solidity test
suite was run for this dependency-only change. The loopback DNS comparison is
the baseline/candidate behavioral reproduction; the full package and process
suites were not repeated on the old dependency graph. No deployment was made.

The focused process run used task-owned log/datadir directories and the following
selection after clearing prebuilt overrides:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export CARGO_NET_OFFLINE=true
export EEZ_TEST_LOG_DIR=/tmp/issue139-implementation/e2e/logs
export EEZ_TEST_DATADIR_DIR=/tmp/issue139-implementation/e2e/datadirs
cargo +1.95.0 nextest run --locked --offline -p eez-node-e2e --test node_e2e \
  -E 'test(=cross_chain::minimal_bidirectional_cross_chain_smoke) | test(=cross_chain::bidirectional_cross_chain_with_history_at_sync) | test(=cross_chain::real_signer_rejects_tampered_post_batch_calldata) | test(=cross_chain::real_signer_rejects_tampered_witness) | test(=cross_chain::outbound_changing_static_reads_and_write_settle_and_replay)' \
  --test-threads 1 --no-tests=fail --retries 0 --flaky-result fail
```

The final signer documentation and vendored regression commands were:

```bash
RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked --offline \
  -p eez-proof-signer --no-deps
CARGO_PROFILE_DEV_OPT_LEVEL=1 cargo +1.95.0 nextest run --locked --offline \
  --manifest-path crates/vendor/reth/Cargo.toml \
  -p reth-engine-tree -p reth-rpc-builder --lib \
  -E 'test(test_engine_termination_waits_for_cleanup) | test(test_exported_guard_shares_debug_quota)' \
  --test-threads 1 --no-tests=fail --retries 0 --flaky-result fail
```

Final review found no blocking issue in the scoped implementation. The retained
original handoff's provenance limits still apply; this follow-up does not
reconstruct the broader original audit. CI results for the implementation are
outside this local validation record.
