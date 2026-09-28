# Reth dependency ports

Reth itself is unmodified upstream commit
`444e867155c2a7a4c028e479ca7201b0b6871579`, containing
[#27429](https://github.com/paradigmxyz/reth/pull/27429). Its package version is
still 2.5.2; the commit identifies the tested post-v2.6.0 source.

| Directory | Upstream | Base revision | Local changes |
| --- | --- | --- | --- |
| reth-gnosis | [gnosischain/reth_gnosis](https://github.com/gnosischain/reth_gnosis) | `3482cbe6ed962830d71b154773c9209267edf1dd` | Dependency/API alignment; retain interpreter features for the custom Gnosis EVM |
| stateless | [eez-association/stateless](https://github.com/eez-association/stateless) | `ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3` | Generalize recovered-block and receipt types for native transactions; expose the post-pre-execution checkpoint for terminal B[0] |
| gnosis-primitives | [gnosischain/rust-primitives](https://github.com/gnosischain/rust-primitives) | `6fd87e36685bc2b5b11136c5d591aa195095595f` | Match Reth/REVM/Alloy types |

Cargo builds these local sources. The [Reth-Gnosis](patches/reth-gnosis.patch) and
[Gnosis primitives](patches/gnosis-primitives.patch) patches show retained
source/manifest differences from their bases; lockfiles and packaging/docs are
excluded. Check them from the repository root:

```bash
git apply --reverse --check vendor/patches/*.patch
```

Runtime sources, manifests, tests and supplied licenses are retained. Upstream
GitHub workflows, Gnosis deployment/vector-generation scripts and image build
files, benchmark reports, and the two Gnosis lockfiles are omitted. EEZ uses the
root `Cargo.lock`. Upstream operational instructions remain available at the
revisions above.

Stateless is vendored from EEZ fork commit
[`ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3`](https://github.com/eez-association/stateless/commit/ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3).
Its [README and host CI](https://github.com/eez-association/stateless/tree/ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3#eez-reth-compatibility-branch)
own the checkpoint/trie regressions and port provenance. The [local patch](patches/stateless.patch) generalizes recovered-block validation
and exposes a pre-transaction state root on its opt-in checkpoint path. That root
includes pre-execution system changes and supports an independently reconstructed
empty terminal prefix. The Ethereum signature-recovery entry points remain
compatible; witness validation and final root checks are unchanged.
The source checkout's execution-spec-test workspace member is omitted, while
all library tests and fixtures are retained. Its separate test lock retains the
root runtime versions and adds the upstream serialization-test dependencies. Run them with
`cargo +1.95.0 test --locked --manifest-path vendor/stateless/Cargo.toml -p stateless -p tries`; EEZ's signer and process tests
validate native integration. Storage resets
must precede checkpoint and final roots; preserve the blinded-storage and
independent full-state-root oracles when changing this dependency.
Gnosis wraps upstream EVM state reads while retaining full providers for roots
and proofs. None of these adapters changes Reth source or chain rules.

EEZ maintains these ports until exact compatible upstream revisions can replace
them. Version compatibility alone does not establish
validation equivalence. See the [migration audit](../docs/audits/2026-09-26-reth-27429/README.md).
