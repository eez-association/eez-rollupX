# Composer gas traces for selected cross-chain sources

This diagnostic records the admission calculation and local EVM execution for
selected cross-chain source transactions. It is off by default. Select one or two
**already signed** source hashes, or one address before sending transactions.
Address mode matches the source sender, source recipient, any executed internal
frame on either simulated chain, or a resolved cross-chain target. It retains
at most eight distinct matching source transactions per node run. The Composer
still decides whether and when each source is eligible. This procedure does not send a transaction,
restart a service, or change admission limits by itself.
It changes no wire, proof, or chain storage format. A Composer binary that
supports only hash selection rejects address selection when the directory is
set; use a matching configuration checkout and address-capable binary. Rollback consists of
clearing all trace settings and using the ordinary restart path. Existing trace
files remain operator data and are not read by Composer on restart.

## Enable for a controlled run

Use a matching source revision and protocol pin. In the Composer's environment,
set the directory and exactly one selector before starting it:

```text
EEZ_COMPOSER_GAS_TRACE_TX_HASH=0x<64 hex digits>[,0x<64 hex digits>]
# Or, instead of TX_HASH:
EEZ_COMPOSER_GAS_TRACE_ADDRESS=0x<40 hex digits>
EEZ_COMPOSER_GAS_TRACE_DIR=/l2/gas-traces
```

For the Chiado Compose profile, `/l2` is the existing `L2_DATA_DIR` mount. The
directory must be writable by the node. A bare process may use any absolute
directory it owns. Empty selectors and an empty directory disable tracing. A
selector without the directory, a directory without a selector, both selectors,
a malformed or duplicate hash, more than two hashes, an invalid address, or a
relative directory fails startup. Compose
reads `.env` at startup, so changing it requires the normal documented restart
procedure. Preserve the existing operator `.env`, image identity, and datadirs.

Hash mode traces only configured sources. Address mode provisionally traces
each candidate while it checks internal calls, then deletes the file if the
address was not touched. This incurs EVM tracing and temporary file I/O even
for unmatched candidates. After eight distinct sources match, later sources
are skipped; already selected sources can still be traced if recomposed.
New directories use mode `0700`
and each JSON Lines file uses mode `0600`. Filenames include the selected hash,
phase, process ID, timestamp, and a sequence number. Files remain in the
configured directory until the operator removes them. Each EVM trace stops
after 1,000,000 steps and records a `truncated` marker; a truncated trace is
incomplete. Calldata, memory, and full stacks are omitted. The output includes
contract addresses, storage keys,
gas and refund transitions. Do not publish bulk trace files or put them in Git.

| Phase in filename | Content |
| --- | --- |
| `admission_execution` | Opcode steps from the selected source simulation and its nested target sessions, with `chain_id`, address, call depth, gas and refund values; final source and L1-target gas totals. |
| `admission_budget` | The selected group's PostBatch projection before/after admission, entry pin and discount, previous-queue cleanup, calldata, target and companion terms, and the resolved caps. This is a calculation, not a PostBatch EVM execution. |
| `preflight_probe`, `preflight_final`, optionally `preflight_fallback` | Opcode steps from the complete ordered L1 bundle at each signed-gas probe. `tx_index=0` is PostBatch; following indices are inbound companion transactions. A `transaction_result` records exact gas for each. |

The `address_match` record names why an address-mode source was retained.
Every retained admission trace and final bundle trace contains all frames,
including proxy, queue, and refund work outside the selected address. A source
that calls the address only during final settlement and not during admission
cannot be selected retroactively. Ordinary transactions forwarded to the source
chain's mempool are outside Composer tracing.

If a source is deferred or excluded before final preparation, only admission
files may appear. An accepted source can be recomposed, so a selected hash may
produce more than one set of files; match their parent hashes and timestamps.
An output error logs `gas trace is incomplete` and does not change settlement
behavior. The selected execution incurs synchronous tracing and file I/O and
may miss its target slot; use a controlled window and leave the selector unset
for normal operation.

## Compare the three bridge cases

Use three cases: inbound, outbound, and a mixed inbound/outbound pair in one
Sync block. Select both signed source hashes for the mixed case, or select the
common bridge address before sending all three cases. This records
each source's admission separately and the whole final bundle once. The
preflight context lists all bundle source hashes. Existing process coverage includes
[`mixed_direction_state_chain_in_one_slot`](../../crates/eez-node-e2e/tests/chained_interstate.rs)
and single-direction cases in
[`cross_chain`](../../crates/eez-node-e2e/tests/cross_chain.rs). These tests do
not yet generate a three-case gas-trace artifact.

Record each case's source hash, L1 parent hash, previous queue length/result
sizes, accepted entry shapes, projected gas, selected final preflight gas,
PostBatch hash, and canonical receipt gas. The old queue changes between Sync
blocks, so compare isolated runs from the same parent when measuring the effect
of direction alone. A sequential run is useful for measuring queue reuse but
must retain each parent as part of the evidence. Do not sum the trace's
per-opcode gas costs: `CALL` forwards gas into child frames, and intrinsic,
precompile, and refund adjustments do not all appear as ordinary opcode rows.
Use `transaction_result` and the receipt as whole-tx totals. After inclusion,
the embedded L1 RPC's `debug_traceTransaction` can provide an independent
canonical trace for the PostBatch hash.

## Disable and verify

Remove the selector and directory settings and use the normal restart procedure
for that deployment. Confirm Composer starts without them and creates no new
files. Keep the selected trace directory only as long as its evidence is needed.
The [live Chiado three-case run](../audits/2026-09-24-chiado-bridge-gas/README.md)
used hash selection; address selection needs its own live validation before
making performance claims.
