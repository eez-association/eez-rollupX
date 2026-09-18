# Native prepared-window checkpoint

This checkpoint replaces the earlier conclusion that the unchanged one-million
witness-item quota prevented the largest native windows. The quota remains
unchanged. Witnesses are sent in independently bounded preparation streams of at
most 64 blocks; final proving sends one cached ordinary range and one fresh full
terminal witness.

On the retained Ethereum corpus, all checkpoints succeeded. The largest legal
K=5 endpoint contained 65,535 total blocks, encoded 3,442 operation bytes in one
blob versus 50,139,220 full-block bytes, and reproduced public-input digest
`fb407cba0298901fb567361b2c18598bf1332b7d6c5d563441c8182b761c1a70`.
The final-source rerun after the atomic lease and bounded backlog/branch catch-up
changes took 38,565 ms for maximum preparation plus proof. Its 20 warm final
requests reproduced the same digest and signature at 178 ms median, 188 ms p95,
and 192 ms maximum. The 43,200 checkpoint's preparation plus proof took 24,705 ms
in the final parallel run and its final proof took 137 ms.

On the corrected Gnosis residue-1 corpus, the legal maximum contained 65,536
total blocks, encoded 3,447 operation bytes in one blob versus 50,138,637
full-block bytes, and reproduced digest
`0aaec8bbdd4eedc533aa98e637c4d2474fc563f458b7c52dd994ae5133e5ba02`.
The final-source rerun took 39,167 ms for maximum preparation plus proof. Its 20
warm final requests reproduced the same digest and signature at 185 ms median,
198 ms p95, and 202 ms maximum. The 43,196 checkpoint took 24,744 ms including
preparation and its final proof took 134 ms. Its terminal hash remained
`0x887cdcc502b76f0148d25cbe015ac19096d2ab933ee9f36628802c72379afd03`.

The `request_bytes_generated` and `witness_items_generated` fields in these runs
are cumulative across all preparation RPCs plus final proof; they are not a
single-request quota. Every individual preparation stream remained within the
existing byte/item/message limits. The final request contained no ordinary
witness vectors and always carried the terminal witness.

Commands used the retained local-only corpora and the source-built
`derivable_operations-faac0c29462f6c6c` harness with a fresh standalone signer:

```sh
EEZ_TEST_PROOF_SIGNER_BIN="$PWD/target/soundness-validation/debug/eez-proof-signer" \
EEZ_TEST_RETAINED_NATIVE_HISTORY=/tmp/derivable-native-history-retained/2562469 \
./target/soundness-validation/debug/deps/derivable_operations-faac0c29462f6c6c \
  retained_native_history_cold_checkpoint --ignored --nocapture --test-threads=1

EEZ_TEST_PROOF_SIGNER_BIN="$PWD/target/soundness-validation/debug/eez-proof-signer" \
EEZ_TEST_RETAINED_NATIVE_HISTORY=/tmp/derivable-native-history-retained/2883187 \
./target/soundness-validation/debug/deps/derivable_operations-faac0c29462f6c6c \
  retained_native_history_cold_checkpoint --ignored --nocapture --test-threads=1
```

The reruns exercised exact-range retention and explicit release between the six
independent prefix checkpoints. Signer SHA-256 was
`3417e9d699d3da9ee70762252aee200ed5416f64998f7603626efce1f448debc`.
Harness SHA-256 was
`53c88d2b2527e7ec17767f8b4866332aa10f606fba50cb930e319b75f955aa3b`.
The exact final-source values are also retained in the compact
[structured result](native-final-retained-checkpoint.json).
The retained corpora remain local-only and must be made retrievable or regenerated
before this is portable release evidence.

The live producer-prepared checkpoint then generated genuine 65k-block histories,
proved all six required logical targets with zero cache misses, posted only the
maximum as a real blob transaction, and replayed every maximum-window block on a
clean follower without sequencer headers. Ethereum reached a legal 65,536-block
endpoint with 3,444 operation bytes versus 50,139,718 full-block bytes. Its warm
final proof was 176/204/205 ms median/p95/max. Gnosis reached 65,535 blocks with
3,446 bytes versus 50,138,245 bytes and 172/189/192 ms warm final timing. Both
used one blob and completed in about 1,258 seconds including 1,094 seconds of
history production.

The compact [live result](native-live-prepared-checkpoint.json) records exact
binary and local artifact hashes plus sampled peak RSS. The larger per-process
peak in each profile was the Composer (7.37 GiB Ethereum, 7.55 GiB Gnosis),
followed by the clean follower (3.75/3.87 GiB). Signer peaks were 446/509 MiB.
No measured process swapped. Process I/O counters do not account for mmap
writeback, so they are not used as a disk-write claim.

This closes maximum prepared proof, 20 warm repetitions, actual L1 post and clean
follower replay for the two supported native profiles on the recorded host. It
does not by itself establish maximum-backlog signer restart after cache loss,
pruned branch-switch recovery, or a distinct live K=6 deployment profile.

After the final lease-renewal, restart-seeding and watermark-order fixes, current
binaries repeated the real post plus clean-follower path at smoke scale: Ethereum
posted/replayed 60 blocks with 3,438 operation bytes and Gnosis posted/replayed 65
blocks with 3,442 bytes. Both used producer-prepared references, one blob, a fresh
terminal and zero cache misses. These final-source smoke runs complement, rather
than replace, the earlier measured maximum live runs.
