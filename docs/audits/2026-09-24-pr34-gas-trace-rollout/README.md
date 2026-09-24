# PR 34 selected Composer gas trace rollout

| Field | Value |
| --- | --- |
| Date and status | 2026-09-24 UTC; implementation and local Chiado binary replacement completed; selected live trace pending |
| Scope | Add opt-in admission and final bundle gas diagnostics to PR 34, replace the existing node and signer binaries, and add disabled trace settings |
| Source | `eduadiez/rollupX` PR 34 head `aeaf0778e4f8bd58e5cc53a6c97146522e6b98c0` plus the staged implementation patch with SHA-256 `3bafd062b645f1428d119502142c6eb2a2b109d506bdb29f88c44e4e24e0038b` at image build time |
| Protocol | Pinned gitlink `820e50f749f63165be6a5797d4c5fe52cbbbbe34`; no pin or contract change |
| Runtime images | Node `sha256:0212b0a4cf000685278faadb4ad021db21cd72e962355c5ca583fafd184a65ca`; signer `sha256:4eb19fda2957f2564223dc6e3c16e9b47ddf93bd1a03ee1cb1fce34d8011a60d` |
| Environment | Local Chiado Compose `eez-node-chiado` and `eez-proof-signer-chiado`; Rust 1.94.0; both trace settings empty |
| Procedure | [Composer gas traces](../../runbooks/composer-gas-tracing.md), [existing-rollup Composer](../../runbooks/chiado-existing-rollup.md) |

## Findings

The PR uses gas required before refunds to choose PostBatch's signed gas limit.
The trace records both gas used and gas required without changing that choice.
The selector is the hash of an already signed source transaction. Empty settings
leave opcode tracing disabled. A selected admission execution trace covers the
source and nested local target simulations; a selected preflight trace covers the
complete ordered L1 bundle. The admission budget file records a projection,
which is not an EVM trace.

## Executed checks

| Check | Evidence type | Result |
| --- | --- | --- |
| `cargo +1.94.0 fmt --all --check` | Static | Passed in the PR checkout |
| `cargo +1.94.0 test --locked -p eez-composer --lib` | Unit | 227 passed, 1 existing ignored; includes selected admission/preflight trace gas equality tests |
| `cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings` | Static | Passed |
| `cargo +1.94.0 test --locked -p eez-control-rpc -p eez-prover-client --lib` | Unit | 6 and 30 passed respectively; the signer package has no library target in that invocation |
| `python3 -m unittest discover -s scripts/ci/tests -p test_chiado_config.py -v` | Static/Compose rendering | 13 passed, including absent, empty, and selected trace setting propagation |
| `python3 scripts/check-settlement-policy-config.py`; `git diff --cached --check` | Static | Passed |
| Docker release image builds from PR checkout | Build | Both completed; image IDs above; build labels carry source baseline, patch SHA-256, and protocol pin |
| `docker compose config --format json` with selected images and empty trace settings | Static | Node received both empty values; signer received neither |

The first trace implementation was tested on a different fork checkout; those
results do not validate PR 34. The PR checkout checks above provide the source
baseline for this rollout. No three-case bridge experiment, selected live trace,
canonical PostBatch receipt comparison, process E2E, or independent replay ran.

## Operational changes

The existing queue reported zero queued, ready, blocked, unknown, and in-flight
requests immediately before recreation. A private, local-only copy of the
operator environment was saved at `/tmp/eez-pr34-env-before-20260924` with mode
`0600`; it is not a portable artifact. The existing `.env` received only the two
new image tags and two empty `EEZ_COMPOSER_GAS_TRACE_*` settings. Compose recreated
only `eez-proof-signer` and `eez-node` with `--no-deps --pull never`. The existing
datadirs, genesis, keys, contracts, deploy image, and other services were retained.
The signer reported healthy, and node RPC answered after restart. The trace
selector remained empty, so no diagnostic was generated. Rollback was not
exercised; the previous node and signer image tags were both `events-820e50f`.

## Remaining acceptance work

For the requested gas comparison, choose a signed source hash, set it with
`EEZ_COMPOSER_GAS_TRACE_DIR=/l2/gas-traces`, and restart only the node at a
controlled time. Repeat for inbound, outbound, and mixed groups, then compare
the trace's whole-transaction totals with canonical PostBatch receipts. The
local startup and queue checks do not establish semantic correctness of those
three cases or recovery from historical data.
