# Reth v2.6.0 dependency ports

These local sources make the validation checkout reproducible without publishing
unreviewed dependency forks. Reth itself is the unmodified official release
`73a3a00862a8f14f89e30da8de001456f18cfae0`.

| Directory | Upstream source | Base revision |
| --- | --- | --- |
| stateless | https://github.com/eez-association/stateless | 4fc3806bdd0e6b296c761ef4d4b260938365cf45 |
| reth-gnosis | https://github.com/gnosischain/reth_gnosis | 3482cbe6ed962830d71b154773c9209267edf1dd |
| gnosis-primitives | https://github.com/gnosischain/rust-primitives | 6fd87e36685bc2b5b11136c5d591aa195095595f (v0.2.8) |

Original licenses are retained. Initial changes align manifests with official
Reth v2.6.0, REVM 43 and Alloy 2.4.2. The upgrade proposal tracks validation acceptance; these sources do not claim
upstream release support for the combined EEZ dependency graph.

The `stateless/crates/tries` manifest and default/lib/zeth modules incorporate
upstream Stateless `923f1b759c916b2b5e42c26fa3a55a8d0b820bc3` to use Reth's batch leaf updates and explicit storage-reset API, including its reset tests.

The Stateless validator retains the fork's transaction checkpoints and applies
storage resets before both checkpoint and final roots. Its regression compares
both paths against an independently constructed trie with blinded old storage.
Reth-Gnosis adapts the download command's return value and selects the existing
interpreter features explicitly; the default Reth binary now also selects JIT,
which is not used by EEZ's custom Gnosis EVM. Gnosis primitives follows the
matching REVM/Alloy type versions. None of these ports modifies Reth source.
