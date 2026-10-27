# Final CI runtime consumption

Tracking [CI run 40098162224](https://github.com/eez-association/eez-rollupX/actions/runs/40098162224),
attempt 1, for PR head `0e34a326536befc31ee62a92647bfc3dca9ddee2`.
The producer failed its example tests after the shipping build and dependency
archive guard passed. It did not publish the immutable runtime/test payloads;
no payload was consumed. The preceding feature correction is recorded in
[ci-runtime-archive.md](ci-runtime-archive.md).

The run's referenced workflows and completed viewer checkout log identify
synthetic checkout `bfe39419af4414b03dc375561ac2ef0e340f4caa`. Its parents are
`a50c326d99f7cd1c21406eedbf370a344015a6b5` and the PR head. Its tree,
`89f510e696cfb3da6818bc79b0dcf33acfde3b64`, exactly matches the PR head's tree.
A new detached worktree at
`artifacts/pr-readiness/ci-candidate/40098162224/checkout` starts clean and has
the recursively initialized core pin `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
No existing branch or worktree was reset.

The planned consumer uses the checked-in `ci-payload.sh` and
`prepare-runtime-artifacts.sh` from that exact checkout. Only this run's immutable
payload digest will be admitted. Verification binds the source revision,
protocol pin, lockfile, release profile, platform, archive hash and image labels;
the runtime archive must contain all seven expected executables. An optional
test payload will be checked with the existing native artifact verifier and
cross-checked against the same packaged runtime. Extraction containers are
never executed and publish no ports. Deployment remains with the Kurtosis agent after a successful handoff. The
failed run supplied no admissible payload for installation or replay; no
container was created and no tag was promoted by this consumer.

Identity, commands, source evidence and actual gate outcomes are maintained in
[ci-final-runtime.json](ci-final-runtime.json). Raw CI logs and downloads are
local-only under `artifacts/pr-readiness/ci-candidate/40098162224/`.

## Actual gates and new failure

Shipping compiled in **19m46s**, and archive compilation completed in **12m25s**
without rebuilding a forbidden external dependency. The producer then ran
30 protocol-vector/example tests: **28 passed and 2 failed**, with no skips.
Both failures occur while `prefixes.rs:228` constructs a private-L1 fixture,
before either replay assertion: its genesis equals the reference timestamp,
but its four-second cadence requires that reference to be at or before
`genesis - 4`. It also retains storage from the base genesis calendar.
Production correctly rejects this inconsistent fixture.
The subsequent [fixture correction](ci-prefix-fixture.md) reproduces both
failures locally before editing and passes the same example target afterward.

The debug unit lane passed **1569 tests**, with **3 explicit skips**, and its
shutdown regression passed **1 test**. These results do not cover the separately
selected example tests: the earlier local `cargo test --workspace` likewise
did not execute this target. Lint failed the inherited dependency advisories;
its subsequent gates were skipped. Runtime payload publication and every
downstream embedded, cross-chain and Kurtosis lane were skipped. The failures
and unexecuted gates remain part of this run even after a local fixture repair.
