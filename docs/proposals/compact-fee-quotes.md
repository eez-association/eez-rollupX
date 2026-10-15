# Compact, direction-aware fee quotes

Status: draft evaluation requested on 2026-10-15; not an implemented or deployed
API. Baseline is the accepted measured-pricing implementation and its 16,777,216
source gas profile. Preserve the [existing pricing contract](../configurable-settlement-policy.md#unsigned-l2-fee-quotes).

## Recommendation

Introduce `eez_quoteFee` on both Composer fronts. Derive the source direction
from the front: `/composer/l1` means L1-origin, `/composer/l2` means L2-origin.
Reject a supplied chain ID that disagrees. Keep `eth_estimateGas` available for
normal wallet compatibility. Reuse its internal estimator rather than issuing
an HTTP request to the node's own public endpoint.

Allow `gas` to be omitted. The service resolves an executable gas limit, computes
the settlement contribution, then replays the final gas/fee combination before
returning. Gas limits and gas consumed remain different quantities: the source
receipt's charged gas divides the tip; gas-limit headroom is not billed as
consumption. Keep independent L1/L2 meters, including nested execution.

Accept an optional list of requested transaction types, defaulting to both
`0x0` and `0x2`. Apps that already know their wallet's supported type can request
one to avoid unnecessary simulation. Return a separately validated option for
each requested type. Do not convert an already-quoted type 2 into legacy by
renaming fee fields: encoding/DA and fee-dependent execution may differ. A gas
limit may also differ between types. All options share pinned chain state and
policy, and the caller signs exactly one.

Use `eez_debugQuoteFee` for detailed estimates, assumptions, component costs and
context. Reserve a future `eez_traceQuoteFee` for actual execution traces; the
current diagnostic response is not a trace. These are project-specific methods,
not existing Ethereum standards. Both compact and diagnostic methods use the
same calculation and return identical fee options for the same pinned context.

## Candidate contract

Request, using ordinary transaction fields plus method options:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eez_quoteFee",
  "params": [
    { "from": "0x…", "to": "0x…", "value": "0x0", "data": "0x…" },
    { "transactionTypes": ["0x0", "0x2"] }
  ]
}
```

`gas`, `nonce` and `chainId` can be supplied explicitly. Preserve explicit gas
and nonce; reject an unusable value rather than silently substituting another.
Apply the active source chain's transaction/block limits and the service's
declared resource profile; a common method does not imply identical chain limits.
Resolve an omitted nonce from the pinned source state. Current execution does
not simulate queued predecessors: return a clear nonce/dependency error rather
than suggesting a conflicting nonce or claiming pending-queue support.

Keep the compact response to a version, source chain ID, resolved nonce,
expiry, recovery mode, and one option per requested type. Each successful option
contains its type, gas limit, ordinary fee fields, normalized intent hash and
estimated total source fee. The direction is already selected by the front;
policy details, component costs, pinned block identities and replay diagnostics
belong in the debug response. All monetary amounts use source-native base units;
encode quantities as canonical hex. The recovery mode distinguishes conditional
builder refunds from enforced settlement funding.

The SDK combines each option with the original authorized destination, value
and calldata. It validates the normalized intent binding, supplied nonce/gas,
source network, expiry and app maximum fee before sending. It recomputes the
maximum from `gas * gasPrice` or `gas * maxFeePerGas`; the maximum is a wallet
reservation, not an estimate of the amount charged. Signing must preserve the
selected option. Changing type, gas, fees, calldata or nonce requires requoting.
Each intent hash covers that option's resolved type and gas along with the
original destination, value, calldata, sender, chain and nonce. A shared hash of
only the untyped request is insufficient. These hashes detect mismatched intents;
they are not authorizations or a substitute for signed admission and native
validation. The SDK copies only the allowed gas/fee fields into the original
intent; it never accepts a replacement destination or calldata from an option.

For two requested types, represent an unsupported/failed variant explicitly;
never silently return an incomplete list or relabel the other variant as valid.
No success may be returned for a stale shared context. Zero source fees are not
a substitute for an unavailable cross-chain policy.

## Implementation work and unresolved decisions

This is more than renaming the current RPC. Today gas estimation and fee quotes
use different snapshot/preparation paths and separate worker limits. Refactor a
shared pinned context, gas-discovery helper and final fee replay. Avoid acquiring
one worker while waiting for the other. One owner retains the permit until all
blocking work exits, including after cancellation. Bound aggregate simulations
and total deadline across gas discovery and both types. Ordinary wallet gas
estimation must not lose its existing resource protection.

The existing estimator tries a measured requirement plus headroom and verifies
the lower limit against the high-limit execution's effects; it does not promise
a universal binary-search minimum. Contracts observing `gasleft()` or `GASPRICE`
make blind arithmetic unsafe. Fee search must use the resolved final limit and
fail explicitly on non-convergence or changed effects. One client round trip
does not necessarily mean one EVM execution or lower server CPU cost.

The L2-origin fee engine and admission policy already exist. L1-origin user-tip
collection through builder refunds remains a separate unimplemented policy:
declare the direction unsupported until builder eligibility, selected tip,
refund recipient, funding source and conditional recovery are specified and
tested. Do not reuse the L2 tariff or advertise a guaranteed L1 refund. The
common method name provides a stable shape, not identical economics.

## Staging and compatibility

1. Add the compact L2 response and diagnostic method around the existing engine,
   retaining `eez_quoteL2ToL1Fee` and the HTTP route unchanged.
2. Integrate gas discovery with the pinned quote context, then support both
   independently replayed transaction types under a shared work budget.
3. Enable the L1 route after its separate recovery policy qualifies. Until then
   return `unsupported_direction` rather than a fabricated quote.

Keep old methods and SDK support during rollout; do not remove the verbose
legacy response while clients depend on it. Upgrade servers first, then clients
with explicit capability handling. Do not turn transient quote failures into
silent unquoted sends. No change to EVM gas accounting, signing authorization,
storage, contracts or genesis is required for the proposed L2 API work.

Acceptance requires ordinary and nested calls, both types, fee/gas-sensitive
execution, actual Falcon workloads, changed heads/nonces, insufficient balance,
unknown L1 policy, cancellation/permit retention, old clients, and independent
checks that compact/debug options match. Decide the exact response field names,
aggregate simulation budget and error envelope before implementation. The draft
does not mark those choices as accepted or claim the new endpoints exist today.
