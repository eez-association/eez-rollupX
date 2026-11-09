# Measure Chiado fee quotes and mixed settlement

This bounded procedure exercises `eez_quoteFee`, signed admission and canonical
settlement on the existing Chiado deployment. It was run on 2026-10-15; see the
[results and retained failures](../audits/2026-10-15-chiado-fee-load/README.md).
The driver pins that runtime/image identity and local ports. Review those guards
against verified deployment evidence before adapting it to another runtime.
It is not a generic benchmark or a network deployment procedure.

The later [full protocol/Falcon stress exercise](../audits/2026-10-16-chiado-batching-stress/README.md)
passed its Falcon workload but exposed shared-batch gas failures, queued-nonce
admission limits and a history-limit recovery blocker. The earlier bounded
success does not qualify that broader matrix; review its pending-work status
before starting another workload on the same deployment.

The ordinary staged protocol runner does not issue compact quotes. This driver
fills that gap using the fee SDK, pinned nested-counter contracts and Safe 1.4.1
fixtures. It records offered and achieved traffic separately. The
[mixed-validation runbook](chiado-mixed-validation.md) covers a different workload
with independent L2 replay; this driver does not provide that replay.

## Prerequisites and ownership

- Existing authorized Chiado deployment with chain IDs 10200 / 906969, local
  L1/L2 RPC on 18645/18688 and Composer L1/L2 on 18999/18998. The harness also
  uses the independent public Chiado L1 RPC for low-rate verification.
- Initialized protocol pin, Foundry, Docker CLI, Node, Python, Bash and `flock`.
  Build the pinned counter fixtures with `forge build --root eez-core-protocol`.
- An existing UI dependency installation containing viem. The measured version
  was 2.47.0 with Node 26.7.0. The UI is used only to resolve this dependency;
  its application is not changed or exercised.
- A private deployment checkout with `deployments.env` and protocol `chain.env`.
  `SOURCE_PK` must be the selected test funder, distinct from the poster. Never
  print these files, enable shell tracing, or commit generated private state.
- Fresh private evidence directory and adequate test funds. Preparation can
  bridge **1.1 test-xDAI**, requires more than 1.25 on L1 for that path, funds
  64 actors with 0.014 each and gives 16 nested-Safe actors another 0.008 each.
  A separate quote actor gets 0.014. Deployment/transfer fixture gas is additional.
  These are this workload's bounds, not current fee recommendations.
- Healthy L1/Beacon/L2 and an empty Composer queue. Coordinate all users of the
  shared funder. The wrapper locks both the deployment and harness checkout's
  existing protocol wallet locks; it cannot discover another host using the key.

No services are created/restarted. Do not change fee policy to make a run pass.
Private actor/owner keys, signed source records, setup hashes and first failures
must be retained. Setup checkpoints prevent duplicate broadcasts after a receipt
timeout; they do not provide general crash recovery or authorize replacement.

## Primary run

From the reviewed harness checkout, select paths using the private inventory:

```bash
export EEZ_LOAD_DEPLOYMENT_ROOT=/absolute/path/to/existing-deployment
export EEZ_LOAD_UI_ROOT=/absolute/path/to/ui-with-pinned-viem
export EEZ_LOAD_OUTPUT=/absolute/path/to/new-private-evidence-directory
umask 077
mkdir -m 700 "$EEZ_LOAD_OUTPUT"
forge build --root eez-core-protocol
bash scripts/run-chiado-fee-load.sh > "$EEZ_LOAD_OUTPUT/run.log" 2>&1
python3 scripts/summarize-chiado-fee-load.py "$EEZ_LOAD_OUTPUT"
```

`mkdir` must fail if the selected output directory already exists. A failed
command is a stop condition, not an instruction to continue the command block.
The wrapper prepares fixtures, runs four canaries, offers the fixed-rate stages,
drains accepted work and verifies semantics/fees. It retains `run.exit`, node and
signer logs. Do not rerun the whole wrapper after measured traffic began: the
driver rejects a second `run-start.json`, and verification is a separate mode.

The stages offer 10/25/50/100 background quotes/s and 0.5/1/2/4 source tx/s for
60 seconds each, then 25 quotes/s plus 8 source tx/s for 30 seconds. Each quote
internally estimates gas for both transaction formats. Source-specific quotes
are additional. The four categories are nested counters, Safe creation, nested
creation of five Safes, and Safe value transfers. Setup is outside measurement.

The primary sender pool is 64. Lack of an idle actor is logged as an omitted
arrival, never an endpoint error or a successful send. Pending work, scheduling
lag, fee reserves and stale-L1 guards bound the exercise. A stage drains before
the next begins. There are no automatic quote/submission retries.

## Verification and interpretation

Require all of the following before reporting successful workload execution:

- `run.exit=0`, `run-end.json` has no `stopped` or `monitorError`, and its accepted
  count equals settled count. Check every offered stage was actually selected.
- `verification.json` reports `pass=true` and the same nonzero source count.
  Inspect omissions and quote/admission error distributions separately; a
  reduced accepted count is not full offered-rate capacity.
- Independent L1 receipts, full settlement events and terminal commitments agree.
  Safe/counter semantics and exact sender/recipient balances pass. Test tips
  cover each whole posting's actual execution plus blob cost.
- Runtime identities/start times/restart counts match; queue is empty; L1 and
  Beacon remain healthy. Record failures even if a later read succeeds.

The wrapper does **not** run independent blob verification. The recorded audit
also grouped every source by posting and ran this existing helper for every
group, with a verified binary from the matching runtime build:

```bash
export L1_RPC=http://127.0.0.1:18645
export L2_RPC=http://127.0.0.1:18688
export BEACON_URL=http://127.0.0.1:6152
export ROLLUP_ID=1
export BLOB_VERIFIER=/absolute/path/to/verified/verify_blob_evidence
bash testing/kurtosis/scripts/verify-semantic-settlement-blob.sh \
  "$POST_HASH" L2 "$VERIFIED_JSON" "$SIDECARS_JSON" "${SOURCE_HASHES[@]}"
```

Set `POST_HASH`, output paths and the Bash `SOURCE_HASHES` array from the complete
group in `accepted.json`. Require `verified`, `semantics_bound_to_settlement` and
`all_trigger_bytes_bound` in every output and matching total source counts. This
helper reads the existing network; it does not create a Kurtosis enclave.
The audit retains exact executable/output hashes. Independent L2 replay remains
a separate check; do not label these blob/L1 checks as follower replay.

Quote latency excludes failed responses; report their count alongside it. The
summarizer reports both settlements observed within the arrival interval and
the offered cohort's rate including drain. Neither admission rate nor optimistic
receipt rate establishes canonical settlement throughput. These short runs do
not establish a maximum sustainable capacity or a change versus an older build.

## Expanded follow-up and failures

The audit includes the exact [expanded adaptation](../audits/2026-10-15-chiado-fee-load/expanded-driver.patch)
and [preparation script](../audits/2026-10-15-chiado-fee-load/prepare-expanded.mjs)
used after primary verification. This is a stateful follow-up, not an independent
command to run on a fresh deployment. It reuses primary fixtures and unspent test
balances, funds 256 fresh actors with 0.384 total test-xDAI, and offers a single
25-quotes/s plus 8-tx/s 30-second burst. `EEZ_LOAD_BASE` selects the verified
primary evidence directory and `EEZ_LOAD_OUTPUT` a new private directory.
Hold both wallet locks throughout preparation, load and verification; adapt the
driver only in an isolated copy. Keep the primary code/evidence intact.

The recorded setup first reverted a 64-proxy MultiSend with out-of-gas; four
groups of 16 then succeeded. The measured burst ran once without omissions.
Independent verification first failed on a missing provider block, then passed
after the bounded availability wait. Original exit/failure files were preserved;
the verification-only retry has a separate exit marker. These corrections do not
turn the original end-to-end procedure into a clean first-pass result.

On any new failure, stop new traffic and reconcile every submitted hash before
another run. Retain ambiguous submissions and accepted ownership; do not resend
or clear state automatically. A verification-only invocation still needs the
same private environment and wallet locks. Preserve its separate log/exit result.
Do not sweep actor balances until checks requiring their original balances have
completed. No cleanup of shared containers, datadirs or wallets is part of this
procedure. Keep a dated audit with evidence availability, omissions, retries and
unrun checks.
