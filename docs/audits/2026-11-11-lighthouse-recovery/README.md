# Existing Chiado Lighthouse: discovery and pruning recovery

2026-11-11 UTC. Authorized investigation and repair of the existing Beacon client
on L1 10200 / L2 916969 / rollup 4. Composer/signer/follower remain release
`a8a26955ef52897ca0756e6700871f00e6821d00`, protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. No contracts, genesis, keys, fee rules,
L1/L2 databases or consensus configuration files were replaced.

## Root causes and repair

The v8.1.3 pruning error named exactly `anchor.oldest_block_parent`:
`0xf314d3750ad053583c54f0fe80f35ecebbda6e146bb0b86188ab99f1d9f344ae`.
The stored chain begins at slot 25060096. This matches upstream
[issue 10442](https://github.com/sigp/lighthouse/issues/10442): the backwards
pruning iterator walks beyond a checkpoint-synced Gnosis node's retained blocks
and aborts before committing its collected deletions. The issue predates the
runtime deployment. Stable v8.3.3 still contains it; an ordinary upgrade is insufficient.

[Fix 10454](https://github.com/sigp/lighthouse/pull/10454) stops the iterator at the
oldest block inclusively. On a separate database copy, stable v8.3.3 reproduced
`BlockNotFound`; the official fixed development binary completed pruning; stable
v8.3.3 then returned success with `Blobs are pruned`. The same sequence subsequently
passed on the stopped live database after another complete backup.

The service now runs stable **v8.3.3**, digest
`sigp/lighthouse@sha256:53f9063d98c3d3a2d290207873f019ea2298cfc02f8cd58d87c33b04aab856d1`.
The official development image
`sigp/lighthouse@sha256:219533433228d9ec2ad51cf5a6c07d718f609b4e633bf656f2fa06501aa76305`
(version `v8.4.0-rc.0-1a704e5`) was used **only as an offline pruning tool**, with
`--network none`, no JWT and no running database writer. Its source contains the
exact upstream boundary guard. It is not the running client.

Stable's explicit `lighthouse db migrate --to 32` migrated schema 28 to 32 before
pruning. All database operations retained the original custom network configuration.
The live fixed pass deleted 1,734,016 expired data-column keys and advanced the
retention marker to slot 25536256. Retention settings were not relaxed. Complete
pre-repair backups retain the original expired data as well.

Peer count was initially one, with no IP/discovery endpoint in the ENR. Restart
restored three direct outbound connections, but discovery requests still timed
out. The operator confirmed inbound forwarding; the public TCP listener was reachable.
Configured `LH_ENR_ADDRESS` and `LH_ENR_UDP_PORT=9205`. The ENR now advertises the
public TCP/discovery endpoint, and discovery logs show successful NODES responses.
This demonstrates working discovery responses, not a guaranteed inbound peer count.

An upgrade compatibility issue was detected and corrected during verification:
v8.3.3 parses deprecated `--libp2p-addresses`, but its service connection code uses
`boot_nodes_multiaddr`. Replacing it with `--boot-nodes` using the same endpoints
restored dialing. No peer-scoring checks were disabled and no peer identities were
marked trusted. Initial post-upgrade zero-peer/catch-up observations are retained.

## Executed validation and preservation

- Captured container identity, exact images, database metadata, peers, discovery
  logs and original private configuration before changes.
- A first host-level copy failed on private log permissions. The client was
  restarted immediately; that copy was used only as a test copy. Two subsequent
  full copies used container-root access with the source mounted read-only after
  clean client stops. They include databases, network identity and private logs.
- Tested schema migration, stable failure, fixed success and stable success on the
  isolated copy before applying the same procedure to live state.
- Restarted only Lighthouse. Composer, signer and follower retained their process
  start times and zero restart counters; public gateway configuration was unchanged.
- Verified a pre-repair settled blob remains retrievable and KZG-valid.
- Verified a fresh post-repair canonical type-3 settlement, actual blob KZG/native
  decoding and independent follower replay. See evidence for the checked range.
- Final checks: five connected peers, zero sync distance, not optimistic, execution
  layer online, no recurring pruning errors in the retained post-repair log, and
  successful public RPC checks. Peer counts fluctuate;
  successful discovery does not establish long-term peer diversity or availability.

The service retains semi-supernode mode, the same network identity and all data
inside the normal retention window. No resync, database reset, state reconstruction,
wallet action, synthetic cross-chain transaction or performance test was performed.
The original deployment's backups do not substitute for the Beacon backups here.

## Operations and limits

The operator environment selects the existing runtime Compose file plus a narrow
Lighthouse override under `data/lighthouse-repair-20261111/`. Use both files (or the
configured `COMPOSE_FILE`); explicitly selecting only the old file would choose the
old Lighthouse image. Private `OPERATIONS.md` records the exact invocation.

Do not start v8.1.3 against schema 32. A rollback requires a supported migration or
the complete pre-repair backup, with canonical catch-up and blob availability
checked. Restore/rollback was not rehearsed. The old checkpoint-boundary bug remains
in stable v8.3.3: this repairs this database's pruning history, not every future
fresh checkpoint database. Prefer a stable release containing 10454 when available.
A hostname ENR is resolved at process startup; re-check it after a public-IP change.

Raw diagnostics, source excerpts, image metadata, scripts, test copy and backups
are local-only in the ignored mode-0700 `data/lighthouse-repair-20261111/` directory.
They are unavailable in another checkout. Small sanitized results and hashes are
in [evidence.json](evidence.json). Official source/release/image references remain
subject to upstream retention. No private network addresses or credentials are
included in this record.

## Repository follow-up validation

The shared Compose update preserves both profiles' existing ports, mounts, network
configuration and optional ENR settings. Existing configuration checks passed:
`python3 -m unittest discover -s scripts/ci/tests -p test_chiado_config.py -v`
(19 tests, no skips) and `python3 scripts/check-settlement-policy-config.py`
(Chiado, Kurtosis and Ethereum passed). No new regression tests were retained;
only the existing expected image reference was updated. Rust and full E2E suites
were not rerun for this Compose/documentation change. The live evidence above
comes from the preceding operation, not a fresh deployment of this PR.
