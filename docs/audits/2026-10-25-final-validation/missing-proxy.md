# Missing static-callback proxy validation

Both strengthened process regressions and all four live negative controls passed
on the final candidate. **The terminal campaign required one preparation retry
after canonical readiness: its first 180-second setup timeout remains a failure.**
No negative trigger had been generated or submitted during that failed attempt.

| Identity | Value |
| --- | --- |
| Date | 2026-10-25 |
| Source | `b1881cf737cdc64ee30a9767807f53d21a3daa86` |
| Protocol | Clean initialized `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Verified CI producer | `57d212d80b89079f4ed9123ed379e94790a59bf7`, tree identical to source |
| Shared tree | `2f6d561c097cec4b1e4b874dacd0ba5d643e7dd2` |
| Evidence | [Compact observations, executable/image identity and SHA-256 hashes](missing-proxy.json) |

## Contract and assertions

The pinned [execution contract](../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md)
and [static-entry specification](../../../eez-core-protocol/docs/STATIC_ENTRY.md)
require the static callback's source proxy to exist before the read. An empty-code
address must not silently produce a successful callback. The
[queue contract](../../proposals/cross-chain-queue-liveness.md#simulation-rejection-diagnostics)
requires a bounded terminal diagnostic for deterministic composition failures.

The two existing tests in
[`cross_chain.rs`](../../../crates/eez-node-e2e/tests/cross_chain.rs) now assert the
exact proxy and source chain through the matching front, matching transaction
hash, both terminal state fields, `SimulationFailed`, null canonical completion,
absent direct/front receipts, unchanged sender nonce/balance, no proxy deployment
and no reader write. No runtime logic or signer acceptance profile was changed
for these controls.

## Results

Both process tests passed once in the verified release archive from
[CI 40195871216](https://github.com/eez-association/eez-rollupX/actions/runs/40195871216):

| Test under `cross_chain::` | Result | Evidence |
| --- | --- | --- |
| `inbound_static_read_without_callback_proxy_is_rejected` | PASS, 53.162 s | [Job 121187111423](https://github.com/eez-association/eez-rollupX/actions/runs/40195871216/job/121187111423), line 604, 13:55:20 UTC |
| `outbound_static_read_without_callback_proxy_is_rejected` | PASS, 48.116 s | [Job 121187111327](https://github.com/eez-association/eez-rollupX/actions/runs/40195871216/job/121187111327), line 609, 13:58:34 UTC |

The archive executable SHA-256 is
`f6c2a55d1574e1d19579b11192ad98207e14dd81e48700da99f182169e05aa65`.
All four cross-chain partitions passed, 131 tests total. The two named cases
establish the intended selection; no redundant local process run was needed.

| Live profile | Origin | Result and observation | Further source blocks |
| --- | --- | --- | --- |
| Fees enabled | L1 | PASS: `-32003` / `quote_unavailable`, exact executor rejection, no queue ownership | 2 |
| Fees enabled | L2 | PASS: same admission oracle | 5 |
| Fees disabled | L1 | PASS after readiness: accepted hash becomes terminal `SimulationFailed`; original protocol oracle exits 0 | 2 |
| Fees disabled | L2 | PASS after readiness: same terminal oracle | 8 |

Every case also checked absent direct/front receipts, unchanged sender nonce and
balance, unchanged reader getters, and absent proxy code before and after the
additional blocks. Admission evidence binds the exact missing proxy/chain error
to the RPC interval and the `funding_rejected` event to the submitted hash.
Terminal evidence checks the exact error, `lifecycle` and ownership state,
matching accepted hash and null canonical completion. The JSON records all four
hashes and exact errors without copying raw envelopes or bulk logs.

Fee-enabled admission ran on `eez-latency-final-20261025` after the latency
campaign completed. Terminal checks ran on fresh `eez-missing-proxy-20261025`.
Only `EEZ_FEES_L1_TO_L2_ENABLED` and `EEZ_FEES_L2_TO_L1_ENABLED` changed from
`true` to `false`; actual container environment was verified. All three images
and all other configuration matched. This is a supported configuration;
simulation, proof guards and timeouts were preserved.

## Execution and retained failures

The pinned staged sender requires a successful fee quote, so it cannot reach
its terminal MissingProxy oracle with these deliberately invalid inputs.
Task-local orchestration used the unmodified `network-scenario.sh` preparation
stage, submitted its exact signed trigger through the normal front, and executed
the unchanged pinned `_check_missing_proxy_rejection` body. Its SHA-256 is
`6277082fbf5483422199a2bdea275c6297384f78216b19fd88759f3557362e5f`.
This adapts orchestration, not the production admission path or the oracle.

From the worktree root, each stage used the following invocation, with the
explicit enclave exported as described above:

```bash
export KURTOSIS_ENCLAVE=eez-missing-proxy-20261025
export TMPDIR="$PWD/artifacts/missing-proxy-20261025/tmp"
bwrap --bind / / --dev /dev --bind "$TMPDIR" /tmp --chdir "$PWD" \
  bash artifacts/missing-proxy-20261025/run-terminal.sh terminal
```

The fee-enabled invocation used enclave `eez-latency-final-20261025` and argument
`fee-admission`. The terminal network was started with the existing
`testing/kurtosis/start.sh`, verified CI image environment, and retained
`terminal-args.yaml`; no local Rust rebuild or replacement executable was used.

Two earlier failures remain preserved under `artifacts/missing-proxy-20261025/`:

- `fee-admission-preflight-quota-failure/`: Kurtosis download exhausted the host
  `/tmp` quota before fixture deployment or trigger submission. The invocation
  above bound only its task-owned temporary directory over `/tmp`; it did not
  clean shared storage or change the running enclave.
- `terminal-first-preparation-timeout/`: pinned `network_wait_setup` expired at
  its unchanged 180-second deadline before trigger generation/submission. Relay
  logs contained `not a known validator` and `no bid found` during bootstrap.
  The retained inventory separates the 12 L1 and 10 L2 setup transactions, later
  confirmed canonical, from the negative triggers. This failed preparation is
  not overwritten by the subsequent passes.

Before the single preparation retry, the same canonical readiness predicate as
the latency campaign passed: L1 block 57, age 5.07 seconds; L2 latest 338, safe
330, lag 8; queued/in-flight/blocked/unknown all zero. Only the gate's allowed
enclave name changed; its 600-second limit and predicate were unchanged. The
180-second setup limit, runtime, guards and enclave were unchanged. Each
successful negative case was submitted once, with its own post-setup snapshots.

## Closure and limits

Focused Rust formatting, shell syntax, Python compilation, shared strict
workspace/all-target Clippy, the two CI process tests and four live oracles
passed. The assertions received independent static review; final JSON integrity,
local links and `git diff --check` were checked at closure.

The fee-enabled enclave was stopped by its owning agent at 14:33:29 UTC with
resources preserved. The terminal enclave's last service stopped at 14:57:13
UTC. Independent inspection confirmed all 23 original containers stopped, all
31 original volumes retained, and unchanged mounts. Logs and deployment state
were preserved. A metadata-helper assumption about the service named
`eez-follower` was corrected and its first failure retained: that service is
Lighthouse, not the Rust follower executable.

These are rejection and state-integrity checks; the invalid inputs correctly
produce no accepted proof or settlement. Successful proving/replay evidence is
recorded in the [separate latency audit](latency.md). This campaign does not
establish clean first-attempt bootstrap or standalone Rust follower coverage.
Bulk evidence and task-local scripts are local-only ignored artifacts; the
checked-in JSON preserves compact observations and hashes, not remote access to
those files. The [earlier admission-only record](../2026-10-24-extended-validation/kurtosis/missing-proxy.json)
remains historical evidence and is not counted as terminal coverage.
