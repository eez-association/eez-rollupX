# Native derivable operations fixture

Captured on 2026-09-16 from the passing `eez-node-e2e` process test
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`, using the in-progress derivable-operations implementation
originally based on `ddf40bc` and protocol pin
`3c0cdf4955941a5c23639346932b55b33babebbd`. This is a public disposable devnet;
all signing/system identities are deliberately public Testkit fixture keys.

`request.protobuf` is the complete length-delimited `prove.v1.ProveChunk` stream
for blocks 87..=91: native physical blobs, exact blocks and augmented execution
witnesses, including observation/removal-closure nodes. `response.json` retains
the original request Keccak hash, byte count, public-input digest and signature.
The remaining JSON files pin the L2 execution rules and L1 genesis/profile and
public identities. `sha256.json` identifies every retained input file.

The capture occurred before main alignment and subsequent signer hardening; it
is an immutable input/original-output oracle, not evidence that later binaries
were deployed. A successful captured request alone does not establish that its
particular posting attempt landed. The whole native scenario passed; the signer
regression independently replays the request through the current shipping path,
without the projected-format test oracle. Since the RollupX V0 wire revision,
the test first verifies the capture's original digest/signature, then repacks its
ordinary transactions/empty spans, unchanged terminal, entries and semantic messages using V0.
The current signer must replay the original blocks/witnesses and attest the new
independently recomputed DA digest. The narrow capture adapter is test-only;
shipping code does not accept tag 3. Captured files remain unchanged.

Run the current regressions with:

```sh
cargo +1.95.0 test --locked -p eez-proof-signer service::tests::derivation::captured
```

To capture new requests, use `EEZ_TEST_PROVE_FIXTURE_DIR` with the existing native
`cross_chain` test above. The proxy retains only successful requests, capped at
16 MiB each. Run the ignored `inventory_native_proving_fixtures` helper explicitly
to select the effect-bearing request; it prints metadata rather than witnesses.
New captures have new timestamps, contract addresses and hashes. Never regenerate
these files merely to make a failed current replay pass.

The current service test harness adds the locally computed
`expected_validation_identity` to the decoded RPC header. This configuration
agreement field did not exist when the immutable stream was captured; it does
not alter blocks, witnesses, PostBatch calldata, blobs or the signed digest.
Identity rejection tests bypass that fixture helper and send the raw header.
