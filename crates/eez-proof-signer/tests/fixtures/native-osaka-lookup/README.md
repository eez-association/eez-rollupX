# Direct V0 caught-revert native capture

Unmodified successful Prove request/response from the first caught revert in
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`,
re-captured on 2026-10-01 after `EEZL2` moved to the upgradeable `EEZProxy`
genesis predeploy `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` (protocol
`855fe06`). The previous witnesses proved the old `0x42…07` manager, so the
current signer could not load the protocol account. The source process test
(the `cross_chain` module of the `node_e2e` target) passed in 115.01s: three
caught reverts retained their original outcomes without destination writes,
followed by a successful inbound call. This request covers blocks 1–100 and one
failed lookup entry; the run exported four requests, and this is the first with
a failed entry. L1-origin semantic transactions publish no signed L1
transaction: the bracket's `tx_data` is empty, and the signer rejects any other
value. The captured replay test asserts that only L2-origin brackets carry
`tx_data`.

`response.json` binds request sizes and digests. The cold request expands original
preparation references with exact captured witnesses. Full signer replay compares
the original public input and signature. These are producer/stateless-replay
oracles, not an independent EVM implementation.

Source: `feat/protocol-main-8d45891` `a01b34c` (the signer's own test-vector edits
were uncommitted and test-only); protocol `855fe0602484750861b4c31502a1a91355d33ed6`.
`capture.json` records the executable and file digests. The run, its logs and all
four exported requests are local-only and not retained. Capture command, from the
fork root after `forge build --root contracts` and with `EEZ_TEST_PREBUILT`,
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` unset:

```sh
EEZ_TEST_PROVE_FIXTURE_DIR=<dir> EEZ_TEST_LOG_DIR=<logs> EEZ_TEST_DATADIR_DIR=<datadirs> \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes \
  --exact --test-threads=1
```

The request was selected with the ignored
`service::tests::derivation::captured::inventory_native_proving_fixtures` helper;
`request.protobuf`, `cold-request.protobuf` and `response.json` are the exporter's
`<hash>.protobuf`, `<hash>.cold.protobuf` and `<hash>.json`, and the genesis,
chain-config and identity files are its unmodified copies.

History:

- 2026-09-30: captured at `feat/system-tx-remaining-gas-cap` `637c445` plus the
  commit adding that capture, protocol `9d13218a355e7d543ca8a9927eea2962600b716d`
  (blocks 1–115, test passed in 130.56s), after L1-origin brackets stopped
  publishing `tx_data`. Superseded by the 2026-10-01 re-capture for the
  upgradeable EEZL2 proxy at protocol `855fe06`.
- The capture before that (base `1d3ee4530205c259a54b288fd940e7b65436177d`, under
  the protocol balance burn) published a 246-byte signed L1 transaction in the
  inbound bracket and is superseded; the signer now rejects it.
