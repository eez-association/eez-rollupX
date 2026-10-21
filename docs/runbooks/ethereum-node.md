# Run a Composer on Ethereum (mainnet, Hoodi or Sepolia)

**Scope.** Start and check the packaged Composer, proof signer and Lighthouse for
an L2 whose L1 is a public Ethereum network, using
[`docker-compose.ethereum-node.yml`](../../docker-compose.ethereum-node.yml) and
[`.env.ethereum.example`](../../.env.ethereum.example). One `EEZ_L1_NETWORK` value
selects the same network for all three services, so a Hoodi or Sepolia rehearsal
uses the mainnet definition unchanged.

**Not covered.** Deploying or registering contracts and rendering the L2 genesis
(`deployments.env` and `datadir/genesis.json` must already exist), explorers and
monitoring. Registration is irreversible: rehearse it on Hoodi first.

**Verification status.** Static checks only: Compose rendering
(`scripts/ci/tests/test_ethereum_config.py`,
`scripts/check-settlement-policy-config.py`), the embedded-L1 and signer network
selection unit tests, and builder acceptance probes (2026-10-02). Not yet run
against a synced Ethereum node; no settlement on Ethereum has been observed.

## Topology

- `eez-node` embeds its own **reth v2.7** as the L1 execution client
  (`EEZ_L1_CHAIN=<network>`, reth's built-in chain spec). It does not connect to
  an external execution client. Its datadir is `L1_SNAPSHOT_DIR`.
- `lighthouse` drives that reth through the engine API on `L1_AUTH_PORT` (shared
  `JWT_FILE`) and runs as a **semi-supernode**, so the deriver can read full
  PeerDAS blobs from `127.0.0.1:6152`.
- `eez-proof-signer` trusts the same network (`EEZ_L1_CHAIN_CONFIG=<network>`).
- The schedule is fixed by the genesis profile: 12-second L1 slots, 2-second L2
  blocks (six per slot).
- Host networking with the Chiado profile's ports: run one packaged network per
  host.

## Prerequisites

1. `deployments.env` and the registered `datadir/genesis.json` (mode `644`).
2. **L1 execution datadir.** Either:
   - a **reth** datadir already synced for the network. Stop the reth that uses
     it first: the Composer becomes its only writer. Confirm reth v2.7 can open
     it, and that its pruning keeps history from `EEZ_REGISTRY_DEPLOY_BLOCK`; or
   - an empty directory. A mainnet sync takes days, and the default (archive)
     storage needs several TB.

   A non-reth datadir (geth, Nethermind, Erigon…) cannot be reused.
3. **Lighthouse.** An empty `LIGHTHOUSE_DATA` checkpoint-syncs from
   `LH_CHECKPOINT_URL`. An existing datadir can be reused with its JWT. Switching a
   regular node to semi-supernode backfills data columns before blobs are complete.
4. Keys: a funded poster, the registered proof signer, and a **dedicated unfunded**
   `EEZ_L1_BUNDLE_SIGNING_KEY` (BuilderNet rejects unsigned bundles).
5. **Settlement policy and fee limits.** Review the example's starting values
   before deploying ([policy reference](../configurable-settlement-policy.md)):
   - **Price ceilings:** 10 gwei (base fee + tip) and 2 gwei (blob). A post above a ceiling waits and retries; it never pays more.
   - **Tip:** the median of recent blocks' median tips (`EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE=50`), kept within 0.05–2 gwei.
   - **Cadence:** ordinary L2 activity settles at most every 15 minutes; an idle chain posts every 4 hours.
   - **Proof window:** `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS=8192`, so an idle cycle needs one post.

   Confirm proof time for a full window and the timing budgets in the rehearsal. The
   poster balance is the only total spending limit.

## First start

From the repository root, in a shell that has not exported another profile's
`.env` (shell variables override `.env`):

```bash
test -f .env || cp .env.ethereum.example .env      # then edit every CHANGE_ME / <full-commit>
openssl rand -hex 32 > data/jwt.hex                 # only if not reusing a Lighthouse JWT
docker compose config --quiet                       # fails on any missing required value
docker compose pull
docker compose up -d
docker compose ps
```

For a rehearsal set `EEZ_L1_NETWORK=hoodi`, `EEZ_L1_CHAIN_ID=560048`,
`LH_CHECKPOINT_URL=https://checkpoint-sync.hoodi.ethpandaops.io` and
`EEZ_L1_BUILDER_RPC_URL=https://rpc-hoodi.titanbuilder.xyz`.

## Checks

```bash
docker logs eez-node-ethereum 2>&1 | grep -E 'l1_embedded.(launching|ready)'   # kind="ethereum", l1_chain_id=<id>
cast chain-id --rpc-url http://127.0.0.1:18645                                    # embedded L1
cast block latest -f hash --rpc-url http://127.0.0.1:18645                        # equals (or trails by 1-2) a public RPC
curl -s http://127.0.0.1:6152/eth/v1/node/syncing | jq .data                      # is_syncing=false
cast block-number --rpc-url http://127.0.0.1:18688                                # L2 height advancing
```

**Stop conditions:**
- **Unexpected L1 kind or chain ID:** if the embedded L1 reports a different chain ID or `kind`, stop. `EEZ_L1_CHAIN` must be exactly `mainnet`, `hoodi` or `sepolia`; any other value is a startup error.
- **Signer refuses to start:** a `prover validation identity differs` error means the node and signer images come from different commits.
- **Invalid builder signature:** `Invalid signature` replies from BuilderNet mean `EEZ_L1_BUNDLE_SIGNING_KEY` is missing.

Bundle acceptance is not inclusion. Measure the share of settlement slots that
land before relying on a builder set.

### Why a post was excluded

[`scripts/why-excluded.py`](../../scripts/why-excluded.py) reports successful and
failed builder submissions, including posts dropped with `reason=excluded`.
It combines Composer JSON logs, canonical block data, relay deliveries and
builder bundle stats, then prints a per-builder summary. It uses only Python's
standard library and makes read-only requests; it does not send bundles or change
the Composer. The built-in relay
and stats endpoints serve **Ethereum mainnet**. The script checks `eth_chainId`
and rejects other networks, including Hoodi and Sepolia.

```bash
source .env
sudo -v
sudo docker logs --since 2h eez-node-ethereum 2>&1 | \
  python3 scripts/why-excluded.py --logs - --builders "$EEZ_L1_BUILDER_RPC_URL"
# Explicit targets provide block/relay context, without logged bundle hashes.
python3 scripts/why-excluded.py 26131577 26131674 --builders "$EEZ_L1_BUILDER_RPC_URL"
```

Pass the builder list explicitly as above: sourcing a Compose `.env` does not
export its variables to Python. Use logs from one Composer and the builder list
it used for those targets. Include the complete dispatch, response, and fan-out
events; JSON stdout is enabled in `docker-compose.ethereum-node.yml`. Plain-text
logs are not parsed. No exclusion event is required: successful-only and
rejected-only logs also produce reports. If no submissions or drops are found,
the error shows how many lines and JSON events were read. Check the log format,
container and time window; an empty report never establishes builder health.
`--rpc` defaults to the embedded L1 (`http://127.0.0.1:18645`). A saved log file
can replace `-`; preserve it if the container's logs are about to rotate.

Each summary row counts one builder domain per distinct `(target block, post
transaction hash)`. Repeated log lines count once; different posts at the same
height remain separate. Multiple endpoints for the same builder are grouped.
Any acknowledgement makes that observation accepted; it is counted as rejected
only when a rejection was observed and none of its endpoints acknowledged.
Observed outcome types and every distinct acknowledged bundle hash remain
in the attempt report. Configured builders with no observed submissions appear
with zero counts, which does not establish reachability or health.

| Summary field | Meaning |
| --- | --- |
| `observed` | Distinct builder/post/target observations in the supplied logs |
| `accepted` | The node recorded an acknowledgement; this does not imply successful simulation |
| `rejected` | HTTP/JSON-RPC submission rejection with no acknowledgement from that builder; method-not-found is separate |
| `included` | An accepted observation's exact post is present in the current canonical target block |
| `excluded` | An accepted observation's exact post is absent from that block and its timestamp matches the logged target timestamp |
| `unresolved` | Accepted, but target inclusion/exclusion cannot be established: missing post hash/timestamp, changed slot, pending block, or RPC failure |
| `simulation_failed` | An acknowledged bundle's available stats explicitly report `Invalid` or `SimulationFail` (including lowercase/snake_case variants); acceptance is retained |
| `unsupported`, `transport_error`, `deadline_exceeded`, `abandoned_after_acknowledgement`, `unknown` | Typed submission outcomes, kept separate from rejection and canonical exclusion |
| `uncorrelated` | No exact post hash was available; the observation is grouped by target and builder only |
| `omitted_from_own_block` | An accepted post was excluded from its matching target slot, and published relay keys identify that builder as the block producer; currently supported for BuilderNet |

`included + excluded + unresolved = accepted`. Submission counters and canonical
outcomes describe different stages, so do not sum all columns as a total. An
included post is counted for every builder that acknowledged that attempt; these
are **not builder win counts**. `omitted_from_own_block` is a subset of `excluded`,
counted per post/target observation, not per block. The report does not establish which builder
caused inclusion, successful receipt execution, inclusion of every companion
transaction, or full settlement. Later inclusion within `--horizon` is displayed
separately and does not change the target-slot counters. These observations use
current canonical blocks and can change after a reorg; they are not finality
assertions.

Typed `eth_sendBundle fan-out finished` events are authoritative for builder
outcomes and retain HTTP failures/timeouts that have no JSON-RPC response line.
Older or truncated logs fall back to response lines only when the target has at
most one observed post. Responses that cannot be assigned across multiple posts
at one height are disclosed as `uncorrelated_responses`; unmatched outcomes and
hashes stay in an uncorrelated target record so their stats can still be queried.
Malformed fan-outs and conflicting timestamps are also reported. Without post
hashes, several attempts can collapse into one uncorrelated observation. Partial logs cannot
establish complete totals. No node upgrade, restart, or storage migration is
required to use the script with the existing structured events.

| Category | Meaning | Action |
| --- | --- | --- |
| `POSSIBLY_LOCAL` | Client-like `extraData`; all listed relays answered but none reported this block | Local construction is only a possibility: an unlisted relay may have delivered it |
| `OTHER_BUILDER` | A relay delivered this exact block, but the identified producer or fallback `extraData` hint is outside the configured builders | Inspect `block_producer` and builder coverage; `extraData` is only a hint |
| `CONFIGURED_BUILDER` | A relay delivered this exact block and the identified producer or fallback `extraData` hint matches a configured builder | Inspect `block_producer` and per-builder evidence; this category alone does not explain exclusion |
| `TARGET_MISMATCH` | The target height has another timestamp | Skipped slot or reorg |
| `UNKNOWN` | Evidence is insufficient or conflicting, including unavailable relay APIs or an absent builder list | Resolve missing evidence; do not interpret it as a builder failure or local construction |
| `NOT_YET_PRODUCED` / `RPC_UNAVAILABLE` | The block is not available yet / the RPC read failed | Retry the observation; builder stats may still be available |

Relay queries and returned records must match the canonical block **hash**, not
just its height. Public relay coverage is incomplete, and a missing delivery
never proves that MEV-Boost was not used.

`block_producer` records published-key attribution separately from `extraData`.
The script includes BuilderNet's [published BLS relay keys](https://buildernet.org/docs/public-identity)
checked on 2026-10-21, including retired keys for historical observations. An
exact key match in a canonical delivery identifies BuilderNet. This takes
precedence over the `extraData` hint when classifying configured builders.
One usable matching relay record suffices despite other outages or missing keys;
a usable key outside the published set alongside a matching key makes the
attribution `conflicting` and the category `UNKNOWN`. Unrecognized or malformed
keys alone leave attribution `unknown`. New BuilderNet keys require updating the
published list in the script; `extraData` alone cannot raise the omission flag.
This trusts public relay reports and the published key mapping; the script does
not independently verify a BLS signature. Returned `relay_deliveries` retain the
canonical block hash, relay, reported key and value for inspection.

`producer_omitted_acknowledged_post: true` requires BuilderNet attribution, a
logged BuilderNet acknowledgement bound to the exact post, and canonical target
exclusion with a matching timestamp. The text report prints a warning and the
BuilderNet summary increments `omitted_from_own_block`. A missing bundle hash
does not prevent this check when the post and acknowledgement are known. Missing
post/timing evidence, another builder's acknowledgement, or an unresolved target
cannot raise it. This identifies cases to investigate, not an internal failure
reason: late arrival, simulation, propagation and block selection remain possible
explanations. It does not establish faulty or malicious builder behavior.

Stats are collected for **every acknowledging builder and distinct logged bundle
hash**, even when another builder won or the target timestamp changed. The script
uses the following public APIs without credentials. Each request passes one
object in `params`, using the provider's hash field below.

| Builder / documentation | Endpoint | Method | Hash field |
| --- | --- | --- | --- |
| [Titan](https://docs.titanbuilder.xyz/bundle-tracing) | `https://stats.titanbuilder.xyz` | `titan_getBundleStats` | `bundleHash` |
| [Bombora](https://bombora.build/docs/bombora-getBundleStats/) | `https://stats.bombora.build` | `bombora_getBundleStats` | `bundleHash` |
| [Quasar](https://docs.quasar.win/bundle-tracing) | `https://stats.quasar.win` | `quasar_getBundleStats` | `bundle_hash` |
| [Eureka](https://docs.eurekalabs.xyz/bundle-tracing/api) | `https://rpc.eurekabuilder.xyz` | `eureka_getBundleStats` | `bundleHash` |

Titan traces become available about five minutes after submission; Bombora stats
after four blocks (about 48 seconds). Quasar requires at least two slots after
the target block or `maxTimestamp` and retains data for up to 24 hours after
submission. Eureka retains stats for about 30 days and documents a shared limit
of 50 requests per second. Save logs promptly and rerun after availability
delays. Missing, expired or rate-limited stats cannot establish a rejection.

BuilderNet's **bundle stats** remain `availability: "unsupported"`: its
[public API](https://buildernet.org/docs/api) documents refund queries, which do
not provide simulation/rejection tracing, and the legacy Flashbots stats methods
were [announced for deprecation](https://collective.flashbots.net/t/deprecation-of-bundle-stats-api-methods-in-flashbots-protect/4973).
On 2026-10-21, authenticated read-only probes of `buildernet_getBundleStats` and
`flashbots_getBundleStatsV2` at `rpc.buildernet.org` returned `-32601` (method not
found). No public tracing API was verified. The script reports that limit rather
than repeatedly calling those methods. Stats `availability: "unsupported"` is
separate from the summary's `unsupported` **submission** counter; an accepted
BuilderNet submission still counts as accepted.

For each distinct valid hash acknowledged by BuilderNet, the script separately
queries [`buildernet_getFeeRefundsByBundle`](https://buildernet.org/docs/api#buildernet_getFeeRefundsByBundle)
at `https://relay.flashbots.net`, with `params: ["0x..."]` (one hash string).
This read-only query needs no credentials and is not a simulation trace.
`builder_refunds` preserves the returned page and any cursor; `target_refunds`
selects records whose hash and block match the logged attempt. Other-block records
stay in the raw result. Malformed records or mismatched hashes make the query
`unavailable` with its result retained. Only one page is requested per hash/target;
a returned cursor means the refund history may be incomplete.

Refund `pending`/`sent` states refer to accounting, not submission or settlement.
The API documents a usual processing delay of about four hours. Empty, delayed,
unavailable or positive refund records never alter accepted/rejected,
included/excluded, simulation-failure or omission counters. In particular,
`refunds: []` does not establish that BuilderNet rejected the bundle.

Raw API results and error details are preserved. Quasar uses lowercase statuses;
the simulation-failure counter also recognizes PascalCase and snake_case. Eureka
reports the best status reached, so a detail such as `error: "HeadAdvanced"`
alongside `status: "Submitted"` is not treated as rejection or simulation failure.
When returned `bundleHash` or non-null `targetBlock` metadata disagrees with the
requested hash or logged target, the record is retained as `unavailable` and
cannot increment `simulation_failed`.

| Builder result | Interpretation |
| --- | --- |
| `Invalid` / `SimulationFail` | The builder reports a validation or simulation failure. Inspect its complete `error` field and the exact captured bundle. |
| `SimulationPass` | The builder reports a successful simulation. This does not establish why it omitted the bundle. |
| `IncludedInBlock` / `Submitted` | Included in a builder candidate / submitted to a relay. Neither establishes canonical inclusion. |
| `ExcludedFromBlock` / `Landed` | Builder-reported omission / inclusion; canonical target counters still come from the exact post check on the L1 RPC. |
| `Pending` / `NotFound` / `Received` | Evidence is incomplete. `Received` has builder-specific meaning; consult that builder's API documentation. |
| `unsupported` / `missing_hash` / `unavailable` | No supported stats API, no valid hash in the acknowledgement, or a failed stats query. These are diagnostic limitations, not bundle failures. |

The exact post transaction hash from dispatch, fan-out or drop logs is checked
in the target and up to `--horizon` subsequent blocks (default 12; zero checks
only the target).
An incomplete scan reports `pending` or `unavailable`, not a negative result.
A later transaction from the same poster, including a recomposed replacement,
does not establish inclusion of the original post. `--poster ADDRESS` optionally
lists that address's target-block transactions as **activity only**. Without a
logged post hash, inclusion remains unknown. Transaction inclusion also does not
prove successful execution or consumption of every cross-chain request; see
[settlement ownership](../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

`--json` prints one `type: "attempt"` object per observed post/target and a final
`type: "summary"` object. Each attempt has `builder_submissions`,
`target_outcome`, and `builder_bundle_stats`: a map keyed by builder domain whose
values are **lists** of stats records with `bundle_hash`, `availability`, and the
API `result` or query `error`. The final record contains `builders` (the counters
above), `categories` and `log_diagnostics`. Consumers of the earlier draft must
handle the typed records, the stats lists and the replacement of `LOCAL` with
`POSSIBLY_LOCAL`; a script rollback restores the earlier exclusion-only behavior
and JSON format. Exit status zero means reporting completed, not
that every builder is healthy; inspect the categories, counters and stats. Invalid input
or a network-check failure exits with status 2; a target RPC read failure exits
with status 1. This diagnostic does not change settlement-attempt accounting.

The BuilderNet extension adds `block_producer`, `relay_deliveries` when the target
is available, `producer_omitted_acknowledged_post`, and `builder_refunds` to attempt
records, plus `omitted_from_own_block` to builder summaries. The refund map uses
the same per-builder list shape as stats, with an additional `target_refunds`
list for available results. Consumers should tolerate these additive fields and
unknown attribution states. Rolling back to the preceding script drops these
fields and restores classification by `extraData` only; no node or storage
migration is involved.

## Stop and restart

`docker compose down` keeps every datadir. Pending requests and retry counters
are in memory: a restart reconciles canonical state, and clients may need to
resubmit unsettled work.
