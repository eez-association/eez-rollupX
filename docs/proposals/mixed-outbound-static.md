# Mixed static and mutable outbound transactions

| Field | Value |
| --- | --- |
| Design status | Mixed-group signer extension authorized by the user. The separate changing-result protocol design remains draft and outside this implementation. |
| Date | 2026-09-22 |
| Source baseline | rollupX `339bfd578eb9352f0dfbdd604740bb6a1a347e12` (deployed PR #34 merge) |
| Protocol baseline | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Evidence | [Contract reproduction](../audits/2026-09-22-mixed-outbound-static/README.md) |

## Problem and scope

The Composer materializes mixed top-level L2-origin roots under one L1 host,
while the signer admits only one static root alone or mutable roots. The reported
transaction reads balances twice, transfers tokens, then reads both balances
again. Support must preserve both the whole user transaction and its outcomes.

There are two separate limitations:

1. The fork signer rejects mixed and multi-static groups.
2. The pinned EEZL2 top-level static pool chooses the first matching call hash,
   independently of the mutable execution-table cursor. Loading before-write
   and after-write predictions for the same call does not make the latter
   reachable. Core specification E.4's rolling-hash disambiguation applies to
   nested execution; it does not apply to these top-level reads.

A signer-only extension cannot establish correct read-after-write behavior.
The contract characterization uses real proxy STATICCALL/CALL execution and
shows the second read returns the first prediction after a successful mutable
consumption. It does not execute the original user's calldata or live L1 token.

## Requirements and acceptance

| ID | Required behavior | Acceptance |
| --- | --- | --- |
| R1 | Preserve ordered read/read/write/read/read results in a single L2 user transaction and L1 host. | Process test with a real token: balances before and after transfer, signer authorization, L1 execution and independent follower replay. |
| R2 | Independently bind every root and outcome to the source table and L1 host. | Signer rejects changed return data, call identity, position, missing/extra roots, reordered mutable roots and incorrect host hash. |
| R3 | Static calls emit no mutable call events and transfer no value. | Mixed and static-only authorization checks exact mutable event coverage and ether delta. |
| R4 | Reverted frames restore source progress and preserve whole-group semantics. | Mixed roots inside reverted spans, failed mutable roots, repeated reads and nested callbacks. |
| R5 | Historical blocks remain replayable. | Old/new protocol activation boundary fixtures and follower replay spanning activation. |
| R6 | Unsupported requests cannot monopolize scheduling. | Independent eligible sender/direction makes progress; blocked requests retain ownership and nonce dependencies. No eviction from opaque signer error text. |

## Authorized implementation without protocol changes

Accept a complete outbound host containing terminal static reads and successful
flat mutable roots. Preserve the existing mutable-only verifier, including its
nested and reverted cases. The mixed extension does not add reverted roots,
root revert spans, nested mutable roots or separate callback deliveries.

Keep the existing ABI, DA, protocol pin and contract runtime. Update both signer
authorization and Composer/follower derivation to use the shared verifier. Recompute the whole host
hash in call order while reading mutable sources and static sources from their
separate canonical table sections. Require equal outcomes for repeated static
lookup identities. Retain exact event coverage for mutable calls and no events
for static roots. No opaque rejection authorizes request eviction.

Acceptance for this extension: the four-read/one-write group reaches signer
authorization and settles with correct read results and L1 state; altered host
hashes, conflicting cache results, malformed entries and incorrect event coverage
fail closed. Changing-result reads and queue scheduling changes are separate work.

## Separate proposed protocol design (not part of this fix)

Give top-level L2 static lookups a position relative to mutable source-entry
consumption. A possible coordinate is the current EEZL2 `entryIndex`: reads
before a successful mutable consumption and reads after it have different
coordinates, while repeated reads with no mutable progress share one coordinate.
EVM rollback naturally restores the cursor for reverted frames.

Two encoding choices require protocol agreement:

- Bind this coordinate into the L2 static lookup key. The ABI can stay unchanged,
  but the key's meaning changes and requires explicit protocol-version selection.
- Add an explicit coordinate to the L2 static table. This makes matching clearer
  but changes ABI, codecs, commitments and every producer/consumer.

Neither is implemented or declared normative by this proposal. Validate the
coordinate against skipped mutable entries, failed calls, reverted parent
frames, nested static callbacks and repeated identical keys before acceptance.
Conflicting predictions at an indistinguishable position must fail closed.

Extend the shared inverse verifier to consume mutable roots by cursor and static
roots by their protocol lookup identity while traversing L1 calls in execution
order. Composer serializes mutable entries before static entries, so zipping
that combined array with the host call order is incorrect. Recompute the whole
host rolling hash; preserve nested verification and exact mutable event order.
Static calls must not advance the mutable cursor or add receipt events.

Update the signer's profile and authorization together with the shared verifier,
materializer, system transaction construction, DA consumer and follower tests.
Keep original transaction boundaries; splitting or rewriting the user's signed
transaction is not an implementation option.

## Compatibility and recovery

A changed lookup identity or ABI requires an explicit activation rule shared by
Composer, signer, EEZL2 and historical derivation. Updating only binaries cannot
change deployed EEZL2 behavior. Regenerated runtime/genesis fixtures are required
but do not migrate an existing rollup. The live-network migration mechanism and
rollback boundary must be specified before deployment.

Do not silently accept new tables against the old runtime, or interpret old DA
under new rules. Coordinate activation and version checks before allowing new
transactions. A downgrade after activation cannot assume compatibility.

Pending requests are in memory. Restarting to deploy may discard unsettled work;
a database backup does not preserve those requests. Recovery must include client
resubmission and canonical reconciliation, without nonce-breaking eviction.

## Validation and implementation status

The signer and deriver extension is implemented and locally validated. No protocol runtime,
protocol pin, genesis, image or live service has been changed. The linked audit records 855 package tests, 34 Solidity tests, workspace lint
checks, and two process tests including independent follower replay. The full
release/Kurtosis matrix and live deployment have not run.
