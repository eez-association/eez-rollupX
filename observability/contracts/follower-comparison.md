# Optional independent follower comparison

The indexer accepts `"follower": {"url": "http://private-follower:8545", "eez": "0x..."}`.
Omitted or `null` disables the collector and causes no follower/database work.
The nonzero EEZ address must be one of the configured L1 posting contracts. The
follower must use the same configured L2 chain ID/genesis, L1 chain and rollup.
URLs use the existing HTTP(S) validation and credential-redacting configuration.

The collector compares the newest currently indexed canonical settlement for the
selected EEZ contract, ordered by L1 block and transaction index, then exact L2
block number. It never turns height convergence into agreement. It checks the
exact L1 occurrence, transaction index, L2 number, block hash and state root;
recomputes complete header hashes; rechecks canonical headers, diagnostic epoch /
generation and the SQL target before publishing. Source invalidity and missing
headers remain unknown. A resumed same-height settlement targets the replacement
hash, not the superseded block at that number.

`eez_getDerivationEvidence` is an additive read-only RPC. The existing correlation
RPC methods, wire fields and persistent MDBX v2 format are unchanged. Its one
parameter is an object containing `l1BlockNumber`, `l1BlockHash`,
`transactionHash`, `transactionIndex` and `l2BlockNumber`; numbers are JSON u64.
It returns version 1, a random process `epoch`, explicit `identity`, `ready`,
scan `generation`, `through`, and optional `evidence`:

- The launchers supply the process role. A Composer endpoint cannot qualify.
- A follower configured with the sequencer unsafe-head overlay advertises that
  mode and cannot qualify, even when its safe chain currently agrees.
- Public scope contains L1 chain ID, EEZ contract and rollup ID, never endpoint
  URLs, signer material or credentials. Standard RPC separately validates L2
  chain/genesis on every collection.
- Evidence contains the exact reference, actual executed header, monotonic
  `ageMs` and `outcome` (`settled`, `claimed_state_mismatch` or
  `template_mismatch`). A template mismatch also carries `advertisedHash`.

The deriver captures successful headers only from actual STF execution accepted
by its existing engine commit path and outbound/claimed-state checks. It also
retains structured evidence when the actual computed header fails the existing
complete authenticated-template check before engine commit, or when an accepted
header fails the later claimed-state check. The original native error, displayed
message and recovery behavior remain unchanged; generic execution errors do not
acquire a computed candidate. It does not
manufacture evidence from persisted correlations, already-matching local
transactions, skipped prefixes, or a total replay count. A partially settled
prefix cannot inherit evidence from a later replayed block. Both the in-flight
buffer and global cache retain at most 128 executed headers. Eviction and process
restart remove earlier proof. Reused history stays unknown; an actual new replay
after restart can establish new proof. There is no destructive replay to fill diagnostics.
Reorg suffix removal also retracts this transient cache.

Agreement additionally requires a ready follower, complete scan through the L1
anchor, canonical scan anchor and safe L2 progress. A mismatch requires an actual
computed differing root at the same reference and number, the same expected
parent hash, and mismatch evidence no more than 90 seconds old. An accepted
claimed-state mismatch must still resolve to its current canonical local header.
A rejected complete-template candidate is deliberately not committed: its
advertised hash must equal the observer's exact canonical target, and the current
follower parent must match the target parent before and after collection. This
case does not require the rejected candidate to appear as a canonical block.
Header-only disagreement and different ancestry remain unknown.
A contradictory state root attached to a claimed unchanged hash is malformed
RPC data, not a divergence page. Generic execution failures, unavailable nodes,
missing blobs, lag, old RPC versions and unproven reused history are unknown.

This is proof that the configured process executed the compared block from the
validated parent. It is not an attestation of separate hardware, a full replay
from genesis, or an independent cryptographic proof system. The operator must
actually provision an independent L1-only follower. A boot-time deterministic
native failure still exits as before; an unavailable process is unknown rather
than a fabricated observed root mismatch.

Collection runs in its own indexer worker every 15 seconds, backing off to 30/60
seconds after repeated failures, with a 30-second total
deadline. Requests share the existing four-request concurrency limit, five-second
HTTP timeout and bounded JSON responses. It reads at most one settlement target
per pass. Collection and target lookup never authorize execution or change
receipt accounting. The collector needs no SQL migration or durable proof cache.

Metrics contain no hash, epoch, transaction or URL labels:

| Metric | Meaning |
| --- | --- |
| `eez_observer_follower_comparison_known` | 1 only for a fresh validated comparison; otherwise 0 |
| `eez_observer_follower_state_root_agreement` | 1 agreement / 0 measured mismatch; absent when unknown |
| `eez_observer_follower_unknown{reason}` | One closed failure/unknown reason, including disabled/lag/reused/role/mode/reorg |
| `eez_observer_follower_settled_lag_blocks` | Measured latest-height deficit, never a substitute for root verification |
| `eez_observer_follower_last_attempt_timestamp_seconds` | Collector attempt time |

Cached samples expire after 90 seconds. The mismatch alert requires known=1 and
agreement=0; unknown values must never be coerced into agreement or disagreement.
