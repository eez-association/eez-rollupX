# Stateless compatibility port

This directory contains the Stateless validator, trie implementations and EEZ's
transaction-checkpoint regressions. The crates belong to the root EEZ workspace
and use its `Cargo.lock`. Source revisions and compatibility changes are listed
in [the vendor guide](../README.md).

Run from the repository root:

```bash
cargo +1.95.0 test --locked -p stateless -p tries -p zeth-mpt
cargo +1.95.0 clippy --locked -p stateless -p tries -p zeth-mpt --all-targets -- -D warnings
```

The checkpoint API preserves upstream consensus and execution validation and
returns roots at selected transaction boundaries. Callers must provide strictly
increasing, bounded indices and apply their own quotas. A transaction checkpoint
can precede post-block state changes and therefore need not equal the final root.
The [fixture](crates/stateless/tests/fixtures/README.md) compares checkpoint roots
with an independently recorded full-state oracle.

The standalone Ethereum Foundation suite and RISC-V build scripts are omitted;
see the [upstream repository at the imported revision](https://github.com/eez-association/stateless/tree/4fc3806bdd0e6b296c761ef4d4b260938365cf45)
for those workflows. The local workspace validates EEZ's supported host build.

Licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).
