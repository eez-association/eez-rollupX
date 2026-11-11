# Nonzero-delay live mixed-load validation

| Field | Value |
| --- | --- |
| Date/status | 2026-10-10 UTC. Retained workload passed independent read-only revalidation; original harness run failed. Not a clean first-pass 30-wave suite. |
| Scope | Existing Chiado canary, 15 minutes of continuous ordinary traffic alongside bidirectional cross-chain waves. No implementation, protocol, or live scheduling changes. |
| PR head | `aaf25716d84353de7f26e6b6f517667a8e0b1f61` |
| Deployed source | `76b210bcd9cc22e429865d483fa2d948c9793780`. Runtime source, Cargo dependencies, contracts and protocol are unchanged between this build and the PR head; later changes concern docs and CI tooling. These are not the latest build's binary bytes. |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, clean pinned inspection checkout |
| Configuration | L1/L2 cadence 5000/1000 ms; P=2, delay=250 ms; proof seed=750 ms; submission slack=700 ms; interval policy=900000 ms, maximum unsettled span=150 blocks |
| Related evidence | [Earlier validation and failures](final-validation.md); [settlement contract](../../configurable-settlement-policy.md) |

## Identity and operations

The user selected the existing network instead of a new Kurtosis enclave.
The newly started enclave `eez-pr79-validation-20261010` was stopped, retaining
its state. Its CLI stop was blocked behind setup; only containers selected by
its verified unique enclave ID `453c8460d6884de2bd1565218996c48e` were stopped.
The earlier debug enclave was not changed.

Live node and signer images remained:

- Node: `ghcr.io/eduadiez/rollupx-node@sha256:0cd439980f354725c7a9ea314565f49e1fafbaffa857812a41e37a9e737b5f89`.
- Signer: `ghcr.io/eduadiez/rollupx-proof-signer@sha256:9e66c0ef402828d84fcfa6ab863ad40397295efb1491e12f2598d30781e9bc5d`.

The retained read-only follower was restarted for verification and stopped
afterwards. It uses source `c3061cb1883eee253890d8310e5393deeb7b4335`, image
`ghcr.io/eduadiez/rollupx-node@sha256:0eeb551755a060d7cd845c0cd4dbe871eb33dec1c55eba924d0e58e21da9a85c`.
It originally reconstructed history from genesis and now resumed from its own
correlation checkpoint. It has L1/Beacon inputs, no signing keys, no sequencer
overlay and no peers. This is also old-follower/new-Composer compatibility
evidence, not a fresh-genesis bootstrap test of the current follower build.

Both known shared-workload locks were held during sending. Test actors were
separate from the poster. The existing test funder funded new actors and
deployed workload-only targets/proxies before ordinary load began. No protocol
deployment, upgrade, operator configuration change or Composer/signer restart
occurred. Their container identities, start times and restart counters were
unchanged. The final queue was empty; locks were released.

## Results

Ordinary sending ran **11:39:02.034–11:54:02.034 UTC**, 900.000081 seconds:

- **4500/4500** intended ordinary self-transfers accepted at 5 tx/s, zero skipped
  sends and no submission error. All 4500 had successful canonical receipts.
- **25** covering L1 postings and **25** full blobs authenticated through
  independent L1 receipt/block observations and follower KZG reconstruction.
- Exact sender nonce advancement and balance difference equal to receipt gas:
  **945004630500000 wei**. No client resubmission.
- Independent follower matched every ordinary receipt and **701** containing
  or terminal blocks, including state roots. Checked terminal L2 block
  `0x3a840`, hash
  `0x909969598b4ff3f4470933d4b870f4fbf365cd780c0b996bb8acbf808bca14f8`.
- Interior window: **673/716** ordinary blocks contained multiple test
  transactions; 43 were empty. All 180 sync positions contained no ordinary
  test transactions, as permitted by this implementation. Positions 1/2/3/4
  included 1910/1038/802/750 workload transactions.

The runner completed **24 of 30 planned mixed waves** before its verifier
failure stopped further sending. All **192 submitted cross-chain transactions**
(96 inbound, 96 outbound) were independently checked afterwards:

- Canonical receipts/logs and 49 distinct containing blocks agreed with an
  independent L1 provider or L1/Beacon-only follower, as appropriate.
- Both setters/no-return setters had the exact expected destination values.
- Registered proxy recipients received exactly **0.003** native units inbound
  and **0.0015** outbound, against independently checked zero starting balances.
- Queue `queued`, `inFlight`, `blocked`, and `unknown` were all zero.

Retained event timing/recovery evidence covers the driver interval including
setup and final draining, not just the 900-second sending interval:

- **31** dispatches, **25** included attempts, **6** exclusions: four
  `slot_skipped` and two `excluded`. All dispatches preceded the configured
  exclusive cutoff by **1619–1732 ms**.
- **194** decisions: 163 ordinary continuation and 31 preparation. Decisions
  were **1.132–13.090 ms after** P+250 ms, never before it.
- At least **164 ordinary transactions** were observed in displaced blocks;
  **13 outbound transactions** had observed receipt movements. Every one was
  included in the successful canonical/replay checks, without resubmission.
  Sampling makes these lower bounds. No controlled fault injection was used.

## First failures and read-only corrections

1. The first setup attempt reused an idle-queue assertion while mixed traffic
   had already begun. It failed before any ordinary transaction was sent.
   Its four submitted inbound requests subsequently settled; the empty queue
   was confirmed before starting a separate attempt with fresh actors.
2. The second attempt completed all 4500 ordinary sends but its ordinary-only
   verifier compared the correlation endpoint with the final execution event
   **inside the posting transaction's receipt**. That is invalid for mixed
   batches: deferred inbound companion transactions can finish execution later
   in the same L1 block. This failure stopped the 30-wave runner after wave 24.
3. A separate read-only revalidation authenticated all rollup execution events
   in the canonical L1 block against independent provider data, required the
   final event to equal the exact correlation endpoint, and checked that
   event's successful containing receipt. It retained receipt, blob, canonical
   block, balance, nonce and independent follower assertions. It resent nothing.
   An initial bare-environment-name parsing error in that read-only wrapper was
   also retained; the corrected invocation used a separate output directory.

Concrete failure: posting `0xdc7a5cba2f91ce7e36a347356066d871cd1ac63556ee7153f100e69f18d19574`
in L1 block **23455743** ended at intermediate commitment
`0x4f5263279de7f67073544194ccb8a49fe4431f1a41ac4818e04889b0330e0cb9`.
The fourth subsequent inbound transaction in that block emitted
`0x2a593df4b2c7e08c6e9dc81a01c4b8d93ab82abc33b5fa1e10ec321d47bf8270`,
exactly the correlation's terminal L2 block hash. This agrees with the pinned
[core execution rules](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md)
and the deriver's existing deferred-entry handling; it is not evidence that
settlement or correlation logic needs changing.

## Evidence and executed checks

Execution evidence root: `/tmp/rollupx-pr79-validation-20261010.lQmFWOBR`.
A preserved local-only copy is at
`/home/edu/Development/rollupX/data/pr79-live-mixed-validation-20261010`.
These files are not available from another checkout; no remote artifact was
uploaded and no guaranteed host retention policy is established.
Original failures, signed ordinary intents, observations, receipts, full blobs,
node/signer/follower logs and wrapper sources remain there. No private keys are
included in this record. Successful revalidation summaries:

| File relative to `attempt-2/revalidation/` | SHA-256 |
| --- | --- |
| `ordinary-evidence-2/summary.json` | `2b5e52755179a023c57f50d1cd302675dc44fb6569fbec054cd9c2b040fb92e0` |
| `ordinary-evidence-2/independent-replay.json` | `8e744bfd6c34f83f1f41312bda1923975e579ed4351344047cd8b77adc6ca52b` |
| `independent-cross-chain-summary.json` | `152414bb67be67009f6a0fb892369944fa0b22c1b7a563ad97770c91bcbe069e` |
| `timing-recovery-summary.json` | `8f72aa7e839f9fd458d919658bffb422f3dccd603e41c4354721adc0606e2f42` |

From the PR worktree, the driver invoked the unchanged
`testing/kurtosis/scripts/cross-chain-wave.sh` with `EEZ_USE_LOCAL_DEPLOYMENTS=1`,
mode `mixed`, 30 waves and 30-second gaps. A temporary copy of the earlier
ordinary stress helper supplied the simultaneous 900-second stream. The
original driver exited **1**. Separate `ordinary-readonly.py`, `verify-mixed.py`
and `check-timing.py` under `attempt-2/revalidation/` exited **0**. Shell syntax
and Python compilation checks were run for the temporary wrappers.

## Limits / remaining validation

- This establishes settlement and recovery for the retained workload, not a
  clean first-pass run of the original verifier or completion of all 30 waves.
  The 15-minute continuous mixed-load window itself was completed.
- The receipt-stability assumption behind earlier protocol CI failures remains:
  successful optimistic receipts can move before canonical settlement. This run
  reproduces movement, not transaction loss; it does not make that CI assumption
  reliable or justify weakening its assertions.
- No intentional late-proof/finalization fault, restart, reorg, or replacement
  exhaustion was induced on the shared network. Natural exclusions are not a
  substitute for controlled boundary tests. Prior native/CI coverage is separate.
- No matched performance baseline, throughput capacity claim, durable queue
  recovery claim, protocol modification, implementation fix, PR merge or live
  upgrade is established by this record.

## Follow-up: complete workload passed, with funding intervention

The user authorized the follow-up plan. The new fork-owned
[procedure](../../runbooks/chiado-mixed-validation.md) reuses the existing wave
runner and promotes the temporary ordinary/independent checks into reviewed
tools. Settlement Rust code, contracts and the protocol pin remain unchanged.
The runtime source and node/signer identities above are unchanged. The retained
follower was restarted at **12:08:40 UTC**, with its own checkpoint/datadir.

The new driver evidence is retained locally at
`/home/edu/Development/rollupX/data/pr79-clean-mixed-20261010`.
It targets all 4,500 ordinary transactions and 30 waves, with no reduction of
expected counts. The ordinary fee ceiling is **50,000,000 wei**, tip unchanged;
there is no matched performance comparison with the earlier ceiling.

An operator setup error was caught before a transaction failed: the chosen
**0.004** per-actor funding does not cover 30 increasing inbound deposits,
which total **0.00465**, before gas. A single authorized **0.002** L1 top-up
to this run's inbound actor was canonically included as
`0x2a130f6cc3af029741bc8e2530247655c9293d39451fa6ae338e6e2ea2e45a70`.
No workload transaction was resent, and the ordinary L2 sender's balance/nonce
accounting is unaffected by that L1 transfer. This is an
operator intervention, not an unattended first-pass execution. The permanent
runner now has a value-budget preflight and the example is raised to 0.006;
that is a value floor plus margin, not a network-derived gas guarantee.

Initial local tooling validation passed **345 tests**, with five explicit opt-in
exclusions: real Docker payload, Kurtosis resource probe, native archive
relocation, and two real Docker runtime tests. The final suite, including review
additions, passed **355 tests in 208.334 seconds**, with those same five opt-in
exclusions. A retained-evidence check reproduced **24** old
posting-receipt-scope mismatches and accepted all **25** settlements using the
complete same-block execution evidence. This is offline evidence against the
retained run, not a new live result.

Claude reviewed only the scoped helper source and design with tools disabled,
under the user's earlier sharing approval. No operator files, keys, raw logs or
databases were sent. Findings addressed include optimization disabling Python
assertions, unnecessary poster-key derivation, canonical-snapshot rechecks and
end-of-verification runtime identity. The suggested `ETH_PRIVATE_KEY` replacement
was tested with a public fixture and is **not supported by this Foundry CLI**;
the documented key option remains, with inherited aliases removed and a trusted
operator-host requirement. An independent follower here means separate L2
execution/state from L1/Beacon data; it shares the local L1/Beacon infrastructure.
External L1 RPC provides the additional provider cross-check. Canonical-at-read
evidence is not a claim of finalized L1 history.

A second Claude pass found a signing-error logging path that could persist
the key-bearing command argv. The new signing wrapper suppresses that exception
and stderr, preserves only a fixed operation label/exit code, and has a regression
checking the rendered traceback. Its public-fixture address and signed bytes
are checked against the original Foundry invocation without broadcasting. The
review also identified the conservative one-posting-per-rollup-per-L1-block
harness limit, now explicit in the runbook. This does not alter protocol rules.

The [upstream runner suggestion](../../proposals/canonical-network-test-receipts.md)
documents the still-unresolved optimistic receipt assumption. A fork-side
characterization sources the real pinned helper with mocked HTTP and requires
the expected unstable-receipt failure. Its successful test result does not
mean the upstream bug was repaired. No upstream change or pin update is made.

### Complete-run results

The workload, original wave assertions, and independent verification all exited
**0**. Ordinary sending ran **12:10:45.506–12:25:45.506 UTC** for
**900.000103 seconds**:

- **4,500/4,500** ordinary transactions accepted and canonically successful,
  zero skipped ticks or submission errors. Exact nonce/gas accounting:
  **945004630500000 wei**. No client resubmission.
- **25** covering settlements and **25** full blobs; independent follower
  matched all **4,500 receipts** and **709** containing/terminal blocks.
  Terminal ordinary endpoint: `0x3afa2`, hash
  `0xa101a9b83a15e6b3d89893cae02430d71eecac7d686fbdbca74f51b1d98a75dc`.
- Interior occupancy: **654/717** ordinary blocks had multiple workload
  transactions, **26** had one and **37** were empty. The 179 sync positions
  carried no ordinary workload transactions. Position counts 1/2/3/4 were
  **1872/1072/791/765**; nonsettling-block occupancy is not a universal guarantee.
- **30/30 waves**, **240 cross-chain operations**: 120 in each direction.
  Independent receipts/logs and **61** distinct containing blocks matched;
  final setters/no-return setters were **330/230** on L2 and **630/530** on L1.
  Exact recipient increases were **0.00465 inbound** and **0.002325 outbound**
  native units from independently checked zero balances.
- Cross-chain safe endpoint `0x3b092`, hash
  `0x28b3f378069347fe923b7355869dc969ff9c3626743196d3e292f641c338ac0e`.
  Pinned ordinary and cross-chain blocks were rechecked after the full workload;
  final queue counters were all zero. No divergence events.
- **36** dispatches, **31** included attempts, **5** exclusions (two
  `slot_skipped`, three generic `excluded`). Dispatch
  headroom before the exclusive cutoff was **1471–1748 ms**. All **249**
  decisions were after P+250 ms, by **1.130–16.244 ms** (213 continuation,
  36 preparation decisions). These timing counts cover setup through drain,
  not only the 900-second ordinary stream.
- **19** ordinary transactions were observed in displaced blocks; **16**
  outbound hashes had observed receipt movements. All ultimately passed
  canonical/replay checks without resubmission. The observer saw all 240 hashes
  and reported zero observation errors; movement counts remain sampled lower
  bounds, not controlled-fault coverage.

Composer and signer image IDs, container IDs, start times and restart counts
were unchanged at the end. The owned follower was stopped after verification,
retaining its datadir and logs. No shared services were restarted or state
deleted. The live ordinary process had loaded its helper before the later
review hardening; the final independent checker included snapshot/identity
rechecks. Subsequent budget/key-handling/optimization safeguards were checked
offline, including unchanged signed bytes, not by another funded 15-minute run.
The underfunding regression failed against the original driver and passed after
the prepared driver was applied once the live invocation finished.

Local-only evidence digests, relative to the follow-up evidence root:

| File | SHA-256 |
| --- | --- |
| `live-mixed-ordinary/summary.json` | `9721ccf5ada88ce8cd1fef1eb40f713cce46695f2037dc4af68f890b8572c739` |
| `live-mixed-ordinary/independent-replay.json` | `1d88c53e1fcfcb1812d0387d7824710cdc794d15c17dd8c194df8540f17ddab2` |
| `independent-cross-chain-summary.json` | `44f8d69ddc8a9095a4a17832fd730e0e51a9e62d1ffcb2984a9b42107abbd064` |
| `timing-recovery-summary.json` | `74d04cc2eaf6b39d44885efc408c3fead5f27b214c2a6c534b656815fc82f8e5` |

This completes the intended workload and semantic/replay validation, with the
explicit funding intervention above. It does not remove the upstream CI
receipt-stability gate, demonstrate finalized L1 history, or authorize merging.

Final local checks: full `scripts/ci/tests` discovery, Python compilation,
`bash -n scripts/run-chiado-mixed-validation.sh`, documentation link resolution,
and `git diff --check` passed. The protocol checkout stayed clean at the same
gitlink. Rust/Solidity were not changed or rebuilt for this tooling-only update;
the new PR CI run owns the full runtime/native/Kurtosis matrix. The earlier
successful CI run `38040620049` remains evidence for `aaf2571`, not the new
validation commit. Raw artifacts and Claude review output remain local-only.
