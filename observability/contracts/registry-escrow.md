# Registry rollup escrow

The optional exporter `registry_escrow` object observes the pinned EEZ registry's
`rollups(uint64)` getter. Its `RollupConfig.etherBalance` is native currency held on behalf of
one configured rollup. It is neither the registry address's wallet balance nor
TVL, withdrawal liquidity across assets, or a settlement-success assertion.
The [pinned protocol definition](../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md)
and [RollupConfig declaration](../../eez-core-protocol/src/interfaces/IEEZ.sol)
fix the getter's output order: `address rollupContract`, `bytes32 root`,
`uint256 etherBalance`.

In exporter JSON (or the `exporter` section of indexer JSON), enable explicitly:

```json
"registry_escrow": {
  "registry_address": "0x1111111111111111111111111111111111111111",
  "rollup_id": 7
}
```

Replace the illustrative address/ID with the verified deployment identity. No
ambient environment variables, node credentials or deployment files are read.
Omitted or null configuration disables the worker and makes no registry calls.
Empty, malformed, unknown-field, zero-address and zero-rollup configurations
fail validation. Indexer mode additionally requires this rollup ID to match its
configured deployment. The configured L1 endpoint, chain ID and genesis hash are
reused; no additional RPC URL or fallback is introduced.

Each pass validates the L1 identity, fetches a fresh latest block, and calls
`eth_call` with the configured registry and canonical ABI selector/calldata. Its
block parameter is `{ "blockHash": "…", "requireCanonical": true }`. Providers
without hash-pinned call support fail explicitly. The getter must return exactly
three ABI words with canonical padding and a nonzero registered rollup contract.
An unregistered rollup or empty/reverted/malformed response is unknown, not zero.
The complete U256 balance is retained without intermediate floating-point
conversion. After the call, the worker revalidates L1 identity and the canonical
height/hash/timestamp of the original block. Unrelated head appends are allowed;
a changed anchor rejects the whole observation.

This worker uses the existing shared four-request RPC semaphore, 1 MiB response
limit, five-second request timeout and no redirects. A whole pass has a 60-second
deadline, followed by the existing 15/30/60-second retry schedule. The observer's
existing task set owns and cancels it during shutdown; it has no database state,
blocking worker, background retry task or transaction submission. Its errors do
not stop receipt, wallet-funding or other network collection.

| Metric | Meaning |
| --- | --- |
| `eez_observer_registry_escrow_enabled` | Unlabelled configuration gauge, including zero when disabled |
| `eez_observer_registry_escrow_evaluation_valid` | Current unfailed observation and canonical head are each at most 90 seconds old, allowing five seconds of future clock skew |
| `eez_observer_registry_escrow_state{reason}` | Closed state: pending, none, stale, stale_head, noncanonical, unregistered, malformed, identity_mismatch, transport, http, rpc, unsupported, oversized or timeout |
| `eez_observer_registry_escrow_last_success_timestamp_seconds` | Last actual success; failures preserve it |
| `eez_observer_registry_escrow_balance_native_units` | Observed registered rollup escrow in whole L1 native units; emitted only while valid |
| `eez_observer_registry_escrow_block_number` | Exact L1 height of the valid balance observation |

Except for `enabled`, intrinsic labels are `chain="l1"` and the single configured
`rollup` ID, plus the closed `reason` label where specified. Prometheus supplies
`environment`, `deployment`, `service` and `instance` target labels. Registry
addresses, roots, hashes and RPC errors are never labels. Dashboard queries must
select the deployment and observer instance rather than sum redundant observers.
The metric exposition divides exact U256 wei by 10^18 using integer quotient and
remainder, preserving up to 18 decimal places without intermediate rounding.
Prometheus stores approximate floating-point samples. The value already represents
whole L1 native units (xDAI on Chiado, ETH on Ethereum): do not divide it by 10^18 again or use it for billing, exact accounting or
authorization.

Pending, failed, stale or future observations omit balance and block-number
samples and set validity to zero; no stale-value or wallet-balance fallback is
used. A successfully observed registered balance of zero is a real zero. Latest
canonical observation is not finality and may later reorganize. Historical gauges
are observations, not a reorg-adjusted escrow ledger.

Coverage is in `registry_escrow::tests`: disabled/invalid configuration and indexed
rollup binding; exact ABI and U256 boundaries; unknown versus zero; freshness and
failure suppression; and real local HTTP JSON-RPC collection checking calldata,
hash pinning, identity, reorg, reverted/malformed/unregistered responses and
recovery. These deterministic checks do not establish a live deployment or
production provider capacity.
