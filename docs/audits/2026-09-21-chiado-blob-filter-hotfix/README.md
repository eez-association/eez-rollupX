# Chiado unrelated-blob filtering hotfix

| Field | Value |
| --- | --- |
| Date and status | 2026-09-21 UTC; live hotfix validated, source fix prepared for review, deployed image remains local |
| Scope | Diagnose the sustained Chiado node/RPC outage, prevent unrelated beacon-slot blobs from being decoded as EEZ blobs, build an image from the deployed revision, replace only the node service, and verify recovery and settlement |
| Source | Working repository `fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`; deployment build checkout `7ff1cf4eb5ef528b15b8bc3f469394e4977f6506`; patch SHA-256 `53ef3eb4c50c7951dcb58110415c321b6083cc0bf9f27910203a1337bed493e8` |
| Protocol | Pinned and initialized deployment submodule `3c0cdf4955941a5c23639346932b55b33babebbd`; no protocol changes |
| Executables | Composer SHA-256 `223b8d079ebfce4cd9a9cc011ebe5daa46e7d5318714e64282788496f1e6e86a`; local image `eez-node:blob-filter-hotfix-7ff1cf4`, image ID `sha256:ba6aebd9e9dee564873b8f655916ad8f0fe46871736130b1da86f755435e7ac4` |
| Environment | Rust 1.94.0 build tools image; Docker Compose project `rollupx`; Chiado chain ID 10200; EEZ L2 chain ID 6290; existing node datadirs, Lighthouse, and proof signer retained |
| Related records | [Chiado deployment runbook](../../runbooks/chiado-deployment.md) |

## Findings

Chiado L1 block `23163027` and beacon slot `24920909` contained two blobs. The
first belonged to the EEZ postbatch transaction. The second belonged to an
unrelated transaction and was a valid EIP-4844 blob, but it did not satisfy EEZ's
stricter 31-byte-per-field-element packing rule.

The beacon response parser converted every blob in the slot to the strict EEZ
`Blob` type before comparing versioned hashes. The unrelated second blob therefore
failed at field element 96 and terminated the L1 watcher. Startup catch-up retried
the same block and produced a deterministic restart loop. Both public RPC paths
returned 502 while the node was unavailable.

The fix computes KZG commitments and versioned hashes over generic, physically
valid EIP-4844 blob bytes. It applies strict EEZ decoding only after a blob matches
a versioned hash committed by the expected postbatch transaction. The strict rule
still applies to every selected EEZ batch blob.

## Evidence inventory

| ID | Contents / provenance | Location and digest | Availability / retention |
| --- | --- | --- | --- |
| E1 | Source fix and regression | `crates/eez-blob-codec/src/{lib.rs,transport.rs}`, `crates/eez-l1/src/blob_source.rs`; patch SHA-256 `53ef3eb4c50c7951dcb58110415c321b6083cc0bf9f27910203a1337bed493e8` | Included in this change |
| E2 | Release Composer built from deployment revision plus E1 | SHA-256 `223b8d079ebfce4cd9a9cc011ebe5daa46e7d5318714e64282788496f1e6e86a` | Local build cache and local image only |
| E3 | Thin runtime image based on `eez-node:fee-recipient-7ff1cf4` image ID `sha256:331e14b24f6dde1b7bd4a703aa426f6c2ad9c8643b2f2ec9c0567ad12c34f8d0` | Image ID `sha256:ba6aebd9e9dee564873b8f655916ad8f0fe46871736130b1da86f755435e7ac4` | Local Docker daemon only |
| E4 | Live canonical postbatch after recovery | Transaction `0x074ff9f1fa2b3575e1e92a5933cb063b19b293d422fb91a5171fd84bf849f2aa`, Chiado block `23163526`, L2 endpoint `258380` / `0x7fa885db8a675a0d8bb8620cfa4c430880f1b5faa316051c10becc9edddb44d8` | Public chain and local container logs |

## Reproduction and executed checks

| Requirement / question | Production path or check | Evidence type | Result and evidence |
| --- | --- | --- | --- |
| Reproduce the outage | Existing image processed Chiado block `23163027` / slot `24920909` | Live | Failed repeatedly with `blob 1 field element 96 has a non-zero most-significant byte`; restart count reached 59 |
| Confirm blob ownership | Compared the two slot blobs with the postbatch transaction's committed versioned hash | Live/static | Blob 0 matched EEZ; blob 1 was unrelated |
| Preserve strict EEZ decoding | Focused codec and L1 unit suites | Unit | `cargo +1.94.0 test --locked -p eez-blob-codec -p eez-l1 --lib`: 21 and 129 passed |
| Regress the incident shape | `hydration_ignores_unrelated_non_eez_blob_in_same_slot` in the exact deployment checkout | Unit | 1 passed, 137 filtered out |
| Check protocol codec regressions | `cargo +1.94.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec` | Unit/independent vectors | All selected tests passed: 167 protocol tests plus vector targets, 21 blob-codec tests plus the KZG oracle, and 15 payload-codec tests |
| Check workspace lint and formatting | `cargo +1.94.0 fmt --all --check`; `cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings`; `git diff --check` | Static | Passed |
| Verify packaged executable | In-image `sha256sum` and `ldd` | Static/runtime | Digest matched E2; all shared libraries resolved |
| Recover the original failure point | Start E3 with the retained production datadirs | Live | Passed block `23163027`; no recurrence of the blob parsing failure; node remained at restart count 0 |
| Restore RPC service | `eth_chainId` and `eth_blockNumber` through local and public L1/L2 routes | Live | L1 returned chain ID 10200; L2 returned chain ID 6290; both public heads advanced |
| Restore settlement | Canonical `BatchPosted`, receipt, and `bundle.observed` events | Live | E4 included successfully, advanced the anchor from L2 block 257440 to 258380, settled 940 L2 blocks and 18 transactions |

The live check validated canonical execution and derivation advancement. It did
not run an independent follower replay. A later catch-up proof request reached the
configured proof-attempt cutoff and retained the complete range. The next included
postbatch advanced the anchor from 258380 to 260380 and settled 2,000 blocks and
118 transactions; further postbatches continued to land afterward.

## Operational changes

The existing authorization covered building a local replacement image, replacing
the node, and testing the fix. Only Compose service `eez-node` was force-recreated.
The existing node datadirs, network, Lighthouse service, proof signer service, and
the prior runtime image were retained. The operator `EEZ_NODE_IMAGE` setting now
selects `eez-node:blob-filter-hotfix-7ff1cf4`, so ordinary Compose operations do
not revert the node. Rollback remains possible by selecting
`eez-node:fee-recipient-7ff1cf4` and recreating only `eez-node`.

## Open work and superseding evidence

| Unmet criterion / uncertainty | Impact | Next concrete check or action |
| --- | --- | --- |
| The deployed image exists only on this Docker host | The live recovery will not survive loss of the local image | Merge E1, run CI, publish an immutable tested image, and replace the local tag with its digest |
| Independent follower replay was not run | Live Composer recovery does not independently establish replay behavior | Run the affected follower/process test lane or observe an independently deployed follower at and beyond Chiado block `23163027` |
