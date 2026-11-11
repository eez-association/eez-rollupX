# Deployed networks

Each directory records a running network by `<l1-network>-<l2ChainId>`. Its
`genesis.json` is the exact chain document the network's nodes use as
`--chain`, copied byte for byte from a running deployment and checked against
that network's block 0.

These files are history. A protocol pin bump or an in-place contract upgrade
never changes them: an upgrade replaces implementations behind proxies after
genesis, so block 0, its state root and its block hash stay fixed. The base
`genesis.json` and `genesis.dev.json` at the repository root are templates for
**new** deployments only.

Verify a recorded genesis:

```bash
# State root of the file; must equal block 0's stateRoot on the network.
cargo +1.95.0 run --quiet --locked --package eez-node --example genesis_state_root -- \
  deployments/<network>/genesis.json
# Block 0 as served by the network.
cast block 0 --rpc-url <l2-rpc> --json | jq '{hash, stateRoot}'
```

Do not edit a recorded `genesis.json`. Record upgrades and other post-genesis
changes in the network's `README.md`.
