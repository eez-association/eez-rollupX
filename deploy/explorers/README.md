# Explorer build and verification notes

Start with the [operator runbook](../../docs/runbooks/explorers.md). The single
service definition is [`docker-compose.explorers.yml`](../../docker-compose.explorers.yml);
use `docker compose --env-file .env.explorers` for all lifecycle commands.
Files here are deployment assets and
operational verification tools. The notes below describe the existing Chiado
image patches and deployment-specific verification fixtures.

## Scope and compatibility

- Blockscout uses the standard EVM/Erigon tracing interface exposed by the L2
  Reth node directly through the configured `L2_RPC_URL`. The L2 chain ID and
  genesis allocation indexing use the actual deployment genesis.
  Its ordinary block confirmation count is an L2 count; use the existing EEZ
  monitor to establish canonical L1 settlement, safe status, and batch ranges.
- The Blockscout 9.0.2 image includes both Mint and Hackney HTTP adapters. The
  derived image selects the pooled Hackney adapter to avoid ephemeral-port
  exhaustion during catch-up. The frontend is pinned to 2.3.5. A local proxy
  routes API requests and live WebSocket updates to the same explorer backend.
- Blobscan 1.40.3 is built from upstream revision
  `424d5e512d57ff4ac42509ef93f3845ead392155` with the reviewable
  [Chiado patch](blobscan-chiado.patch). Its indexer already recognizes Chiado,
  but the shared chain table and web schema in that release do not.
- The dedicated Chiado build adds chain 10200, 16 slots per epoch, the actual
  Deneb/Pectra dates, maximum 2 blobs, target 1 blob, a 1 gwei minimum blob fee,
  and the 1,112,826 fee-update fraction. These parameters remain valid through
  the deployed Fulu fork. They match the node's `reth_gnosis` preset and the
  live beacon configuration. Ethereum's shared fork parameters are unchanged.
- This is a **Chiado-specific build**: native fee amounts use the xDAI label,
  preserve integer precision, and have no mainnet ETH/USD price feed. Do not
  reuse the custom image for a different chain. A future Chiado fork changing
  blob parameters requires updating this profile before using its capacity or
  derived-price displays.
- Blobscan retains all Chiado blob transactions from the configured
  `BLOBSCAN_START_BLOCK`, including other posters. Preparation derives the
  associated slot and block hash from matching L1 and beacon endpoints. The
  API uses that slot as the statistics floor, and the web UI uses the block
  as its navigation floor and coverage label through runtime configuration.
  No per-deployment image build or source edit is needed.
- Blob data persists in PostgreSQL, with Redis queues on a separate durable
  volume. The indexer resumes its durable upper cursor and handles live reorgs.
  Unbounded backfill to Chiado's original Deneb fork is disabled. The local
  Lighthouse serves native blob bytes and commitments on Fulu. Its legacy
  sidecar endpoint supplies empty blob proofs after Fulu, as documented in
  [Lighthouse's release notes](https://github.com/sigp/lighthouse/releases/tag/v8.0.0).
  The UI marks these proofs unavailable; the raw upstream API retains the
  placeholder. Do not use that field as a cryptographic proof. Real nonempty
  proofs from retained pre-Fulu blocks remain visible.
- The explicit `STATS_SYNCER_LOWEST_SLOT` setting scopes totals to this
  deployment's retained history without overriding the real Deneb fork date.
  `verify.py --complete-backfill` enables aggregation only after scanning every
  canonical L1 block through a finalized snapshot and comparing all of its
  blob transactions, stored contents, commitments, and receipt fees. Earlier
  Chiado history is neither fabricated nor marked indexed. Totals update each
  minute. Completed-day chart aggregation is checked every minute and does no
  work when up to date; the current UTC day appears after it finishes.
- Blobscan shows raw blob data and generic decoders. It does not add an EEZ
  decoder or change the blob encoding. Continue using the EEZ monitor and
  authenticated blob verification tooling for protocol-specific decoding.
  PostgreSQL storage links use the frontend's existing blob-data route so
  downloads work through an SSH tunnel without resolving Docker hostnames.
  The patch also corrects upstream's PostgreSQL download conversion: hex JSON
  becomes the original 131,072 binary bytes, including in the raw-data viewer.
  Missing exchange prices remain unavailable instead of displaying $0.00.

## Operations

Use `docker compose --env-file .env.explorers` followed by `ps`, `logs`, `stop`,
or `up -d` for routine operations. Stopping this project does not stop the rollup. Ordinary
`down` preserves explorer database volumes; **do not use `down -v`** unless
intentionally discarding the indexes and retained blobs.

If you opened chart pages before initial backfill and aggregation finished,
upstream may cache the empty result until the next UTC day. After aggregation,
refresh only those disposable chart entries (this preserves queues and blobs):

```bash
docker exec eez-explorers-redis-1 sh -ec '
  redis-cli --scan --pattern "trpc:query:getTimeseries:*" |
    while IFS= read -r key; do redis-cli UNLINK "$key"; done
'
```

`build-blobscan.sh` runs the upstream migrations in an isolated disposable
database for Prisma's typed SQL generation, builds the API before the web image,
and runs [profile checks](check-chiado-profile.ts) and
[blob proxy checks](check-blob-proxy.ts). A temporary Buildx builder and the
database share an isolated Docker network using service DNS. Build steps share
the builder container's network namespace (`--oci-worker-net=host`); the builder
itself uses the isolated bridge, not the machine's host network. The script
removes the builder, build cache, database, and network on exit. The runtime
databases and credentials are never used for the build.

The profile checks cover fork boundaries, Chiado capacity and fees, epoch
conversion, chain-name routing, exact xDAI formatting, and preservation of
Ethereum's fork parameters. Deployment validation additionally compares live
explorer records with canonical RPC and beacon data and checks the browser.

For the single-domain path layout, the optional
[`build-blockscout-frontend-paths.sh`](build-blockscout-frontend-paths.sh) pins
Blockscout frontend 2.3.5 to revision
`95feb0ba3245c67b1ced38e71a38569951add266` and applies the reviewable
[`blockscout-frontend-paths.patch`](blockscout-frontend-paths.patch). Run
`build-blobscan.sh --paths` to apply the
[`blobscan-paths.patch`](blobscan-paths.patch) after the Chiado patch. It builds
only a separately tagged web image; the existing Chiado API image remains in
use. The path overlay and cutover checks are in the
[operator runbook](../../docs/runbooks/explorers.md#single-domain-gateway-paths).

## Repeatable verification

The [canonical-data check](verify.py) scans every L1 block from deployment
through a finalized snapshot. It checks blob transaction ordering, receipt blob
fees, beacon commitments, versioned hashes, and the exact stored blob bytes.
It also samples L2 blocks and transactions up to the current safe head when
Blockscout is selected in the env file. It compares against the running native clients; it does not
independently recompute KZG commitments or replay L2 execution.

Run the canonical check against the selected deployment (Blobscan is required):

```bash
python3 deploy/explorers/verify.py --env-file .env.explorers \
  --output artifacts/explorer-verification.json --complete-backfill
```

Node endpoints, ports, and history settings come from the env file. Use
`--l2-rpc URL` to override its `L2_RPC_URL` for verification. Blobscan-only
verification skips L2 checks and can still enable complete-history statistics.

The browser script below retains recorded examples from the original Chiado
deployment. They are optional regression fixtures; adapt their chain ID,
transaction hashes, and block numbers for other deployments.
The browser check requires Playwright with Chromium. Use an existing Playwright
installation or install it in a disposable directory:

```bash
browser_work="$(mktemp -d /tmp/eez-explorer-browser.XXXXXXXX)"
npm install --prefix "$browser_work" playwright@1.63.0
PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  "$browser_work/node_modules/.bin/playwright" install chromium
NODE_PATH="$browser_work/node_modules" \
  PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  node deploy/explorers/verify-browser.cjs artifacts/explorer-browser
```

This checks real browser API calls and live L2 WebSocket events, a finalized
transaction using both Chiado blobs, accurate fee units, proof availability,
completed-day charts, and a binary download against its verified SHA-256.
The fixed historical examples belong to this deployment; use different known
canonical examples when adapting the scripts to a fresh deployment.
