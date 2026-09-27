# Reth dependency ports

Reth itself is unmodified upstream commit
`444e867155c2a7a4c028e479ca7201b0b6871579`, containing
[#27429](https://github.com/paradigmxyz/reth/pull/27429). Its package version is
still 2.5.2; the commit identifies the tested post-v2.6.0 source.

| Directory | Upstream | Base revision | Local changes |
| --- | --- | --- | --- |
| stateless | [eez-association/stateless](https://github.com/eez-association/stateless) | `4fc3806bdd0e6b296c761ef4d4b260938365cf45` | Dependency/API alignment; batch trie updates and storage resets; retain transaction checkpoints |
| reth-gnosis | [gnosischain/reth_gnosis](https://github.com/gnosischain/reth_gnosis) | `3482cbe6ed962830d71b154773c9209267edf1dd` | Dependency/API alignment; retain interpreter features for the custom Gnosis EVM |
| gnosis-primitives | [gnosischain/rust-primitives](https://github.com/gnosischain/rust-primitives) | `6fd87e36685bc2b5b11136c5d591aa195095595f` | Match Reth/REVM/Alloy types |

Cargo builds these local sources. The [Stateless](patches/stateless.patch),
[Reth-Gnosis](patches/reth-gnosis.patch) and
[Gnosis primitives](patches/gnosis-primitives.patch) patches show retained
source/manifest differences from their bases; lockfiles and packaging/docs are
excluded. Check them from the repository root:

```bash
git apply --reverse --check vendor/patches/*.patch
```

Runtime sources, manifests, tests and supplied licenses are retained. Upstream
GitHub workflows, Gnosis deployment/vector-generation scripts and image build
files, benchmark reports, and the two Gnosis lockfiles are omitted. EEZ uses the
root `Cargo.lock`. The three Stateless crates are workspace members, so their
library tests run with EEZ. Standalone EF/RISC-V tooling is omitted; checkpoint
and trie regressions remain in the workspace.
Upstream operational instructions remain available at the revisions above.

The Stateless trie port incorporates `923f1b759c916b2b5e42c26fa3a55a8d0b820bc3`
from upstream Stateless. Storage resets must precede checkpoint and final roots;
regressions retain blinded-storage and independent full-state-root oracles.
Gnosis wraps upstream EVM state reads while retaining full providers for roots
and proofs. None of these adapters changes Reth source or chain rules.

EEZ maintains these ports until exact compatible upstream revisions can replace
them. Preserve transaction checkpoints, destroyed-storage handling and their
regressions when updating. Version compatibility alone does not establish
validation equivalence. See the [migration audit](../docs/audits/2026-09-26-reth-27429/README.md).
