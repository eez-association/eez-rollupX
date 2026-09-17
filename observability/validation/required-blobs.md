# Current required blob validation

Dirty worktree after `c36bbd6`, 2026-09-17. This is fixture evidence, not a claim
that an operator deployment's archive or recovery source is sufficient.
See [the source and metric contract](../contracts/required-blobs.md).

The final focused command was:

```bash
cargo +1.94.0 test --locked -p eez-l1 -p eez-deriver -p eez-observer \
  -p eez-node-common --features eez-node-common/l1-startup --lib
```

It passed **133 L1, 48 deriver, 75 observer and 16 node-common tests**. Logs:
`/tmp/eez-required-blobs-final-suites.log`. Earlier logs retain the sandbox's
loopback-bind denial and a fixed generic test-header type annotation; neither
required skipping a test or changing native behavior.

New behavioral coverage includes:

- Actual native private Beacon HTTP 410 and empty-body/expected-hash-missing
  retrievals produce exact typed dependency evidence only for applied batches.
  The same misses on unapplied posts retain ordinary source-incomplete behavior.
- Original error text, retry/nonterminal classification, and absence of
  classification for generic provider errors remain compatible.
- Repeated exact references count; a changed hash resets; success, unrelated
  owner failure and reorg clear; an idle sibling or older completed scan cannot
  erase a newer active dependency.
- Configured Watcher poll failure and reorg exercise the actual diagnostic
  attachment, not only direct cache calls.
- A real private observer HTTP fixture confirms the successful type-3 receipt,
  exact contract/reference/anchor, repeated attempts and recovery. Stale native
  age, changed diagnostic generation, reorg and already-covered dependencies
  become unknown; historical probe reasons cannot qualify.
- Expired/absent observations emit no blocked/healthy data value.

The existing `happy_case_follower_l1_derived` native canary now includes a private
Beacon fault proxy. It requires live Watcher evidence after at least three
attempts, checks the actual canonical receipt and EEZ recipient, restores body
availability, and waits for blocker removal plus safe-chain convergence before
its persisted-history restart test. The combined-source exact canary passed
**1/1 in 111.00 seconds**, with no production Rust edits during the run; log
`/tmp/eez-observability-final-follower.log`. It used source-built Composer
`b4dcdda3ef41d080e6c5d85fcc701d3bb854933894af6e4610ffa227378aa5b4`,
follower `039a610ef02519d5242ce5c13dcd1de0710cd26f214ce0f7f125ee5f10760f8d`
and signer `c5f46d1f2840dc2d5be1e27a47d01c9fbbbcf6a47c661a6e5232d34c1142c05a`.
Workspace Clippy and later signer/fixture changes remain separately coordinated.
The Grafana delivery/suppression/recovery gate is owned by the alert lane.
