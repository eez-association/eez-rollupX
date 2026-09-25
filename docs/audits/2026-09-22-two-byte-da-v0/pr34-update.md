# PR #34 base update — 2026-09-24

This record retains separate results for successive PR heads; earlier checks
do not validate later revisions. The latest update is recorded below.

## First update: `11241000`

The `feat/two-byte-da-v0` worktree was fast-forwarded from
`68fde5bb5b0f9743e55a5de98cdff682e9076a49` to PR #34 head
`112410002d2b800467a3ea4b07dabfc25e6d2fdb`. The protocol submodule now matches
that base at `820e50f749f63165be6a5797d4c5fe52cbbbbe34`.

The uncommitted two-byte implementation was reapplied. The recovery stash
`5b534708afdb197560f8a2d4f39799127532f5f3` remains available locally; no
implementation commit or push was made as part of this update.

The initial update targeted `f476a195`; PR #34 then gained `11241000` while
validation was building. The initial build was interrupted before tests ran,
the additional gas-tracing commit was fast-forwarded without conflicts, and
validation restarted on the final head.

Two conflicts were resolved in the captured signer tests and the old mixed
fixture README. The resolution retains the new position-bound static-read
fixture, its independent cold execution witnesses, and rejection of the old
system selectors without signing. The test-only adapter verifies each original
capture's digest/signature before converting its DA wrapper to V0. It does not
alter execution witnesses or introduce a production legacy decoder.

The earlier audit logs apply to their recorded source/protocol revisions, not
this updated base.

## Validation on the updated base

With Rust 1.94.0 and `CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target`:

```sh
cargo +1.94.0 test --locked --offline \
  -p eez-driver -p eez-composer -p eez-proof-signer -p eez-prover-client \
  -p eez-control-rpc -p eez-protocol -p eez-blob-codec -p eez-node-common \
  -p eez-payload-codec
```

Passed: 908 tests, zero failures, three intentionally ignored manual tests
(`profile_blocked_backlog_discovery`, `inventory_native_proving_fixtures`,
`cold_span_feasibility`). This includes all 43 payload-codec tests, 330 signer
tests, 233 composer tests, and the independent commitment/KZG vectors.
The combined fixture resolution passed both positive V0 replay and the negative
old-selector/sidecar mutation gates.

Also passed:

- `cargo +1.94.0 fmt --all --check` and `git diff --check`.
- `cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings`.
- `python3 -m unittest discover -s scripts/ci/tests -p test_chiado_config.py -v`:
  13 passed, none skipped.
- `python3 scripts/check-settlement-policy-config.py`: Chiado and Kurtosis pass.

Local-only logs are under `artifacts/two-byte-da-v0-pr34-update/`:

| Log | SHA-256 |
| --- | --- |
| `component-tests.log` | `ba88b829589fe1ba57f3dbb4ecb0db8f8f7a23375ba81a3d7a188733915f2b48` |
| `chiado-config.log` | `f4acd7592ff9438494f0eec03b4d425757b46d7125532052f6d3dc603228f9e3` |
| `clippy.log` | `c2379d4f06603d174d32f9b36d49e18502d053a11500b66125293c336cbc4572` |

Full process/E2E, Solidity, release-archive and Kurtosis suites were not rerun
for this base update. The component results are not a claim of full CI coverage
or deployment validation. No deployment was performed.

## Second update: `4956d754`

Fast-forwarded from `112410002d2b800467a3ea4b07dabfc25e6d2fdb` to
`4956d754fe0bb5115672f1feb46cb3ff0ee3d7e5` and updated the clean protocol
submodule to `9d13218a355e7d543ca8a9927eea2962600b716d`.
The uncommitted implementation reapplied without conflicts. Tracked codec,
admission, settlement-index and signer implementation files remain identical
to the pre-update snapshot. The three overlapping documents retain both the
new protocol revision and our V0 changes.

Recovery stash `2d8a87ddca691fe244d752f3a270f0bc0cd56ffc` is retained locally.
No implementation commit, push or deployment was performed.

The same nine-package Rust test command above passed on this revision:
908 passed, zero failures, the same three manual tests ignored. Workspace
all-target clippy with `-D warnings`, formatting, `git diff --check`, and both
settlement-policy configuration checks passed. No merge conflicts remain.
`python3 -m unittest discover -s scripts/ci/tests -v` also passed: 255 tests
reported, comprising 251 passes and four opt-in skips (real data-only Docker
image, Kurtosis resource probe, native-archive relocation and runtime Docker
build). Those skipped integrations are not covered by the mocked tooling tests.

New local-only evidence is under `artifacts/two-byte-da-v0-pr34-4956d754/`:

| Log | SHA-256 |
| --- | --- |
| `component-tests.log` | `a58e9951c663f4956b91745a956b63047aa69b181c80bf426b0e03704a24ad17` |
| `clippy.log` | `5c3e2af402753d3a4b05446927b85bf33debb45981a9f1ce828128c1a67b6211` |
| `ci-tooling.log` | `e858ce7c91a33b8c6bdccf820980e5986d12b00849788fc7049e0e1502a3235d` |

This synchronization did not rerun Solidity, source-built process/E2E,
release-archive or Kurtosis suites. PR #34's inherited protocol-pin evidence
is recorded in its [audit](../2026-09-24-events-tip-pin/README.md); it does not
replace process validation of the combined two-byte implementation.

## PR preparation — 2026-09-25

PR #34 merged into main as `f128b6d9bfba7f9edb251490f4a3a01b918c753e`.
Its source tree is identical to `4956d754` (`git diff --exit-code` passed).
This branch was fast-forwarded to that merge commit without changing the
implementation or protocol pin, so the new PR targets main directly.
Formatting and diff checks passed again; the retained validation log hashes
above were rechecked. Only documentation changed during PR preparation;
the runtime suites were not rerun for this tree-identical fast-forward.
