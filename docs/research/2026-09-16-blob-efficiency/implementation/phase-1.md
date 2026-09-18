# Codec and reconstruction foundation

Date: 2026-09-16. Branch: `feat/derivable-chain-operations`.
Base: `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`.
This is implementation evidence for parts of packages A and B, not activation
evidence or a completed signer security review.

## Implemented

- [V3 codec](../../../../crates/eez-payload-codec/src/derivable.rs): canonical
  profile-1 wire, bounded sparse records, environment changes, exact transaction
  envelopes and full blocks, allocation-free block-plan iteration and checked
  exact whole-payload sizing. Construction validates syntax only.
- [Codec tests](../../../../crates/eez-payload-codec/src/derivable/tests.rs):
  golden field/record wire, byte-exact preservation of the legacy terminal,
  entries and groups, malformed/truncated/noncanonical inputs, transaction count
  bounds, immediate full-block ancestry, 65,536-total logical count and exact
  sizing at RLP boundaries. Four property tests run 256 cases each, including an
  independent dense model of mixed empty/transaction/full/environment records.
- [Shared driver adapter](../../../../crates/eez-driver/src/derivation.rs), behind
  the opt-in `derivation` feature: published inputs plus actual parent and trusted
  chain rules produce Reth next-block attributes. Arithmetic overflow and forks
  outside Merge through Osaka reject. Reconstruction uses the pinned Reth
  `EthBlockAssembler` with execution receipts, requests, gas, bundle and supplied
  replay state root; whole canonical block RLP equality is explicit.
- [Driver tests](../../../../crates/eez-driver/src/derivation/tests.rs): independent
  empty-block headers for Merge/Shanghai/Cancun/Prague/Osaka and each transition;
  every published environment input; false parent seals; receipt-count mismatch;
  mutations of all 23 pinned header fields; body/trailing-byte mismatches. Real
  EVM execution checks consecutive empty blocks updating EIP-4788 storage and a
  nonempty transfer. The transfer's transaction/receipt roots are also checked
  with an independently constructed single-leaf trie.

The pinned `Header` fixture names every field with no struct-update shorthand,
so a dependency adding a field forces a fixture review. These assembly fixtures
use a sentinel post-state root. They do **not** demonstrate stateless witness
authentication, trie-root verification or L1 settlement authorization.

## Verification

Toolchain: Rust 1.94.0. The lockfile adds two dependency edges only; no dependency
versions or pinned revisions change. The retained [source manifest](source.sha256)
identifies the implementation and tests corresponding to this evidence.

From the worktree, using the shared target directory:

```sh
export CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target
cargo +1.94.0 test --offline --locked -p eez-payload-codec -p eez-driver \
  --features eez-driver/derivation --lib -j3
cargo +1.94.0 clippy --offline --locked -p eez-payload-codec -p eez-driver \
  --features eez-driver/derivation --all-targets -j3 -- -D warnings
cargo +1.94.0 fmt -p eez-payload-codec -p eez-driver -- --check
git diff --check
sha256sum --check docs/research/2026-09-16-blob-efficiency/implementation/source.sha256
```

[Test output](checks/test.log): **87 passed** (37 codec, 50 driver), including
32 new tests. [Clippy output](checks/clippy.log) and
[format/whitespace checks](checks/format-and-diff.log) record the final verification.

## Integration boundaries still outstanding

At this historical phase checkpoint, `eez_payload_codec::decode` still accepted
only tags 0–2 and no production caller used the new driver adapter. The completed
implementation supersedes that boundary: the old codec was deleted and tag
3/profile 1 is the only operations format. At this checkpoint no change had yet
been made to signer source, limits, attestation types, cross-chain encoding,
producer emission or follower replay. The opt-in codec types and reconstructed
blocks carried **no authorization authority**.

Next work must bind the recipe to the actual cold proof window, authenticate the
first parent from the accepted witness/anchor, and perform reconstruction while
successful Stateless outputs are still available. The adapter's state-provider
argument must be resolved at that boundary: the pinned assembler currently does
not read it, but this is not permission to use fabricated state for execution or
root computation. Preserve full intermediate outbound/system-sender checks,
terminal checkpoints, DA/KZG/semantic binding and the private attestation boundary.

Follower integration must consume every block plan against its own authenticated
state, including empty blocks, and preserve immutable claimed ranges separately
from resumed L1-applied endpoints. Full records require actual reproduction;
they are not a blanket fallback for unsupported forks or body/header shapes.

Exact composable range sizing, composer activity indexing, cold signer/follower
equivalence, legal long-span benchmarks, resource/default changes, any necessary
cache/RPC work and coordinated activation remain open in the coverage ledger.
The maximal-count codec test is a format-capacity test, not evidence that a
65,536-block request can be proven, posted or recovered within deployment limits.
