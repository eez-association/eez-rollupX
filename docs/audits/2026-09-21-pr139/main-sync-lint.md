# Main synchronization and lint repair

## Revision and scope

The user requested updating the PR139 branch to current RollupX main and fixing
main's failing lint lane. The public HTTPS fetch resolved main to
`37f6f5a56de6f7bd6be4c56bee96045bc918e4bd`. The configured SSH fetch could not
authenticate; no remote configuration or credentials were changed. The original
`main` worktree and its untracked operator/research documents were left untouched.

Merge commit `ede7b8f19dc0534cd2251d011dc44812b513b595` joins previous branch tip
`80484ac93078a5ee86fea847293a689b8881390d` with that main revision. It includes
main's queue metrics, settlement observations, and unrelated-beacon-blob filter.
Protocol remains `3c0cdf4955941a5c23639346932b55b33babebbd`.

Conflicts were confined to collection imports in `held_pool/status.rs` and
adjacent tests in `held_pool.rs`: both imports and both sets of tests were kept.
The merged production diff against main retains the backport's source-deferral
helper and bounded local-removal observations alongside main's read-only queue
snapshot/metrics. No ownership or settlement policy was deliberately changed.

## Reproduced lint failure and correction

Main's [CI run](https://github.com/eduadiez/rollupX/actions/runs/35619308497),
Lint job `106398059542`, failed at **Check dependency policy**, before Clippy.
Its log reports `RUSTSEC-2026-0292` against `imbl-sized-chunks 0.1.3`, reached via
`imbl 7.0.0` and both pinned Reth transaction pools. The unchanged merged candidate
also failed `cargo +1.94.0 deny --locked check advisories bans sources` locally;
bans and sources passed. First failure: `artifacts/pr139-main-sync/deny-before.log`.

The [advisory](https://rustsec.org/advisories/RUSTSEC-2026-0292.html) identifies
panic-unwind memory safety in chunk/inline-array removal and lists `>=0.2.0`
as patched. The compatible
[imbl 7.0.2 release](https://github.com/jneem/imbl/blob/v7.0.2/CHANGELOG.md)
uses that version without changing imbl's public API for this dependency update.
The downloaded chunk source was also checked for metadata updates before its
deferred element drops. This is not a full independent dependency security audit.

`cargo +1.94.0 update -p imbl --precise 7.0.2` produces the required update:

- `imbl`: `7.0.0` -> `7.0.2`.
- `imbl-sized-chunks`: `0.1.3` -> `0.2.0`.
- Remove unused `bitmaps 3.2.1`; imbl now uses the already locked `equivalent`.

Cargo also initially reselected `data-encoding-macro-internal`'s broad Syn
requirement to the existing Syn 1 version. That unrelated edge was restored to
main's Syn 2 selection, and locked checks accepted it. A structured comparison
against main confirms all retained package records are identical, including all
208 Git-sourced records. No Reth/Stateless/Gnosis/REVM/Alloy pin, Cargo manifest,
advisory exception, or dependency policy changed.
Final lockfile SHA-256:
`10e5991a1ef61bdda72539278896c729c729553146980146b2b4d25946325797`.

## Validation

Rust 1.94.0, cargo-deny 0.20.2, cargo-audit 0.22.2. Logs and local orchestration
are under `artifacts/pr139-main-sync/` and are not available in another checkout.
Advisory database commit: `57ad4063bb49c1deb04b6fcee30cfbac6b508474`.

- `cargo +1.94.0 deny --locked check advisories bans sources`: **passed**.
- `cargo +1.94.0 audit --json`: **passed**, zero non-ignored vulnerabilities.
  Existing policy exceptions remain unchanged. Wildcard/yanked warnings and
  cargo-deny's non-fatal workspace-diagnostic messages remain visible in logs.
- `cargo +1.94.0 fmt --all --check`: **passed**.
- `python3 -m unittest discover -s scripts/ci/tests -v`: **success**, 245 tests
  in 108.619 seconds, four existing opt-in integration tests skipped.
- `python3 scripts/check-settlement-policy-config.py`: **passed** for Chiado and
  Kurtosis.
- `cargo +1.94.0 clippy --offline --locked --workspace --all-targets -- -D warnings`:
  **passed**, 36.06 seconds, including both Reth transaction-pool copies with the
  patched imbl. Reused `target/pr139-check`; global `RUSTFLAGS`/wrapper overrides
  were unset and warnings were denied via Clippy's command arguments. This is
  not a fresh hosted-runner build with CI's global `RUSTFLAGS=-D warnings`.
- Affected unit suites: **435 passed**, zero failures, one ignored manual
  held-pool profiling test. Blob codec: 21; composer: 214; L1: 138; node: 47;
  testkit: 15. Both main's queue/settlement metrics tests and the backport's
  funding-deferral/local-removal tests passed. Compilation took 1m26s, followed
  by approximately eight seconds of test execution; no diagnostic retries.
- `git diff --check`: **passed**. The fetched main commit is an ancestor of the
  branch; no protocol gitlink change or unresolved merge remains.

Unit command, reusing `target/backport-rust-1.94`:

```bash
cargo +1.94.0 test --offline --locked \
  -p eez-composer -p eez-testkit -p eez-node -p eez-l1 -p eez-blob-codec --lib \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-follower.opt-level=0
```

These are development-profile unit tests, not release-performance evidence.
All results above apply to the merged source plus the exact lockfile change
described here; subsequent edits in this operation are audit text/links only.

Lint and unit compilation use four Cargo jobs within a four-CPU, 18-GiB,
zero-swap systemd scope, with temporary files on the workspace filesystem.
The scopes run sequentially. No live service was deployed, restarted or funded.

The previous full release validation belongs to `4da1efd0`, not this merged
candidate. Full native/release/Kurtosis validation has not been repeated here;
the previously recorded embedded restart timeout is not resolved by this lint
fix. No GitHub push, workflow dispatch, PR creation or image publication occurred.
