# Native derivable operations fixture

Captured on 2026-09-16 from the passing `eez-node-e2e` process test
`cross_chain::minimal_bidirectional_cross_chain_smoke`, using the in-progress derivable-operations implementation
originally based on `ddf40bc` and protocol pin
`3c0cdf4955941a5c23639346932b55b33babebbd`. This is a public disposable devnet;
all signing/system identities are deliberately public Testkit fixture keys.

`request.protobuf` is the complete length-delimited `prove.v1.ProveChunk` stream
for blocks 61..=65: native physical blobs, exact blocks and augmented execution
witnesses, including observation/removal-closure nodes. `response.json` retains
the original request Keccak hash, byte count, public-input digest and signature.
The remaining JSON files pin the L2 execution rules and L1 genesis/profile and
public identities. `sha256.json` identifies every retained input file.

The capture occurred before main alignment and subsequent signer hardening; it
is an immutable input/original-output oracle, not evidence that later binaries
were deployed. A successful captured request alone does not establish that its
particular posting attempt landed. The whole native scenario passed at its original protocol revision. The
feature/events port changes both L2 system-call selectors. The current signer
regression therefore replays the unchanged execution bytes as a compatibility rejection
and asserts that no signature is produced. Positive signing and DA mutation
coverage use the separate new-protocol capture; the original oracle is retained.

The test-only V0 adapter verifies the original digest/signature, then repacks the
DA wrapper while preserving blocks, witnesses, entries and semantic messages.
This lets the regression reach the protocol-selector rejection instead of failing
on the historical operations tag. Captured files remain unchanged; shipping code
does not accept tag 3.

Run the current regressions with:

```sh
cargo +1.94.0 test --locked -p eez-proof-signer service::tests::derivation::captured
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
