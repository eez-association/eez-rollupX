# L1 static local-write evidence oracle

The `staticLocalWrite` contract succeeded on the R03 Kurtosis network, but the
independent collector rejected it because only the single-lookup scenarios had
an oracle. The initial runner result remains a failure. A separate read-only
continuation now verifies the same canonical transaction and settlement without
rebroadcasting or changing runtime behavior. Exact identities, source hashes,
claims and local evidence hashes are in [the record](static-local-write-oracle.json).

The accepted contract is the pinned protocol's
[static-entry specification §4.4](../../../eez-core-protocol/docs/STATIC_ENTRY.md#44-local-writes-between-identical-static-reads),
with the actual scenario and resolver inspected at
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. One L1 transaction writes a local rate
and reads a remote quote three times: 1 → 2 → 1. Each quote reads the rate back
through one static callback. The second lookup tries candidate A, rejects its
callback-result hash, then selects B. The third lookup starts at A again.

The collector now routes this scenario to the existing lookup pipeline. Its
additional checks are:

- Recompute the lookup identity with canonical ABI encoding. Verify every field
  of each same-key persistent candidate, including its common root pin, callback
  identity, static flag, zero value/gas/rollback span, success and return bytes.
  Independently compute both untagged callback-result hashes. Retain all rows;
  A/B/A is valid, and an identical A before B increases the observed retry count.
- Match the three actual proxy/registry reads, proxy forwarding frames and all
  callback attempts in the canonical call trace. Require returns 1, 2, 1 and
  callback results 1; 2, 2; 1 for the observed A/B/A table. Verify all five reader
  state getters at the trigger's L1 block.
- Bind exactly six logical actions in one authenticated semantic bracket:
  three quotes and their three callbacks. Candidate retries are local validation,
  so they must not appear as additional logical actions or L2 deliveries.

Existing receipt/canonicality checks, same-block queue ownership, KZG/table
binding, canonical L2 endpoint/root verification, quote-key absence and final
reorg checks remain in effect. The original post contains another scenario's
bracket; it is not accepted as evidence for this trigger.

The read-only continuation passed with three serialized candidates A/B/A,
selected indices 0/1/0 and callback attempt counts 1/2/1. Its shared root equals
the authenticated canonical settlement endpoint. Quote-key absence was checked
from L2 block 5377 through 5970 on the final collector bytes. The earlier
continuation through 5838 remains separately recorded. This record does not claim a fresh scenario
broadcast, a fresh complete protocol suite or independent cold follower replay.
L1-origin semantic brackets contain empty transaction bytes, so the blob
verifier's `all_trigger_bytes_bound: false` is preserved; the canonical trace,
identity and settlement tables establish the binding used here.

The regression tests execute the real shell/jq logic. RPC, KZG and Forge are
mocked boundaries, complemented by the captured real trace and separate live
continuation. They cover A/B, A/B/A and A/A/B/A, rejecting mutations of candidate
identity, roots, results, callback fields, trace retry counts, forwarding,
semantic parent/index relationships, extra logical retries, persisted state and
L2 absence. Rejections assert the intended verifier error, not just a nonzero
exit. The first dispatch regression reproduced the original missing-oracle
failure. The first full unit run encountered shared `/tmp` quota failures in
existing L2 tests; the retained logs identify that environment failure, and the
final run uses the repository's private `/tmp` bind.

The final focused suite passed all 30 tests in 159.521 seconds, with no skipped
tests; shell syntax and diff whitespace checks also passed. Validation commands
and hashes are recorded in the JSON. No Rust compile,
contract deployment, new transaction, runtime restart or protocol pin change was
needed for this correction.
