# Builder submission diagnostic summary

| Field | Value |
| --- | --- |
| Design status | Draft for PR review; implementation scope requested by the operator |
| Date | 2026-10-21 |
| Source baseline | PR #67 at `6464a344e404b22b7b65aaa49f602e9d1d5b0056`; target fork main `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074` |
| Protocol baseline | `855fe0602484750861b4c31502a1a91355d33ed6`, unchanged |
| Related work | [PR #67](https://github.com/eez-association/eez-rollupX/pull/67), [fan-out contract](../configurable-settlement-policy.md#builder-relay-fan-out), [operator instructions](../runbooks/ethereum-node.md#why-a-post-was-excluded) |

## Scope and acceptance

Extend the existing read-only diagnostic to observe successful submissions as
well as exclusions. No runtime, contract, queue, or deployment changes are needed.

| ID | Requirement | Regression evidence required |
| --- | --- | --- |
| R1 | Select dispatches, acknowledgements, rejections and drops without requiring an exclusion log. | Successful-only and rejected-only CLI inputs produce reports. |
| R2 | Correlate by target height and exact post hash; preserve different posts at the same height. Use the typed fan-out result over duplicate response breadcrumbs. | Repeated logs, multiple attempts, HTTP rejection and ambiguous response tests. |
| R3 | Report per-builder accepted/rejected submission counts separately from canonical target inclusion/exclusion. Never credit a builder with producing a block based on acknowledgement. | Multi-builder inclusion, absent/unrelated posts, skipped slots, reorg observations, unavailable RPC and pending target tests. |
| R4 | Keep unsupported APIs, transport errors, deadlines, incomplete logs and simulation failures explicit. Query every acknowledging builder's distinct usable hashes. | All fan-out outcomes, malformed logs, missing hashes and stats-result tests. |
| R5 | Publish summaries in text and JSON and document their counting unit and limitations. | CLI subprocess fixtures and runbook command checks. |
| R6 | Add verified public Quasar and Eureka tracing APIs alongside Titan/Bombora; keep BuilderNet's unverified tracing support explicit. Preserve raw statuses/details and reject mismatched returned correlation metadata. | Provider-specific request fields, lowercase failure states, missing/rate-limited records, mismatched hash/target, and builder inclusion claims tested through the CLI. |

## Design and compatibility

The counting unit is one builder domain per `(target block, post transaction)`.
Duplicate events do not increment counts. Multiple configured endpoints for one
builder are grouped: any acknowledgement counts as accepted; a rejection counts
as rejected only if none acknowledged. Transport/deadline outcomes remain visible.
For older or truncated logs without a post hash, retain an uncorrelated target
record. Do not assign hashless response events across different posts sharing a
height; disclose that limitation. These logs cannot establish complete totals.

Accepted means the submission was acknowledged, not that simulation succeeded.
Included/excluded count accepted submissions whose exact post is present/absent
in the current canonical target. Exclusion additionally requires the logged
target timestamp to match. Other accepted submissions remain unresolved. Later
post inclusion is reported separately and does not change the target-slot counts.
These are observations shared by all acknowledging builders, not builder win
counts, successful receipts, full-bundle execution or settlement completion.

Use the existing fan-out fields; older logs can fall back to individual JSON-RPC
responses when correlation is unambiguous. No node upgrade or restart is required.
The draft CLI JSON format adds typed attempt/summary records and lists stats per
builder so multiple hashes are retained. Consumers must update their parsing;
rolling back the script restores exclusion-only selection and the earlier format.

The tracing extension starts from PR head
`841ee4f783e077e95ae0e465799376879138021f`. It adds provider endpoints and an
explanatory `reason` for unsupported stats without changing the CLI or counting
unit. Returned hash/target mismatches remain visible as unavailable evidence;
missing stats and provider detail strings cannot turn acceptance into rejection.
Builder-reported inclusion never overrides canonical post checks. See the
[runbook's API list and availability limits](../runbooks/ethereum-node.md#why-a-post-was-excluded).

## Validation

Run the new regressions against the baseline before implementation, then repeat
them with the focused suite and CLI HTTP fixtures. Run the Python tooling suite,
syntax checks and documentation checks, recording environment failures and skipped
live probes separately. PR #67 records executed results; a passing fixture suite
does not establish that the operator's logs are present or that any builder is
healthy on mainnet.
