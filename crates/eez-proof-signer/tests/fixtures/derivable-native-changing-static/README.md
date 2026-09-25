# Position-bound changing static reads fixture

Captured on 2026-09-23 from the passing source-built process test
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay`.
Source baseline: `1832f970884ca9460646a4cde87cca70eaffb3e1` plus the local
feature/events port and preserved settlement/timing changes. Protocol:
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`. Rust 1.94.0, Foundry 1.7.1.

The transaction performs two identical reads of an L1 value initially 5,
a successful cross-chain write to 79, then two identical reads returning 79.
The native sidecars contain one mutable row and four static rows at positions
`0, 0, 1, 1`. One complete host group contains all five calls. The process test
checks canonical settlement, the read sum 168, one completed write, and an
independent follower's matching block hash and state reconstructed from L1/blob
data. This was a fresh disposable local network, not the live Chiado deployment.
No database was copied.

The immutable `request.protobuf` is the original length-delimited proving
stream (with references to already prepared ordinary blocks) for blocks 1
through 42. `response.json` retains the original request hash, length,
public-input hash and signature. `cold-request.protobuf` expands those references using the
exact successful preparation witnesses, following endpoint and parent hashes.
Its header and terminal witness are unchanged, and its byte count/hash are also
recorded in `response.json`. The signer fully re-executes this cold stream; no
cached validation facts are imported from the original process. JSON inputs pin
the chain rules and public Testkit identities;
all keys used by this fixture are deliberately public test keys. `sha256.json`
identifies the retained input/output files. The signer tests replay actual
execution, verify the original digest/signature on retries, and reject changes
to sidecars, positions and complete-group boundaries without signing.

Source-built executable SHA-256 values:

- `eez-composer`: `e30cf5a1211529c53cbe9f36c1d847439031ede2316eb88e916463c7410aabac`
- `eez-proof-signer`: `53c9e2ff26af19d8774a3b9749e8ae07b5e9b651c7916e2678effdca852a17e0`
- `eez-follower`: `7c2495de7e89858a33801a581151c6b2ad74e10cdf88396185c9be2a6fa76c32`
- `cross_chain`: `4d7a06d74202230495a316a70c42de110b1f85b1bea1af88630a42e40d884769`

Run from the fork root:

```sh
cargo +1.94.0 test --locked -p eez-proof-signer service::tests::derivation::captured
```

The old `derivable-native-mixed` capture is retained unchanged as an old-selector
rejection test. This capture is additional evidence, not a regeneration of that
oracle. Raw process logs are local-only under
`artifacts/events-protocol/capture-complete/`; the retained fixture is portable.

The first provisional capture omitted the preparation witnesses and could not
be replayed from scratch. It and the failed fixture-check log remain under the
local `incomplete-changing-fixture` / `signer-tests-incomplete-capture.log`
evidence paths. This complete capture was taken only after fixing the exporter
and repeating the passing process test; it does not replace any old-protocol
oracle.
