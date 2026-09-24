# Chiado bridge gas: admission, final preflight, and receipts

| Field | Value |
| --- | --- |
| Date/status | 2026-09-24 UTC; live measurement complete for one inbound, one outbound, and one combined Sync block |
| Source | `eduadiez/rollupX` PR #34, commit `5d9e04e93900684ecb123b0899500edc0d0e87de` |
| Protocol | Pinned `eez-core-protocol` `820e50f749f63165be6a5797d4c5fe52cbbbbe34`; no protocol edits |
| Node image | Local `eez-node:pr34-dualtrace-8af218d`, image ID `sha256:a86a08a87f7e022ed1aaa8f4015e65da61a0d2fdd220c9f6a2aa50ba8a7078e6`. Its `io.eez.source.patch.sha256` label `e71f884ff483d0ac43c025166eff0907eb8ad8a9bf7f72fcc5fafbe0f049c9e3` equals `git diff 8af218d..5d9e04e | sha256sum`. Release build; signer stayed on `eez-proof-signer:pr34-gas-trace-aeaf077`. |
| Environment | Existing Chiado L1 (chain 10200) and EEZ L2 (6291), existing Composer/datadirs and signer. Public fixture sender from `scripts/xchain-test.sh`; no funding or deployment transaction. Each bridge transferred 10,000,000,000 wei to a fresh recipient. |

## Measurements

All values below are gas units. `postBatch projection` is the admission reserve after accepting the selected group(s). `Final used / required` comes from the ordered L1 bundle preflight: `required` includes gas that must be available before refunds or a calldata floor. Receipt values were read independently with `eth_getBlockReceipts` from the canonical L1 block. The source receipt for an outbound bridge is on L2.

| Case | Source transaction(s) | Admission source simulation | postBatch projection | Final postBatch used / required | Canonical postBatch receipt | Companion or source receipt |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Inbound alone | `0xd206b4ad855ea38dfa47111ac61954b78d73026b0ab312b8cf560adfe7fcbf0a` | 327,959 | 923,156 | 867,782 / 1,084,727 | 867,782 | L1 companion 398,895 |
| Outbound alone | `0x463c655fa3b7a04caa6750ff897a4deec182ae069db8fddcafd9726e8f4c0ab3` | 327,786; L1 target simulation 57,114 | 666,742 | 212,199 / 250,599 | 212,199 | L2 source 398,452 |
| Inbound and outbound in Sync block 51,580 | inbound `0x1372c69aa7f01f1bec3b48cccb38f6a587346a3446631cd292fe13eab00570af`; outbound `0x280d6bd8805eeb0eed957f396a0bda8f15afeac50c0804d83af1e511d45bfc18` | inbound 327,959; outbound 327,786; outbound L1 target 57,114 | 843,910 after both | 401,594 / 401,594 | 401,594 | L1 inbound companion 398,895; L2 outbound source 398,452 |

The combined preflight has `source_hashes` containing both hashes and two L1 transactions: postBatch 401,594, then inbound companion 398,895. The L2 outbound source receipt is in block 51,580. L1 block 23,205,628 (`0xed1b65ac6bf2d561d5fcd57e7deb947bb9b3403b0bb8a3098aa72efce41a8dc1`) contains successful postBatch `0xbc4b75b3a8f96b7b69cc7139cd760ebd273eb69a6fd0703e1f4e1b2be3a512b5` and companion `0x1372c69aa7f01f1bec3b48cccb38f6a587346a3446631cd292fe13eab00570af` with exactly those gas charges. The combined admission reserve exceeds the final **required** gas by 442,316 (110.1% of required). It is below the 13,381,013 planning cap, so gas admission did not reject this pair.

The final preflight used gas equals the canonical receipt gas in all three cases. The large discrepancy is in the **earlier admission estimate**, not in the final ordered L1 execution. The inbound source simulation goes the other way: it underestimates the executed companion by 70,936 (21.6% of its simulation). Comparing only receipt gas to admission reserves would hide a separate risk: the inbound-only postBatch required 1,084,727 before refunds, 161,571 more than its 923,156 projection. Final bundle validation caught this before signing.

### Where execution diverges

For the combined case, the inbound admission execution and final companion share their first 1,313 opcode positions (opcode, program counter, depth, address). At the bridge `CALL` at program counter 1,934, admission records the cross-chain effect and returns from the intercepted call; final preflight enters the proxy/registry path. Admission records 1,340 steps; the corresponding final companion records 4,588. A canonical `debug_traceBlockByNumber` call trace for L1 block 23,205,628 shows the companion's nested proxy call consumed 68,436 gas. That call plus 2,500 net surrounding gas accounts for the 70,936 difference. These are measurements for this transaction and state, not a universal correction factor.

The canonical call traces for the isolated and combined cases both show 68,436 gas in the inbound proxy call. They both show 35,286 gas in the outbound postBatch target call. This repeatability supports a shape-specific estimate for this fixture, but it does not establish a state-independent multiplier.

The combined postBatch reserve used one 370,000 entry pin for each group, with a 140,000 shape discount on the inbound deferred entry. The outbound group moved the projection from 181,000 to 611,742; inbound moved it to 843,910. The executed postBatch used 401,594. The entry pins are conservative upper bounds for contract shapes and proxy states, while the final preflight executes their actual sequence. A fixed percentage discount would also mishandle refunds (`used` versus `required` above), proxy creation, calldata, queue cleanup, and other batch contents.

The three cases settled different amounts of preceding L2 history: 1,460 blocks in the inbound-only post, 110 in outbound-only, and 125 in the combined post. Their postBatch receipt totals cannot be subtracted to isolate a deterministic per-bridge grouping discount. The paired admission reserve and final preflight in the **same** combined attempt provide the valid 442,316 comparison.

## Selection behavior and operations

Three earlier mixed broadcasts settled in separate Sync blocks. The first two were affected by source timing. In the third, both hashes were held before the Sync slot and the held pool reported two candidates, but the log recorded `selection_limit=1` and accepted one. Automatic admission starts with one group until it learns a preparation sample; restarting the node for each exact hash selector reset that in-memory sample. This is a scheduling limit separate from gas admission. For the successful mixed measurement only, `EEZ_MAX_USER_TXS_PER_BUNDLE=2` selected both candidates; gas/blob and final execution checks remained active.

Only the existing `eez-node-chiado` container was recreated. The signer, wallets, L1/L2 datadirs, genesis, and contracts were preserved. A private copy of the operator `.env` was made in `/tmp` before the image replacement. After the measurement, `EEZ_MAX_USER_TXS_PER_BUNDLE=auto` and both trace flags were restored to empty, and the node was recreated again. Its image ID matched the one above, restart count was zero, L2 RPC responded, and the cross-chain queue showed zero queued and zero in flight. The newer PR node image remains installed. The trace files remain in the existing L2 datadir and in the ignored local evidence directory; they are not committed.

## Evidence and verification

| Evidence | Result / location |
| --- | --- |
| Full opcode traces | Local only: `artifacts/chiado-gas-real-numbers/{inbound-1790241390,outbound-1790241863,mixed-1790242309}/traces/` in this checkout; private mode. Unsuccessful mixed attempts also retained locally under `mixed-1790241968`, `mixed-1790242068`, and `mixed-1790242193`. These ignored files are unavailable from another checkout. |
| Combined admission source traces | Inbound SHA-256 `7ecd085d4d11481abc82b469dc06d602ae75e8a565e77da65c054a59bae50d8d`; outbound `dce2b04e866f015bcac82b369035c846db81093ee4e376bd58bf93a8d1fb4e57`. |
| Combined final ordered preflight | SHA-256 `d929cf02c9d74a59816a5fb8dec3732944eeb73c65b2d5ac858df9479a00ae89`; 26,799 opcode steps, complete (`truncated=false`). |
| Single-direction final preflights | Inbound SHA-256 `cc51f4527f9c1327e4ce1f99e1101bfe28b654f3e463ca12d53ce863178a66c4`; outbound `77fe37dbc8399d420b6266d7aa82e958571921829b0a5d712c14b4e2146302dc`. |
| Canonical L1 receipts | `eth_getBlockReceipts` for L1 blocks 23,205,522, 23,205,542, and 23,205,628, each with successful postBatch status and the gas in the table. Historical `eth_getTransactionReceipt` by hash later returned `null` for some older transactions on the local RPC, although their canonical blocks and block receipts remained available. |
| Canonical call trace | `debug_traceBlockByNumber("0x16216fc", {"tracer":"callTracer","timeout":"30s"})` on local L1 RPC; postBatch gas 401,594 and companion gas 398,895. `debug_traceTransaction` by hash returned `transaction not found` after the node restarted, so the block method was used. |
| Code checks for trace selector change | Composer unit tests: 228 passed, 1 pre-existing ignored; Chiado config tests: 13 passed; settlement config check, Rust formatting, workspace Clippy with `-D warnings`, and `git diff --check` passed before commit `5d9e04e`. |

Executed from the PR checkout root: `docker compose -f docker-compose.chiado-node.yml -f docker-compose.chiado-postings.yml up -d --no-deps --pull never eez-node` for each selector change; `eth_sendRawTransaction` to the existing L1/L2 front RPCs with locally signed fixture transactions; `eth_getBlockReceipts` for `0x1621692`, `0x16216a6`, and `0x16216fc`; and `debug_traceBlockByNumber` with `{"tracer":"callTracer","timeout":"30s"}` for those blocks. The trace JSONL files were parsed by `kind` and `tx_index`, and `sha256sum` was run on the retained files. All listed transactions and postBatch receipts had status `0x1`. The experiment did not run an independent deriver replay.

This was a live receipt/preflight comparison, not an independent semantic replay or a capacity benchmark. It does not establish how many additional transactions a Chiado block can safely accept. A follow-up estimator should use the measured parent state and whole ordered candidate bundle where practical, retain pre-refund `gas_required` and a margin for uncertain state, and keep final validation. It should be tested across warm/new proxies, inbound/outbound order, varying historical ranges, refunds, and transaction counts before changing admission limits.
