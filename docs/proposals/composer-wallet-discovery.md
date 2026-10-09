# Wallet-compatible Composer discovery

| Field | Value |
| --- | --- |
| Design status | Accepted for implementation by the repository owner's request, 2026-10-09; not yet deployed |
| Source baseline | `eduadiez/rollupX` main `a39e2ccd01a28d067f87af4bfdd67c637be97853` |
| Protocol baseline | Unchanged `855fe0602484750861b4c31502a1a91355d33ed6` |

## Contract and scope

Wallet providers can restrict custom RPC methods. Both Composer HTTP fronts
therefore serve a virtual `eth_call` discovery probe in addition to the unchanged
`eez_composerInfo` result. This is runtime metadata, not an EVM precompile,
contract, chain-state read, readiness check, or gas-estimator capability claim.
No contracts, genesis allocations, transaction execution or proving rules change.

The address is `0x7Ae2c80116976915a0Ee9b7994e7Bb12026087f8`, the last 20 bytes
of `keccak256("eez.composer.discovery.v1")`:
`0xed1c297298163dbf67d4d5147ae2c80116976915a0ee9b7994e7bb12026087f8`.
It does not overlap the repository's configured predeploys or pinned
Ethereum/Gnosis precompiles. This is not a global reservation: ordinary RPCs
continue to execute normal EVM calls at this address, and no deployed-chain
code check is implied by this source change.

```solidity
function composerInfo(bytes32 nonce) view returns (
    bytes32 marker,
    uint256 schemaVersion,
    bytes32 echoedNonce,
    uint256 sourceChainId,
    bytes info
);
```

The selector is `0x98da5085`. The marker is
`keccak256("EEZ_COMPOSER_DISCOVERY")` =
`0xf331ada39582afc2fae4082b2baeb82639a90399d9d6688eda56cc24dd03e734`.
Schema version is **1**, independent of the package version. `info` contains
UTF-8 JSON of the same serialized value returned by `eez_composerInfo`.
`sourceChainId` is the configured expected source chain ID of the receiving
front, not an upstream lookup. A fresh client nonce must be echoed exactly.

Only the exact destination and selector pair is intercepted. Calldata must be
`0x`-prefixed hex with exactly four selector bytes and 32 nonce bytes. `data`
and `input` are aliases; when both appear their decoded bytes must match.
Use positional params `[transaction, "latest"]`; an omitted block tag also
means latest. Any other block selector and any third parameter (including null
or empty state overrides) is rejected with `-32602`. An optional valid `from`
and valid wallet gas/fee fields are accepted and ignored. No balance is read;
`value` must be absent or zero. Malformed matching probes return `-32602`.

Requests may be single calls or batch members. Mixed batches retain ordinary
Ethereum forwarding/estimation, IDs (including explicit null), and notification
suppression. The existing whole-batch rejection of any
`eth_sendRawTransaction*` member happens before any work is performed.

All `eez_*` methods are Composer-front-only: discovery, queue status,
transaction status, both settlement-correlation methods, and augmented execution
witnesses. The witness method still requires explicit `debug` enablement in
the Composer's HTTP API configuration and retains its shared worker budget.
Neither Composer's ordinary node transports nor followers expose `eez_*`.
The L1 front's `eez_*` observations describe the same Composer/L2 as the L2
front, not an independent L1 queue/index.

## Design and alternatives

Keep ABI constants/encoding in `eez-protocol` and dispatch in Composer ingress.
Reuse existing queue/correlation/witness modules locally; do not proxy custom
methods to a regular node RPC, duplicate their schemas, add an internal RPC
listener, or deploy a discovery contract. Mixed requests reuse the existing
estimation/forwarding path without adding unbounded parallel workers.

## Compatibility and recovery

Clients must move custom-method requests from regular L2/follower URLs to a
Composer front. Standard `eth_*` and `debug_*` calls remain on their existing
endpoints. Upgrade endpoint configuration alongside the Composer binary;
older fronts do not serve the moved methods or virtual probe. Detection should
fail closed on empty/malformed results, a wrong marker/version/nonce/chain, or
provider errors; it cannot authenticate an untrusted provider.

Restart applies the new routing. No database, genesis, historical decoding,
follower replay or signer wire migration is needed. A binary rollback restores
the old endpoint placement, so client URLs must roll back too. Pending queue
ownership remains in memory; this change adds no durable recovery guarantees.

## Acceptance and validation

- Independent address/selector/marker derivation and ABI round-trip vectors.
- Both source-chain IDs, aliases and conflict/malformed-input rejection.
- Latest-only/override/value policy, unfunded sender and ignored gas fields.
- Discovery while readiness is false and upstream is unavailable.
- Mixed batches, IDs, notifications, invalid members, and submission guard.
- Front-only custom methods; ordinary RPC does not emulate discovery.
- Update repository clients, test helpers and operational instructions.

Use pure unit and mock HTTP tests for this implementation; no deployment,
transaction signing or broadcasting is authorized. Source-based live process
and Kurtosis validation remain CI responsibilities, not claimed local evidence.
