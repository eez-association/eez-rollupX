# Explorers for EEZ on Chiado

This independent Docker Compose project indexes the existing Chiado deployment.
It does not redeploy the chain, mount node databases, use signing keys, or submit
transactions. The stopped Kurtosis devnet is separate.

| Service | Local URL | Coverage |
| --- | --- | --- |
| L2 Blockscout | http://127.0.0.1:18110/ | EEZ L2 chain 6290, from genesis |
| L2 Blockscout API | http://127.0.0.1:18111/api/v2/ | Blocks, transactions, receipts, addresses, traces |
| Chiado Blobscan | http://127.0.0.1:18100/ | Chiado chain 10200, from the EEZ deployment |
| Blobscan API | http://127.0.0.1:18101/ | Blob contents, commitments, L1 transactions, slots |

All four ports bind only to the **server's loopback interface**. PostgreSQL,
Redis, and the contract verifier have no published ports. The existing HTTPS
gateway now also publishes the two browser applications:

| Service | Public URL |
| --- | --- |
| L2 Blockscout | https://eez.asuscomm.com:4445/ |
| Chiado Blobscan | https://eez.asuscomm.com:4443/ |
| Monitor with explorer buttons and contextual links | https://eez.asuscomm.com/monitor/ |

The gateway joins `eez-chiado-explorers_default` and imports
[public.caddy](public.caddy). The raw Blobscan API and databases remain private.
An SSH tunnel remains an alternative to public access:

```bash
ssh -N -L 18110:127.0.0.1:18110 -L 18100:127.0.0.1:18100 YOUR_SSH_HOST
```

Blockscout's browser API and WebSocket URLs follow the opened origin, so a
different local forwarded port also works. Blobscan's browser uses its own
same-origin API; it does not need port 18101 forwarded.

## Initial deployment

Run from the rollup repository root. Docker Compose v2 and a Linux Docker host
are required. The profile expects the current local Chiado RPC, Lighthouse, and
UI RPC proxy ports; see `compose.yaml` before using another deployment.

```bash
python3 testing/explorers/prepare.py \
  --runtime .runtime/chiado-settlement-20260913/explorers \
  --genesis .runtime/chiado-settlement-20260913/genesis.json

docker build -f testing/explorers/Blockscout.Dockerfile \
  -t eez-blockscout:9.0.2-pooled testing/explorers
bash testing/explorers/build-blobscan.sh

docker compose \
  --env-file .runtime/chiado-settlement-20260913/explorers/explorers.env \
  -f testing/explorers/compose.yaml up -d --wait \
  blockscout-db verifier blockscout blockscout-web blockscout-gateway \
  blobscan-db redis blobscan-api blobscan-web

python3 testing/explorers/initialize-blobscan.py \
  --env-file .runtime/chiado-settlement-20260913/explorers/explorers.env

docker compose \
  --env-file .runtime/chiado-settlement-20260913/explorers/explorers.env \
  -f testing/explorers/compose.yaml up -d blobscan-indexer

# After catch-up completes (retry later if verification reports missing data):
python3 testing/explorers/verify.py \
  --output artifacts/explorer-verification.json --complete-backfill \
  --env-file .runtime/chiado-settlement-20260913/explorers/explorers.env
```

The credential generator creates only explorer database/API secrets in a mode
0600 file and preserves them on repeat runs. Do not print or commit that file.
`initialize-blobscan.py` verifies chain 10200 and the execution/consensus block
hash, derives the starting slot from the actual beacon genesis and slot time,
and initializes only an empty cursor. It preserves an existing cursor.

Do not start a fresh indexer before initialization: upstream otherwise starts
at the head, omitting the desired history. Restart the initialized deployment
with ordinary `docker compose ... up -d`; no initialization reset is needed.

## Public gateway and monitor

The active public deployment is managed separately from the explorer stack:

```bash
docker compose -p eez-chiado-public \
  -f .runtime/chiado-settlement-20260913/public/compose.json up -d --wait gateway monitor
```

That generated Compose file preserves the existing certificate volumes and
ports 80/443, adds TCP 4443/4445, and attaches the gateway to the external
`eez-chiado-explorers_default` network. It mounts `public.caddy` at
`/etc/caddy/explorers.caddy`; the existing Caddyfile imports that path. Use this
Chiado configuration rather than the separate Kurtosis gateway Compose file.
The monitor's existing link and archive settings are:

```dotenv
EEZ_L1_EXPLORER_URL=https://gnosis-chiado.blockscout.com
EEZ_L2_EXPLORER_URL=https://eez.asuscomm.com:4445
EEZ_BLOBSCAN_URL=https://eez.asuscomm.com:4443
EEZ_BLOBSCAN_API_URL=http://127.0.0.1:18101
```

These populate the monitor buttons and block/blob links and enable structural
blob decoding and historical lookup by blob hash. The local API URL is used
only by the monitor server, which runs on the same host.

The public Blobscan origin accepts reads. Its Chiado blob-data route accepts
only GET requests for a correctly shaped blob path on the configured archive
origin, rejects arbitrary targets, and does not follow redirects. This keeps
public blob downloads from becoming an unrestricted internal HTTP proxy.
[check-blob-proxy.ts](check-blob-proxy.ts) covers valid binary data and rejected
requests without any outbound fetch. The build script runs that check offline.

## Scope and compatibility

- Blockscout uses the standard EVM/Erigon tracing interface exposed by the L2
  Reth node. Genesis allocation indexing uses the actual deployment genesis.
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
- Blobscan retains all Chiado blob transactions from L1 block **23043686**
  (beacon slot **24788218**) onward, including other posters in that range. It
  does not claim coverage of older Chiado history. The UI shows this boundary;
  its first-block navigation floor is the local coverage boundary. Filter by
  EEZ poster `0x2eA2A5A1fe1BA4F4134c993E0c9A700706031E52` to inspect EEZ posts.
- Blob data persists in PostgreSQL, with Redis queues on a separate durable
  volume. The indexer resumes its durable upper cursor and handles live reorgs.
  Unbounded backfill to Chiado's original Deneb fork is disabled. The local
  Lighthouse serves native blob bytes and commitments on Fulu. Its legacy
  sidecar endpoint supplies empty blob proofs after Fulu, as documented in
  [Lighthouse's release notes](https://github.com/sigp/lighthouse/releases/tag/v8.0.0).
  The UI marks these proofs unavailable; the raw upstream API retains the
  placeholder. Do not use that field as a cryptographic proof. This archive
  starts after Fulu, and no pre-Fulu proof coverage is claimed.
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

Use the same `--env-file` and `-f` arguments above for `ps`, `logs`, `restart`,
`stop`, and `up`. Stopping this project does not stop the rollup. Ordinary
`down` preserves explorer database volumes; **do not use `down -v`** unless
intentionally discarding the indexes and retained blobs.

If you opened chart pages before initial backfill and aggregation finished,
upstream may cache the empty result until the next UTC day. After aggregation,
refresh only those disposable chart entries (this preserves queues and blobs):

```bash
docker exec eez-chiado-explorers-redis-1 sh -ec '
  redis-cli --scan --pattern "trpc:query:getTimeseries:*" |
    while IFS= read -r key; do redis-cli UNLINK "$key"; done
'
```

`build-blobscan.sh` runs the upstream migrations in an isolated disposable
database for Prisma's typed SQL generation, builds both images, and runs
[profile checks](check-chiado-profile.ts). It removes that build database on
exit. The runtime databases and credentials are never used for the build.

The profile checks cover fork boundaries, Chiado capacity and fees, epoch
conversion, chain-name routing, exact xDAI formatting, and preservation of
Ethereum's fork parameters. Deployment validation additionally compares live
explorer records with canonical RPC and beacon data and checks the browser.

## Repeatable verification

The [canonical-data check](verify.py) scans every L1 block from deployment
through a finalized snapshot. It checks blob transaction ordering, receipt blob
fees, beacon commitments, versioned hashes, and the exact stored blob bytes.
It also samples L2 blocks and transactions, including the recovery anchor and
large payloads. It compares against the running native clients; it does not
independently recompute KZG commitments or replay L2 execution.

The browser check requires Playwright with Chromium. Use an existing Playwright
installation or install it in a disposable directory:

```bash
browser_work="$(mktemp -d /tmp/eez-explorer-browser.XXXXXXXX)"
npm install --prefix "$browser_work" playwright@1.63.0
PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  "$browser_work/node_modules/.bin/playwright" install chromium
NODE_PATH="$browser_work/node_modules" \
  PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  node testing/explorers/verify-browser.cjs artifacts/explorer-browser
```

This checks real browser API calls and live L2 WebSocket events, a finalized
transaction using both Chiado blobs, accurate fee units, proof availability,
completed-day charts, and a binary download against its verified SHA-256.
The fixed historical examples belong to this deployment; use different known
canonical examples when adapting the scripts to a fresh deployment.

To exercise the public origins with the same Chromium installation:

```bash
EEZ_BLOCKSCOUT_BROWSER_URL=https://eez.asuscomm.com:4445 \
  EEZ_BLOBSCAN_BROWSER_URL=https://eez.asuscomm.com:4443 \
  NODE_PATH="$browser_work/node_modules" \
  PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  node testing/explorers/verify-browser.cjs artifacts/public-explorer-browser
NODE_PATH="$browser_work/node_modules" \
  PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  node testing/explorers/verify-public-monitor.cjs artifacts/public-monitor-browser
```

The public checks use ordinary DNS and certificate validation. They verify
secure WebSocket updates, follow the monitor's safe L2 block link, check blob
links, and decode a known finalized two-blob post through the monitor.
