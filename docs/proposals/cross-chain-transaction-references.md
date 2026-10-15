# Cross-chain transaction references

Status: implementation requested by the owner on 2026-10-15 for
[issue 90](https://github.com/eez-association/eez-rollupX/issues/90).
Baseline: fork `227b24d90a059a5af4a81b7a9fe29104bebd385e`, protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. This is an additive observation API;
it does not change the core protocol or authorize new execution profiles.

## Response contract

Both Composer fronts keep `eez_getCrossChainTransaction([sourceHash])`, its
existing fields and null result for unknown source hashes. Add `transactions`:

```json
{
  "basis": "attempt",
  "complete": true,
  "attemptGeneration": "0x7",
  "l1": { "chainId": "0x27d8", "hash": "0x…" },
  "l2": [
    { "chainId": "0xdd6d9", "hash": "0x…" },
    { "chainId": "0xdd6d9", "hash": "0x…" },
    { "chainId": "0xdd6d9", "hash": "0x…" }
  ],
  "postBatch": { "chainId": "0x27d8", "hash": "0x…" }
}
```

Every reference has the Ethereum chain ID as a canonical hex quantity, not a
rollup ID or an unqualified hash. L1-origin work uses the signed source as `l1`;
L2-origin work uses its proposed posting transaction. The posting role is also
returned as `postBatch`; it may equal `l1`. The L2 list includes every native
load, signed source and callback/delivery assigned to that source, in physical
order. Multiple references on one chain must not be collapsed to one. Internal
EVM calls have no separate transaction hash and do not create entries.

Native L2 loads and deliveries retain transaction/receipt type `0x76`. Clients
fetching these receipts need an EEZ-aware or open receipt decoder (for example,
Alloy's `AnyNetwork`), or raw JSON. Ethereum's closed receipt enum rejects that
existing native type. Do not relabel native receipts as legacy/type 2.

Before composition, `basis` is `source`, `complete` is false, and
`attemptGeneration`/`postBatch` are null. An inbound source already has its `l1`
reference and an empty `l2` list; an outbound source has its own L2 reference and
null `l1`. Once an attempt exists, `complete: true` means its complete candidate
group is known. A lookup-only inbound source can have a complete empty L2 list.
It never means all candidates executed or settled. Receipts can still be null.

References follow the currently retained owner generation. A replacement may
change the poster and native transaction hashes. A retry, partial settlement or
terminal eviction must not turn proposed hashes into claimed canonical evidence;
the existing ownership fields and canonical receipts remain authoritative.
This endpoint accepts source hashes, not arbitrary native child hashes, and is
not an archive of all superseded candidates.

## Implementation and boundaries

Map each user through the existing validated source forest and physical
checkpoint positions, not all transactions in a shared batch. Attach hashes
from the final built native block after gas limits are resolved. Verify complete,
non-overlapping physical coverage and the outbound source's membership. Store
only chain IDs and hashes with the attempt manifest. Preserve them with the
existing bounded terminal observations when finality prunes the ledger.

The reference format permits different L2 chain IDs. The current runtime/signer
profile executes one paired L2 per Composer: it cannot observe unsupported
foreign-L2 execution. This change does not claim remote-chain discovery or add
multi-rollup execution support. Never invent references for another chain.

The API retains its existing in-memory lifetime: queued/attempt records and the
last 1,024 terminal observations disappear on restart; older terminal records
expire by count. Added memory is proportional to those retained groups, bounded
by their physical transaction counts. It adds no durable ownership or historical
index. Clients requiring archival lookup must retain results or use a separate
indexer. This limitation applies to the old status fields too.

## Compatibility and acceptance

Older clients can ignore the additive field; newer clients must treat an absent
field on older nodes as unavailable, never as an empty completed execution.
No database schema, historical decoding, follower replay, configuration, signer
wire, contract or genesis migration is introduced. A binary upgrade enables the
field; rollback drops it. Drain accepted work for either restart as usual.

Required checks: source-only pending/unknown responses in both directions;
multiple inbound actions and nested outbound callbacks; no neighboring user's
transactions; chain IDs on every reference; lookup-only groups; malformed
physical boundaries; replacement/finality retention; canonical receipt-backed
process checks and existing partial-settlement/reorg regressions. No test may
treat an attempt's complete list as proof of canonical settlement.
