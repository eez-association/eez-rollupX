# Chiado capacity recovery — 2026-09-14

Status: recovered and public access restored. All captured ordinary transactions
have settled, fresh cross-chain calls pass in both directions, and the monitor
and explorer checks pass. Submitters can resend requests that remain pending.

The user explicitly authorized resubmission of pending cross-chain requests.
This permits replacing the unconfirmed suffix after L2 block **72425**; it does
not mean the volatile bridge queue was exported. The last captured queue selection
reported **6312 sources at 20:47:12 UTC**. Submitters must resend requests that
were still pending. Existing deployed contracts and L1 history are retained.

## Confirmed history and backups

An isolated follower independently reconstructed all 72425 confirmed L2 blocks
from L1. Its endpoint exactly matched the original node:

- Block hash: `0x1373edd60923022182e86cefcae9e4dabece6f1aec13e63389410d0528a315da`.
- State root: `0xd60c0fbf7b767395b3d123c9b43f8032a963dfbf6442ae5170c741bd935aea51`.
- Last settlement: L1 block **23056675**, transaction
  `0xfdcf7e348e1b41bcf2c807503d475ddb743e89ae2f6c6c758c674cc3e7146ae9`.

The cutover used this independently derived database rather than an in-place
unwind. A complete stopped backup and the original database remain under the
runtime's private directory. The live composer alone was stopped at 20:48 UTC;
contracts, signer, native beacon service, and L1 chain identity were preserved.
Only the node image changed to `eez-node:chiado-da-guard-20260914`.

The deployed composer SHA-256 is
`131ed913c14ecc89be578713bff3f2ea287b9f421b90b2ddeb42f7bb12d2cbba`.
It matches the executable used by the new capacity tests. The four focused
native tests, 201 affected unit tests, and strict workspace Clippy passed before
the cutover. Broader native regressions continue during recovery.

## Transaction preservation and replay

Public HTTP, WebSocket, composer, and direct submission paths were quiesced.
The final snapshot through L2 **81025** contains **10455 ordinary transactions**;
all later provisional blocks were checked to be empty before stopping the node.
No entries were recovered from the standard Reth shutdown transaction-pool
backup. This does not establish preservation of requests held only in memory.

The export checks block hashes, parent links, transaction and receipt roots,
unique transaction hashes, and recovered signatures. Replay submits those exact
signatures, preserving sender nonce chains. Contract-containing original block
groups remain separate. Adjacent EOA-only groups may combine after checking code
and aggregate maximum debits. Later dependent groups wait for execution; at
most five groups await settlement together. Every receipt is compared for status,
gas used, contract address, and ordered log addresses/topics/data.

Replay completed at **21:31:02 UTC**: all **10455** original signed transactions
were included and settled, with zero receipt differences or nonce conflicts.
The final replay endpoint was L2 **84070**, hash
`0x85ed2f5e23a5105f1e10030f7694b1f61c8d1a98ad6ac849451055d6532f994b`,
state root `0x66b2ce2cca23dd58dfad1dfabd0af3b19193e065fac745b890c163b5da2c21ba`.
The independently derived follower matched that endpoint and exited cleanly.

During a replay-runner restart, two runners briefly overlapped. Both were stopped;
127 duplicate local receipt records were identical. All 2192 unique receipts then
present were rechecked against the canonical node, with zero mismatches, before
resuming under an exclusive process lock. Only original signed transactions were
submitted. Final DA verification separately checks transaction uniqueness.

Replay later paused on original transaction
`0x6b832b935aa16f4a0bcd8a0918451f8dd4a396488b7571dc1e2d8fe5eeb15390`.
Its current nonce, balance, 1000-wei gas price, empty-code recipient, and
21,000-gas read-only simulation were valid, but empty blocks continued while
the RPC reported it pending. Included activity had already settled through
82360. A controlled composer restart preserved that confirmed hash/root;
resubmitting the identical envelope then executed successfully in block 82976
with matching receipt fields and subsequently settled. No fee or nonce was
changed. The underlying pool-selection cause is not established; this is a
separate unresolved observation, not evidence of another DA overflow.

Replayed transactions acquire new block positions, timestamps and block hashes.
Receipt equality does not assert that arbitrary time-dependent contract storage
is identical to the replaced provisional history. The original database is
retained for inspection. The follower check establishes correctness of the new
canonical execution independently of the composer.

## Verification and evidence

An independent follower starts from the previously verified L1-derived checkpoint
and compares every observed settled endpoint hash and state root with the composer.
Native blob evidence is checked using the existing `verify_blob_evidence` helper;
the recovery check additionally compares every covered L2 hash and requires
contiguous coverage and unique transaction hashes.

The final pinned check through L1 **23058793** authenticated **241 postBatch
transactions**, containing **274 blobs**: 208 one-blob posts and 33 two-blob
posts. It verified every recovered L2 block from **72426 through 84190**
(11765 blocks), all 10455 original signatures, and 10461 unique L2 transaction
hashes including the subsequent cross-chain smoke's system/source transactions.
Every receipt succeeded; every blob passed native KZG and payload checks; all
covered L2 hashes matched the node; coverage was contiguous with no duplicates.

The fresh smoke sent two inbound and two outbound `setValue` calls through
the existing cross-chain fronts. Both nonce chains advanced by exactly two,
both targets ended at **92002**, inbound execution commitments were consumed,
and all four receipts were canonical and successful. These are generic
cross-chain calls, not a new bridge deposit/mint test. A second run of the same
independent follower database matched the settled smoke endpoint **84190**.

The first smoke attempt failed its funding preflight before submitting any
calls. A single **0.1 test xDAI** transfer from the poster funded the existing
test account, with fees estimated from Chiado and capped at 50 gwei. The failed
zero-submission attempt is retained separately; the successful smoke is not a
duplicate retry of already submitted calls.

Blockscout still held the replaced provisional suffix while its RPC access was
paused. Its complete database was backed up before requesting a refetch of
exactly **72426–80934** through its existing
[`Block.set_refetch_needed/1`](https://github.com/blockscout/blockscout/blob/v9.0.2/apps/explorer/lib/explorer/chain/block.ex)
function. This queues its normal import/reorg processing; it does not delete
the index or modify the rollup. The final explorer check compared the complete
recovered block range and all original indexed transaction receipts: **11766
blocks**, including the retained 72425 anchor, and **10455 receipts**, with zero
missing blocks, wrong hashes, pending refetches, or receipt mismatches at
**21:40:37 UTC**.

## Public restoration and live state

Public access was restored at **21:39:14 UTC**, after all recovery and selected
native regression gates passed. The exact original Nginx template was restored,
the temporary direct-port firewall rule was removed, and UI, monitor and
Blockscout services were restarted. This step did not restart the composer.

- All four public RPC/composer routes returned the expected chain IDs: L1
  **10200**, L2 **6290**.
- Both public WSS subscriptions advanced, returned hashes matching canonical
  RPC blocks, and unsubscribed successfully.
- The browser monitor received 14 snapshots over WSS with both head sources
  using WebSockets. It observed changing L1/L2 heads, retained search state,
  used one HTTP fallback request during a forced disconnect, stopped polling
  after reconnecting, and had no browser runtime errors. Median snapshot gap
  during the measured window was 1002 ms.
- The existing public explorer browser test opened the monitor's safe-block
  link and checked it against canonical L2 RPC, followed the Blobscan link, and
  decoded the existing historical two-blob post successfully.
- Three recovery posts, including a two-blob post and the final smoke post,
  matched Blobscan's stored block/transaction metadata. All five sampled blob
  byte arrays and commitments matched the authenticated beacon evidence.

The **21:43:27 UTC** health capture showed new ordinary and cross-chain traffic
settling after reopening: L2 safe **84810**, latest **84815**, healthy monitor,
and no composer error or `unpostable_range` events in the sampled two minutes.
The last queue observation was 30 sources before selecting five at 21:43:22;
this is a live queue sample, not a claim that the newly arriving queue is empty.

The running container retains policy `always`, general consolidation at **1800
L2 blocks**, automatic cross-chain admission, no manual blob cap, the **500-gwei**
execution fee ceiling, and a **20%** network-estimated priority-fee margin.
The composer binary still matches the tested SHA-256 above. No contracts were
redeployed.

One initial WSS verification attempted an immediate HTTP lookup and received a
null block. The repeat check allowed a bounded wait for readability and required
the exact notified canonical hash; its successful samples resolved in 4–24 ms.
Blockscout also logged transient near-head trace/state lookup errors before
converging. This audit does not establish the cause of that timing observation.
The browser launch additionally needed the existing Chromium cache path; it
passed after configuring that path, without downloading or changing the UI.

## Evidence

Evidence lives under
`artifacts/sequencing-da-budget-20260914/current-network-recovery/`:

- `confirmed-state-candidate.json`: independent reconstruction and endpoint.
- `public-maintenance.json`, `quiesced-ingress.json`: submission pause evidence.
- `authenticated-export-result.json`, `authenticated-transactions.jsonl`: verified
  original signatures and receipts; signed-export SHA-256
  `0b243b3c3326681005c1969f6b59d5f9b64e29195dc54b680217f3a2911e9517`.
- `cutover.json`: original/backup locations, image identity, and verified startup.
- `replay-result.json`, `replay-receipts.jsonl`, `replay-runner-lock-review.json`:
  replay progress, receipt comparisons, and runner restart reconciliation.
- `pool-stall.json`, `pool-stall-node.log`, `pool-restart.json`,
  `pool-restart-outcome.json`: the separately observed pending-pool delay and
  confirmed identical-signature recovery. The first restart command was rejected
  before execution by automatic approval review because its failure path could
  leave the node stopped; the executed revision guarantees a restart attempt
  through cleanup and copies backup bytes without parsing them while stopped.
- `post-recovery-follower.json`, `post-smoke-follower.json`: independent execution
  comparisons before and after the fresh cross-chain calls.
- `cross-chain-smoke.json`, `smoke-funding.json`: bounded live calls and funding.
- `authenticated-blobs/`, `post-verification-final.json`: complete native DA and
  receipt evidence.
- `blockscout-before-reindex.json`, `explorer-recovery.json`: index backup,
  bounded refetch, and convergence checks.
- `public-restored.json`, `public-rpc-wss.json`, `public-monitor-recovery.json`,
  `public-explorers/monitor.json`: public restoration and actual browser checks.
- `blobscan-recovery.json`: sampled recovery archive byte/commitment checks.
- `final-health.json`: deployed identity, whitelisted active configuration,
  observed queue selection and live chain/monitor state.

The prevention audit records the **201 unit tests**, **38 selected native
cases**, formatting, build, feature and strict Clippy checks. Recovery artifacts
and private database backups are retained locally and are not added to Git.

## Remaining limits

Pending cross-chain requests were intentionally not preserved across the reset;
the user authorized resubmission. The captured ordinary history was preserved
through replay, but that is not a guarantee about uncaptured in-memory requests
or identical timestamp-dependent effects in the replaced provisional history.

The separate ordinary-pool selection stall described above remains a specific
follow-up investigation. Near-head RPC readiness and consumer retry behavior
also deserve review. The network is functioning and the capacity prevention
checks pass; this audit does not claim either separate issue has been fixed.
