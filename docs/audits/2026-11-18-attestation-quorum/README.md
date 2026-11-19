# Local native quorum qualification

## Revisions and scope

Issue #149 was implemented on `codex/issue-149-quorum` from `81404f15e57c699c398538b54447817774c70c1f`. Core stayed at `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The native reference was closed, unmerged rollupX #129 (`5cabdad9` to `c717f992`). The upstream reference was rollup0 #171 (`25aae562`, merged as `aa5341f8`). Runtime implementation began at `e8c1d293`. The fixture correction is `86895434`. The catch-up fix and its initial qualification are at `c6c4e5d1fd61481e87f26be57beebb2a297a14ce`.

The [proposal](../../proposals/attestation-quorum.md), [operator procedure](../../runbooks/attestation-quorum.md) and [RPC contract](../../../crates/eez-control-rpc/SPEC.md#5-validating-and-using-the-response) define the behavior. Tests used Rust 1.96.0, LLVM 23.1.6, Foundry 1.7.1, Compose 5.5.1 and this checkout's locked dependencies.

## Process evidence

The native harness built the three runtime executables from source with the prebuilt overrides cleared. Local SHA-256 digests were:

| Executable | SHA-256 |
| --- | --- |
| Composer | `22a183a665b226cda96bb6e9b77d07cf41e704b81b1ec68e100d283a76791c40` |
| Signer | `bc70604db9273ce620a39034ca0b8db097ff8f1df21a132b4c8faee2a44c7def` |
| Follower | `2968313c587e301e4215938a06fe7322827b8230692253e7a4cf32a23f927d37` |

The serial `cross_chain::attestation_quorum::` module exercises independent signer processes, the embedded L1 and real Core settlement. It checks both execution directions and recovers every attester in the posted L1 transactions. The 2-of-2 case suspends one signer until Composer retains a user-bearing round, then checks zero charged attempts and convergence of the follower's safe prefix. The 2-of-3 cases cover a registered signer still suspended at settlement and a member with an incompatible network key. The latter also checks that the two capable signers retain sufficient capacity to drain a historical backlog.

The first run on `e8c1d293` passed two cases, including follower replay. The wrong-profile fixture failed on its first wallet estimate with the documented retryable `settlement_reconciliation_pending` result. The original evidence remains in `target/quorum-qualified-e2e.log` and `artifacts/issue149-qualified/`. The corrected fixture polls for an estimate across transient reconciliation. The next run at `86895434` exposed a production scheduling defect. Its one-Sync-interval cap held the backlog at 80 blocks as the cursor advanced from 0 to 100. The fee quote then timed out with ordinary history beyond the posting window. That run and its failing datadir remain in `target/quorum-certified-e2e.log` and `artifacts/issue149-certified/`.

Commit `c6c4e5d1` removes the clamp. The same wrong-profile scenario reported a signer ceiling of 65,536 blocks with a five-block Sync interval, and successfully estimated and settled. All three cases passed on this revision (three selected, zero failed, zero ignored). See `target/quorum-catchup-e2e.log` and `artifacts/issue149-catchup/`.

Reproduce the catch-up process run with:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
EEZ_TEST_LOG_DIR="$PWD/artifacts/issue149-catchup/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/issue149-catchup/datadirs" \
cargo +1.96.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::attestation_quorum:: --test-threads=1
```

## Additional evidence and limits

- Nine process regressions passed on an earlier candidate, before the final startup-capacity and follower assertions. They included the three quorum scenarios plus fee quoting, dynamic admission, partial consumption, Gnosis settlement with independent replay, Ethereum fee history and gas-limited grouping. See the local `target/quorum-candidate-e2e.log`.
- The eight affected Rust packages passed 1,326 tests serially on the reviewed implementation. Eight existing profiling, fixture-generation and manual datadir cases were ignored. A parallel iteration intermittently missed the expected trace in `escrow_quote_redacts_remote_failures`. The serial pass does not establish parallel stability. The node library also passed all 107 tests during the startup-limit revision. Workspace formatting and strict all-target Clippy passed on the final implementation.
- All 155 integration contract tests passed, including the 65-case native gas suite. All seven SDK fee tests passed. The conformance checker verified 29 frozen wire vectors and their fixture inputs. Standalone prover compilation and signer/prover documentation with warnings denied passed.
- Compose rendered absent, explicitly empty and two-member environment values on all three supported profiles. Chiado and Ethereum configuration suites passed. CI tooling ran 607 tests with nine skips for opt-in Docker, Kurtosis, native archive and Gitleaks checks.
- Opus 5.5 was consulted on simplification. Fable 5.2 reviewed the implementation and its revisions. Findings led to one embedded policy reader, a bounded registration worker, an absolute retry cutoff, separation of eager preparation from cold recovery, checks on queue accounting and real follower replay. After the native catch-up failure, Opus supported retaining the known cohort's capacity and Fable found no confirmed blocker in the correction. The per-client block-limit check runs before streaming an oversized request, and the final Clippy and process runs covered the exact revised code. Repeated unsigned member rejections can still delay independent work. The runbook describes the observable signal and recovery implications.
- Evidence paths in this record are local and ignored by Git. This was local fixture qualification. No shared network was deployed or reconfigured, and release promotion, live Kurtosis scenarios and third-party signer interoperability were outside this run.

## Pre-PR cleanup

Commit `e006c2e44eb9bc71ad0a708d7a4924d2d0a46a57` reuses the configured proof-system list for gas sizing. Wallet and fee estimates select the first threshold of eligible verifiers and check their shared deployment contracts once per snapshot. Startup and submission emit `eez.composer.attestation.member_skipped` when a verifier is ineligible, including unrecognized bytecode. Quote requests perform the same eligibility checks silently.

Fable 5.2 reviewed the full port and then the cleanup. Both static reviews reported no confirmed merge blocker. Source inspection confirmed that the singleton and quorum next-block paths use the remaining retry deadline, and that explicit-roster proving uses the selected members' clients.

On this revision, formatting and strict workspace all-target Clippy passed. The Composer library passed 483 tests with one manual profiling test ignored. All three source-built quorum process tests passed, covering 2-of-2 outage recovery, 2-of-3 settlement with a suspended signer, and 2-of-3 estimation and settlement with a wrong-profile signer. The 2-of-2 case recovered the posted signatures and verified independent follower replay.
