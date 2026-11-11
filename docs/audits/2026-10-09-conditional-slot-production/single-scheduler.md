# Single scheduler and delayed cutoff

Status: implementation and validation in progress; no live rollout or performance
claim. This extends the [implementation record](implementation.md), preserving
its earlier failures and the original temporary opt-in design.

## Identity and scope

- PR #79 base: `4c4fec08d8a713497e1c8fc23bc0cf6fe0273e0f`.
- Candidate A branch head: `9ca75baed975d4fd9aef560ea359ac6a800dd3af`.
- A's tested PR merge source: `c3061cb1883eee253890d8310e5393deeb7b4335`, merging
  A into `1eab3955775aec75612a7144d70f8245771ba97d`. Both A and that merge have
  tree `8fc4ad89ea6ff5110402a968c7eb02e1a79c53b2`; source contents are identical.
- Protocol gitlink: `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and
  unchanged. No contract, genesis, ABI, wire, dependency-version or storage changes.
- Candidate B: this change atop A. The user authorized removing legacy scheduling
  and adding an optional delay; implementation is not production acceptance.

[A's CI run](https://github.com/eduadiez/rollupX/actions/runs/37964363878) passed
all required lanes, including all native shards and Kurtosis. The aggregate job's
producer outputs and consumer logs identify these immutable release images:

| Role | Reference |
| --- | --- |
| Node | `ghcr.io/eduadiez/rollupx-node@sha256:0eeb551755a060d7cd845c0cd4dbe871eb33dec1c55eba924d0e58e21da9a85c` |
| Signer | `ghcr.io/eduadiez/rollupx-proof-signer@sha256:63efcafa50c239a447bb941a65335df02ad773f7e2e9bf7c9c51eb62869c961f` |

Both images were downloaded by digest and their release/source/protocol labels
checked against the merge identity. The private CI payload could not be fetched:
anonymous retrieval was unauthorized and the existing GitHub credential lacked
the required package scopes. No access policy was changed. The actual published
runtime images are publicly retrievable. No substitute source build is claimed
to be CI-tested bytes.

## Behavioral and configuration changes

One scheduler requires explicit P and rejects any obsolete preparation-time
setting, including empty. Optional `EEZ_SETTLEMENT_START_DELAY_MS` defaults to
zero, must be less than one L2 interval, and consumes existing deadline headroom.
The absolute trigger is P's scheduled grid time plus delay. P is still normally
paced; a late P does not restart the wait. Queue planning runs after the trigger.
Empty boundaries, original L1 target, proof/submission limits, bounded recovery
and unconditional transaction repooling remain unchanged.

Standalone followers share timing parsing and require the new configuration too,
although scheduling position/delay do not change replay. Existing followers and
signers use unchanged wire formats. See the owning
[settlement contract](../../configurable-settlement-policy.md#conditional-slot-production)
for migration and rollback boundaries.

## Executed validation

Using Rust 1.95.0, `--locked`, LLVM 22 and the existing local debug Cargo cache
with four build jobs:

```bash
cargo +1.95.0 test --locked -p eez-composer -p eez-deriver -p eez-driver --lib -j4 --quiet
python3 -m unittest discover -s scripts/ci/tests -v
python3 scripts/check-settlement-policy-config.py
```

- Rust units: 317 Composer + 44 deriver + 70 driver = **431 passed**. One existing
  manual Composer timing profile is ignored; no filtered tests in these suites.
- After the planner-wait correction and its regressions, the same command passed
  **433 tests** (317 Composer + 44 deriver + 72 driver), again with only the same
  manual profile ignored and no filtered tests.
- Python tooling: **323 passed / 5 skipped** of 328 discovered. Exclusions are the
  opt-in real data-only Docker payload, real Kurtosis resources, real native
  archive relocation, and two real Docker runtime checks. They are not coverage.
- Three-profile configuration checker passed, including missing/empty/invalid
  P/delay, obsolete input transport, precedence and signer environment isolation.
- After the shared follower-parser migration, all 12 focused public-network
  launcher checks passed, including P/delay propagation from the selected args,
  explicit overrides and missing/empty values. These are mocked, not live replay.
  The modified replay script also passed `bash -n`.
- The first formatting check found only new process-test formatting; standard
  formatting was applied before compilation. No runtime failure was hidden.
- Final full workspace Clippy passed:
  `cargo +1.95.0 clippy --locked --workspace --all-targets -j4 -- -D warnings`.
  Final `cargo +1.95.0 fmt --all --check` and `git diff --check` also passed.
- Source-based process reruns of the final timeout correction and the new CI
  matrix remain pending at this commit; the PR validation update must distinguish
  their exact source identity and outcome from the initial run below.

The initial source-based process selection passed **6 tests / 0 ignored / 130
filtered**, in 362.59 seconds after a 7m19s build: delayed-start mixed workloads
on Ethereum and Gnosis, paced ordinary suffix plus independent replay,
stale-seed transaction repooling plus replay, after-P cross-chain arrival plus
replay, and late same-target settlement. It used the pre-review-correction node
test executable SHA-256
`6a1d0dc9a4d8775d39ff13172d088beb525589b52d54473642aee199e063d9ec`.
That result does not validate the subsequent timeout correction; final reruns
are required. Local-only logs/datadirs are retained under
`artifacts/single-scheduler-delay/` (not available in another checkout).

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
EEZ_TEST_LOG_DIR="$PWD/artifacts/single-scheduler-delay/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/single-scheduler-delay/datadirs" \
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  delayed_start conditional_ late_sync_settles_due_history --test-threads=1
```

The unchanged integration-contract build passed with cached compilation and its
existing Foundry lint warnings; this was not a core-contract test run.

## Independent review

A scoped Claude review used read-only Read/Grep/Glob tools with no MCP, shell,
write tools or session persistence. Sharing relevant tracked source/design was
previously authorized; operator files, secrets and databases were excluded. It
reported no hard blocker, but identified the delayed trigger's one-interval
planner timeout as able to overrun the next ordinary tick. The correction caps
waiting at the next future grid tick (or deadline headroom if earlier), logs
timeouts and retains paced recovery. Paused-time tests cover normal/late entry
across K=5/6/12 without adding slow sleeps or production worker machinery. The
driver's Tokio `test-util` feature is dev-only, in the separate unit lane; no
dependency version/lockfile changed and shipping features remain unchanged.
A second scoped review approved this correction without blockers and identified
a test-clock race: paused Tokio time does not pause SystemTime. The regression's
injected timeline now stays ahead of wall time so setup crossing a second cannot
change the expected timeout. Both reviews had no tool permission denials and
made no edits. Static approval is not runtime or live performance evidence.

Follower startup now deliberately requires configuration migration, not a replay
format migration; shared parser tests and launcher checks cover the inputs. The
review also highlighted that a canonical generation change invalidates an
ordinary decision, allowing fresh recovery/policy evaluation. R4 now states that
existing security/recovery exception explicitly. Broader load measurements,
ineligible-queue behavior and live follower comparisons remain release gates.

## Live comparison boundary

Read-only inspection identified an existing active Chiado deployment, not a
disposable second Composer: source `1eab395`, matching protocol, 5-second L1 and
1-second L2 cadence, proof seed 750 ms, slack 700 ms, old preparation 500 ms.
Operator files, databases and services have not been changed. A controlled
handover or an isolated environment still needs to be selected; concurrent
writers must never share databases or a posting wallet.

Compare A with explicit P and B with the same P/zero delay before separately
measuring nonzero delay. Use the same policy/workloads, retain semantic and
independent replay evidence, and record actual block arrival/receipt times.
Do not collect representative timing measurements while compilation or unrelated
workloads contend for the same host. Merely staging images does not run a canary.
