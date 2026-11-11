# Candidate-A live canary

Status, 2026-10-09 UTC: candidate A remains active on the operator-authorized
existing Chiado environment. Handover, canonical settlement and a bounded
ordinary-transaction smoke test passed. Candidate B, nonzero delay, cross-chain
semantic workloads and independent live follower replay remain untested here.

## Identity and preservation

Candidate A is branch head `9ca75baed975d4fd9aef560ea359ac6a800dd3af`, tested as
CI merge `c3061cb1883eee253890d8310e5393deeb7b4335` with identical tree. Node and
signer are pinned to the exact digests in the [single-scheduler record](single-scheduler.md).
Both report protocol `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. The previous
runtime was `1eab3955775aec75612a7144d70f8245771ba97d` on that same protocol.
The unrelated operator checkout remains at `6b1e281a418f845605744c09a6bc9e8782eb6b61`
with its existing Compose edits preserved; it is not claimed as runtime source.
Effective Compose environment/commands matched the old running services before
applying a narrow canary overlay. No host protocol build/deployment was used.

Network: Chiado 10200 / L2 906969, 5-second L1 / 1-second L2. P=2, delay=0,
proof seed=750 ms, slack=700 ms. Existing interval policy (900000 ms), maximum
unsettled height trigger (150 blocks), admission and fee settings were unchanged.
Only node/signer images, scheduling selection and copied database paths changed.

The operator `.env`, deployment bindings, genesis/profile, prior container
configuration and diagnostic logs were preserved privately. Both services exited
0 at approximately 19:34:55 UTC, with no OOM/forced kill. The cross-chain queue
and ordinary pending RPC were empty immediately before stopping. No container
writer remained on the source databases. A network-disabled root helper copied
complete L1/L2 directories with source mounts read-only; originals remain intact.
Candidate A opened the copies. Lighthouse, explorers, monitoring and posting-log
storage were not restarted or changed. Volatile queues are not made durable by
these copies; public ingress was not independently frozen, so the empty-queue
observations are not a proof against arrivals racing shutdown.

The active configuration is a local overlay, not a rewrite of the operator
`.env`. The private handoff README records the exact restart command and warns
that base-only Compose startup selects the old configuration/state. Rollback
restoration and a second restart were not exercised.

## Handover and chain checks

Candidate node started at 19:35:33 UTC (approximately 38-second RPC interruption).
Embedded L1 initially unwound its in-memory tail and had zero execution peers;
normal peer sync resumed and Composer caught up around 19:36:39. No manual
payload import, fork-choice call, state override, deployment or genesis change
was performed. L2's finalized tag temporarily reported genesis during startup;
it subsequently advanced past the pre-handover finalized sample. All sampled
prior safe/finalized L1/L2 block hashes and state roots remained canonical.

Mounted L2 genesis hash/state root match deployment bindings. The implementation
runtime hash matches the retained profile, and the registered proof signer matches
the retained attester. Genesis/profile SHA-256:

- Genesis: `ec2ca22f45a8801ca4e955c4ab4dd6f2cea268bd85a63a85a22d755cf48cc7bf`.
- Profile: `988892b38222a80713a0884fb63c01c031b5cbe2fa8b4f526e1800d7f2641599`.

New settlement `0x2f615de2340a84fa13b313eca244ec1e3fb03af8cd272348f1fe33810a954599`
advanced the registry commitment, resolves with `canonicalL2=true`, has a successful
type-3 receipt, and was sent by the configured poster. Its block/receipt fields
match the independent configured execution RPC. The local Beacon returned the
versioned-hash-selected full blob. This is not independent KZG verification or
follower replay.

## Ordinary-transaction test

A bounded helper sent 40 zero-value self-transfers at approximately 0.5-second
intervals using the existing funded workload account, distinct from the poster.
Both known protocol-checkout workload locks were held. It checked the actual
chain identity and aligned latest/pending nonce before signing. Per-transaction
gas cap 21000 and maximum fee 2 gwei bound aggregate exposure to 0.00168 native
tokens. No wallet funding or contract deployment was needed. The stale saved
workload profile targets an older rollup; its endpoints/bindings were not used.

All 40 transactions obtained successful canonical receipts and the final nonce
advanced exactly 40. Positions 1/2/3/4 included **16/8/8/8** transactions. Every
receipt was joined to a slot-plan record with `prepare=false`, including all 16
late-position transactions. No transaction in a sync boundary is claimed.
The final included range settled through
`0xfedbdabc43b1e7f50411afa22fbc18ce823e8ab2e6531d4b9a85415805506e53`.
The cross-chain queue remained empty at completion. This bounded, uncontrolled
live sample is not a throughput benchmark or matched A/B performance result.

## Evidence and remaining work

Private, local-only evidence is retained under the owned
`data/slot-production-canary-20261009.rTwtCl9Z/` directory, including copied
databases, configuration, exact helper scripts and raw logs. It is not available
from a fresh checkout and must not be published wholesale.

| Sanitized evidence filename | SHA-256 |
| --- | --- |
| `before.json` | `42dcdd182a0c7bd068693ad616fd1b09e773aea0813b35d4e322eb10644d0c7c` |
| `candidate-a-ready.json` | `1e69369b783a490d7bbaa9e185d06eacb066feba1e6be51e6674393cf0696e4b` |
| `candidate-a-settlement-check.json` | `3cda2622f51b54f49fe57969a42a3891f0cf64fd32cc106b3ab7245a2c57916e` |
| `candidate-a-ordinary.json` | `ba733e1790ca8db160249c76c276fe96e9bf001fd3eef3601cbcb9e969303723` |
| `candidate-a-nonsettling-inclusion.json` | `24ac6d6710641f5b7e9e5ddac57480087a54e86d8170ceaf23196bc0fa73cb06` |

Candidate B `a4536050b6cecf80b247344a08348277ba098f92` has passing local units,
process regressions and lint; its [CI run](https://github.com/eduadiez/rollupX/actions/runs/37979169850)
was still producing runtime artifacts after lint and debug-unit lanes passed.
Next: obtain B's verified exact-revision artifacts, preserve A's state before
handover, repeat P=2/delay=0, then assess nonzero delay separately. Cross-chain
semantic assertions, independent live follower replay and representative reactive/
continuous-arrival load remain distinct gates. Keep PR #79 draft.

Later evidence supersedes this smoke-test readiness assessment: the
[continuous-load run failed](stress-failure.md), and candidate B's CI completed
with failure. See the [bug review](bug-review.md) and [repair record](bug-fix.md)
before considering a handover; no candidate-B deployment is recorded here.
