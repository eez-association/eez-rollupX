# Main alignment, 2026-09-16

The implementation worktree `target/worktrees/derivable-chain-operations`, branch
`feat/derivable-chain-operations`, was fast-forwarded from
`ddf40bc2eb5fac8f7973b112eb383256cd8aac74` to freshly fetched fork `origin/main`
`5469c88860408daf776ee812156dd280c1a9e262`. The protocol gitlink remains
`3c0cdf4955941a5c23639346932b55b33babebbd`.

Main contains the merged Composer recovery/admission fixes and repository agent
guidance. Both overlapping files, `composer/admission.rs` and `composer/policy.rs`,
merged without conflicts. Inspection of their diff against main confirms the
new timing jitter, preparation history and admitted-work backoff remain present.
The other 99 files in the saved implementation snapshot were verified identical
by SHA-256 after restoration. The original checkout and other worktrees were not
updated. A local recovery stash was retained at
`8b175dc76795f85407a4e309ce44029884a8f357` (local-only, not a published artifact).

Validation on the aligned working tree:

- `cargo +1.94.0 test --offline --locked -p eez-composer --lib -j3`: 207 passed.
- `cargo +1.94.0 test --offline --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -j3`:
  300 signer, 4 RPC and 18 client tests passed; signer feasibility benchmark and
  fixture inventory remain two explicitly ignored utilities. No ignored test is
  counted as validation. This run includes the new native-request regressions.
- After adding an independent reverted ordinary CREATE fixture, the complete
  signer suite passed again: 301 passed, 2 intentionally ignored utilities.
  The fixture independently specifies the failed receipt, gas and sender nonce
  transition, then proves that omitting the reverted transaction from DA rejects
  before another signing call. [Final signer checkpoint](checks/main-aligned-signer-final-checkpoint.log).
- Clippy for signer, prover client and Testkit, all targets with `-D warnings`,
  passed before that final synthetic-test addition; [log](checks/main-aligned-clippy.log).
  Signer all-target Clippy passed again after that addition; [final log](checks/main-aligned-signer-final-clippy.log).
- `git diff --check` and `cargo +1.94.0 fmt --all --check`: passed.

Logs: [Composer](checks/main-alignment-composer.log),
[signer/RPC/client](checks/main-aligned-signer-rpc-tests.log).
These are component results, not final native/release evidence. Earlier native
results do not validate newly built binaries; repeat relevant native checks after
the remaining implementation is complete. Re-fetch main again before final review.


A final HTTPS fetch on 2026-09-17 confirmed
`HEAD == origin/main == FETCH_HEAD == 5469c88860408daf776ee812156dd280c1a9e262`
with mutual ancestry against the fork remote. A later review of upstream PR #152
refreshed the distinct public `upstream/main` and found it at
`c5120e137decfc9726a59570c6eebcabf1c905c6`. The fork base and public upstream
have diverged: `git rev-list --left-right --count upstream/main...5469c888` reports
47 upstream-only and 118 fork-only commits, and neither tip is an ancestor of the
other. Therefore this work is aligned with the latest fork `origin/main`, not the
latest public upstream main; an upstream integration must choose and reconcile a
base explicitly instead of treating this branch as a fast-forward of upstream.
The historical 207/208 Composer and 300/301 signer counts above describe
intermediate checkpoints.

On the later implementation source, Composer passes 209 tests, the signer passes
317 with two explicit measurement/inventory utilities ignored, the prover client
passes 30, and Node passes 44. The final-source retained native maxima pass with
the executable identities and values in
[native-final-retained-checkpoint.json](native-final-retained-checkpoint.json).
The focused [soundness review](soundness-review.md) records subsequent renewal,
restart seeding, watermark ordering and cache-transition findings and fixes.
Final process-suite results are recorded in the coverage ledger; release-only
evidence that remains open is not represented as implementation completion.

## Fork-main rebase, 2026-09-17

The fork's live HTTPS `main` was fetched again at
`fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`, 18 commits after the preceding
checkpoint. The feature commit was rebased onto that exact tip. The fork delta
contains explorer packaging and CI-duration/test-fixture work; it does not
change the signer or protocol gitlink.

Twelve paths overlapped textually. Git merged eight without conflict. Four
configuration/document paths required manual resolution:
`.env.chiado.example`, `docker-compose.chiado-node.yml`,
`docs/configurable-settlement-policy.md`, and
`scripts/check-settlement-policy-config.py`. The resolution retains the fork's
new explorer, logging, timing, healthcheck and CI behavior while keeping the
new-network contract: mandatory settlement policy, no enable toggle, and no
`EEZ_MAX_BLOCKS_PER_BATCH` compatibility path. The fork's Chiado configuration
test was updated to assert those mandatory defaults.

Post-rebase checks passed: 241 Python CI/tooling tests with four explicit opt-in
environment probes skipped, 8 Testkit unit tests, all-target `eez-node-e2e`
checking, Chiado and Kurtosis mandatory-policy Compose rendering, Rustfmt, the
source manifest, and the diff whitespace check.

A fresh full Kurtosis run built commit
`1cb7035ef833fca446f88e6d3a4044fb52587041` after the rebase. Inbound,
outbound, mixed, mixed-pure, every state-chaining case, and all 28 protocol
scenarios passed. The independent collector reported complete accounting,
causal coverage and semantic blob authentication, with 28 signed windows,
28 remote attestations and zero L1/L2 root divergence. The owned enclave was
removed after evidence collection; see the
[validation record](../../../audits/2026-09-17-derivable-operations-kurtosis/README.md).
