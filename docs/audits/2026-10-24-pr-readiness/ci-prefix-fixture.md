# Blob-evidence example fixture correction

[CI run 40098162224](https://github.com/eez-association/eez-rollupX/actions/runs/40098162224/job/120884663004)
passed shipping compilation and the release archive dependency guard, then failed
two of its 30 producer tests. Both failures occurred in
`verify_blob_evidence/prefixes.rs:228`, before the replay assertions, while
constructing an inconsistent private-L1 genesis. The original CI result remains
**28 passed, 2 failed**, with publication and downstream lanes skipped.

The fixture put genesis and its L1 reference at the same timestamp `T`, then
selected a one-second/four-position schedule. This requires the reference to be
at or before the bootstrap cutoff `genesis - 4`, so production correctly rejected
it. The fixture also retained the production base's history storage, whose
cutoff/interval/delay were `12/12/6` rather than the fixture's calendar.

Only the example's `#[cfg(test)]` module changes: genesis is `T + 4`, its reference
is exactly the bootstrap cutoff, and the existing `bootstrap_storage` helper
installs the matching next cutoff, interval and delay. The positive test checks
the exact supplied genesis survives unchanged and asserts the concrete calendar
and first child timestamp. Negative cases independently reject a reference after
the cutoff and incorrect next-cutoff, interval and delay slots. Existing
rejections and all production validation remain intact.

Before editing, the complete example target reproduced the same two failures:
**10 passed, 2 failed**, no ignored or filtered tests. The identical command after
the correction passed **13 tests**, with no ignored or filtered tests:

```bash
cargo +1.95.0 test --locked --offline -p eez-l1 \
  --example verify_blob_evidence --message-format=json -- --test-threads=1
```

The baseline build took 65 seconds and the incremental build 5.18 seconds. Both
used one job, LLVM 22 and a private temporary directory; minimum available memory
was 15.55 and 15.86 GiB respectively. Both executables and first-failure logs are
retained locally. Exact source, executable and log hashes are in the
[portable record](ci-prefix-fixture.json). Independent static review confirmed
the fixture and the specificity of its negative assertions. Workspace format
and whitespace checks passed.

This is a local debug regression, not a successful corrected release producer.
Earlier `cargo test --workspace` and CI's lib/bin unit lane did not execute the
example unit tests; their passing totals did not cover this fixture. No runtime,
genesis fixture, lockfile, feature, protocol pin or archive guard was changed.
