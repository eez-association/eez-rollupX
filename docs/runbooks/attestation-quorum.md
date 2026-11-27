# Composer attestation quorum

To connect two Composers to the same independent providers, start with [Share a pool of remote provers](#share-a-pool-of-remote-provers).

Set `EEZ_PROVERS` in the Composer environment to a comma-separated list of `url=attester_address=proof_system_address` entries. Use one to sixteen distinct endpoint origins (HTTPS for remote provers, HTTP for literal loopback development), nonzero signing addresses and nonzero verifier addresses. Provision each signer's existing v2 configuration with its own key and locally derived network-profile vkey. Each signer receives the execution witnesses for its requests. Use trusted, access-controlled endpoints. Keep credentials out of shared configuration and logs.

Quorum startup requires the embedded L1 to have imported the registry, manager and verifier registrations. For a fresh or restored L1 datadir, sync to the deployment first. The bootstrap Composer can run in singleton configuration while the embedded L1 imports history. Switch to the explicit roster after verifying its registrations on that node.

For a 2-of-2 rollout, start both signers and register both supported ECDSA proof systems and their independently derived vkeys with the rollup manager. While the current threshold is still in effect, start the upgraded Composer with both entries and confirm the reported capability agreement. On an isolated rehearsal network set the threshold to two and confirm that an actual settled batch contains two sorted verifier addresses, two signatures and matching assignments. Recover each signer against its own digest and check canonical rollup advancement. Perform the production threshold change only through the deployment's authorized manager procedure.

Composer reads the effective threshold from the registered manager at startup and at each submission parent. Admission and pre-proof gas sizing reserve the effective threshold at their selected L1 parent. Before responses arrive, sizing covers the most expensive eligible combination of verifier addresses. A zero threshold still requires one proof. In 2-of-2 both signers must respond before the existing cutoff. In 2-of-3 the first two verified responses release the post. A later response appears in member diagnostics until the proof deadline or supersession. An unreachable threshold or a missed deadline retains selected work under the existing missing-signature path. Repeated user-bearing `eez.composer.prover.range_retained` events without a canonical advance warrant investigation. Check signer availability, capabilities, registry settings and member rejections for that window. If Composer admits a user request that the signers deterministically reject, it can repeatedly block unrelated users. Compare Composer and signer validation inputs and fix the configuration or implementation disagreement. Restarting Composer may discard all unsettled work, and resubmission can reproduce the stall. An unsigned rejection alone is insufficient evidence for removing any particular user. No per-prover fee is configured.

Composer gives each signer’s pending proofs priority over background witness preparation. A running signer validation can finish before the worker becomes available. Preparation resumes from durable witnesses after the member’s pending proofs finish.

Set `EEZ_PROVER_STREAM_MAX_IN_FLIGHT` in the Composer environment or Compose env file to limit unanswered Block and Finalize operations on **each** prover stream. For native Kurtosis, pass it as a quoted string in `eez.settlement_env`. Both singleton and explicit rosters default to **32**. The value must be a positive integer that fits the platform’s `usize` (1 through 18446744073709551615 on 64-bit targets). Empty, zero, malformed and overflowing values fail startup.

For a Compose A/B comparison, use the same pinned Composer image, project, Compose files and env file for both runs. After editing the env file for each run, recreate `eez-node`. `docker compose restart` keeps the container’s old environment. Use the stack’s usual Compose options for both commands below.

```bash
docker compose up -d --no-deps --force-recreate eez-node
docker compose logs --no-log-prefix eez-node | grep -F 'eez.node.prover.ready'
```

Wait for the recreated Composer’s ready event and check its `stream_max_in_flight` field before starting the workload. For an 8-versus-32 comparison, record **8** for the first run and **32** for the second. If the value differs, check for an exported `EEZ_PROVER_STREAM_MAX_IN_FLIGHT` overriding the env file, then recreate and verify again.

Earlier Composer binaries use a fixed limit of 8. Set the value to `8` before upgrading to preserve that behavior. Rolling back the binary also restores 8. The setting is local to Composer and works with existing v2 signers. Larger values allow more witness data in transit. If queued frames exceed a Rust signer’s pending byte or witness-item quota, it closes the stream with `RESOURCE_EXHAUSTED`. Reducing the pipeline to `8` restores the previous depth. A proof window that exceeds the signer’s request limits must itself be reduced. Each member’s negotiated limits and the existing proof deadline still apply. There is no wire, storage or historical replay change.

Omitting `EEZ_PROVERS` selects the existing singleton tuple from `EEZ_PROVER_URL`, `EEZ_ATTESTER_ADDRESS` and `EEZ_ECDSA_PROOF_SYSTEM_ADDRESS`. An explicitly empty or malformed roster fails startup. A valid explicit roster takes precedence over all three settings. Compose passes the roster through to the Composer service. After editing a packaged roster in the env file, recreate the Composer with the same Compose files and project using `docker compose up -d --no-deps --force-recreate eez-node`. A plain container restart retains its old environment. Degraded startup uses the block ceilings of the admitted cohort. If a recovering member has a lower ceiling, it can serve requests that fit it. Provision comparable signer capacity for failover during a backlog. A smaller signer may be unable to restore quorum for a larger window until the cohort or planning limit is changed. If a previously admitted signer changes its limits, its client rejects the changed capabilities. Restore the agreed limits or restart Composer to plan against the new configuration. Queued work is in memory and clients may need to resubmit.

The existing Core ABI, signer protocol, validation identity and historical batch format apply throughout. Older plaintext v2 signer binaries can serve matching singleton views on loopback. Before rolling back to a singleton Composer, establish that one registered system can meet the active threshold and that the legacy tuple is valid. A threshold above one requires an authorized on-chain policy change or a quorum-capable Composer to resume settlement.

## Share a pool of remote provers

Verified on an isolated rollup with two Composers sharing three provers at threshold two, using source built from the #151 candidate based on `5d8748da`. The three packaged stacks were rendered offline. Separate local Anvil deployments exercised both initial-attester inputs and verified the registered signer and vkey. Release-image and live Kurtosis qualification are pending.

Each Composer connects directly to each provider using the existing RollupX v2 API. Each prover operator chooses which Composers can request work. The rollup manager's threshold determines how many verified proofs a batch needs. Two Composers can use the same roster with separate posting wallets, writable datadirs and client keys.

**For a new rollup**, obtain the initial provider’s public Ethereum attester address through an authenticated handoff. Set `EEZ_DEPLOY_ATTESTER_ADDRESS` in the deployment env file and remove the `EEZ_PROOF_SIGNER_KEY` assignment. Use a deploy script or image containing this change. Verify the address with the provider before broadcasting. The verifier fixes it at construction. Use the checksummed address supplied by the provider when available. Mixed-case inputs must pass the EIP-55 checksum. The local-signer workflow can still supply the key alone. If both values are supplied they must identify the same address. An explicitly set address must contain `0x` followed by 40 hex digits and must be nonzero and distinct from the reserved native system address.

The [deployment procedure](chiado-quickstart.md#3-deploy-contracts--once) creates the initial verifier and registers it at threshold one. Share the public bindings in `deployments.env` and the corresponding genesis/profile with the provider. Keep the deployment `.env` private. Use the emitted `EEZ_ECDSA_PROOF_SYSTEM_ADDRESS` and `EEZ_ATTESTER_ADDRESS` for the first roster entry. Before starting the Composer, confirm the verifier’s `signer()` matches the address the provider supplied and have the provider check its independently derived vkey against `EEZ_VKEY`. See the [deployment identity checks](chiado-deployment.md#deployment-identity) and [vkey derivation](chiado-deployment.md#rollup-manager-and-coordinated-upgrades). Register any additional providers with the manager before enabling their roster entries. Prover TLS certificates are provisioned separately.

**For an existing rollup**, use the existing public registrations and compatible mTLS-enabled binaries. An additional Composer uses the network’s normal bootstrap procedure.

### On each prover host

1. Give both Composer operators the prover's HTTPS origin, server CA certificate, registered attester address and proof-system address. The server certificate needs a DNS or IP subject alternative name matching the origin and must permit server authentication.
2. Have each Composer operator generate its client key locally and obtain a client-auth certificate from an issuer you trust. Have them return their public leaf and issuer chain. Verify the operator, leaf and any new trust anchor through your agreed handoff. Each issuer can sign other certificates, so grant the two specific leaves. Produce one line per leaf with the following command, substituting the public certificate filename. Put the lines in `allowed-clients.sha256` with no duplicates, comments or blank lines.

   ```bash
   set -o pipefail
   openssl x509 -in composer-a.pem -noout -sha256 -fingerprint | cut -d= -f2 | tr -d ':'
   ```

3. Set `EEZ_PROOF_SIGNER_TLS_DIR` to an absolute directory with these files:

   | File | Contents |
   | --- | --- |
   | `cert.pem`, `key.pem` | Server leaf followed by any intermediates, and its matching private key |
   | `ca.pem` | Trusted issuers of Composer client certificates |
   | `allowed-clients.sha256` | One granted client leaf fingerprint per line |

   With the packaged signer image, mount the directory read-only. UID 65532 needs read access to the files and traversal access to the directories.

4. Load the [normal chain and attestation settings](../../crates/eez-proof-signer/SPEC.md#3-startup-configuration). Start the standalone signer on its service IP, replacing the example address and directory:

   ```bash
   eez-proof-signer --listen-addr 192.0.2.10:50061 --tls-dir /srv/eez/prover-tls
   ```

To add another Composer to a running pool, add its leaf fingerprint to each provider’s grant file. Restart each granting prover in turn and verify it before moving on. A 2-of-2 pool can miss a settlement window while either member is down. Each signer shares its validation worker across callers, so one Composer’s load can delay the other.

### On each Composer host

Set the roster and the absolute host credential directory. For example, with two registered providers, substitute their public addresses in the four shell variables below. For a packaged stack, save the resulting two assignments with literal values in the stack env file so they survive a new shell. Repeat with the same roster on the other Composer host.

```bash
export EEZ_PROVERS="https://p1.example:50061=${ATTESTER_1:?}=${VERIFIER_1:?},https://p2.example:50061=${ATTESTER_2:?}=${VERIFIER_2:?}"
export EEZ_PROVER_TLS_DIR=/srv/eez/composer-prover-tls
```

Create one subdirectory per **exact URL authority**, including the port when specified. For example, `p1.example:50061/` holds `ca.pem` (trusts that provider's server), `cert.pem` (this Composer's client leaf for that provider followed by any intermediates) and `key.pem` (matching client key). An IPv6 example is `[::1]:50061/`. Each Composer needs its own leaf at each provider so its sessions and grants are independent. Provision separate client keys when cloning a host. Reusing a leaf and key gives both instances the same session identity. Use restricted file permissions and mount the directory read-only. An unreachable provider may leave enough members for quorum. A missing or malformed local credential for any selected member prevents startup.

A standalone Composer uses those settings directly.

### Run a packaged Composer without a local signer

Omit the local signer key and image from the new configuration. **When converting an existing stack**, first verify the old project and services with the original env and base file. During the scheduled cutover stop and remove just that project’s local signer before changing its env or file selection. For a Chiado stack running from this checkout:

```bash
docker compose -f docker-compose.chiado-node.yml ps --all eez-proof-signer
docker compose -f docker-compose.chiado-node.yml rm --stop --force eez-proof-signer
```

Use the original Ethereum or external Kurtosis base file, env file and project selection for the equivalent cutover. Preserve any unrelated services and all volumes.

With Compose 5.5.1, load the base file first and [the remote-provers overlay](../../docker-compose.remote-provers.yml) second for **every** command. For the external Kurtosis stack also pass its usual `--env-file .env.kurtosis`. The overlay mounts the host credential directory at `/run/eez/prover-tls` inside the node. On Chiado, check the service list from the repository root:

```bash
docker compose -f docker-compose.chiado-node.yml -f docker-compose.remote-provers.yml config --services
```

Expect just `eez-node` and `lighthouse`. Use `bash scripts/chiado-up.sh --remote-provers` for startup and `bash scripts/teardown-chiado.sh --remote-provers` for teardown.

### Verify the pool

- On **each** Composer check the startup event `eez.node.prover.ready`. Its `attesters` count is the number admitted at startup. For a fully available three-provider roster expect three, even when two are enough to start. Compare the proof-system address on a member warning with the roster.
- If the node reports `HTTPS prover requires EEZ_PROVER_TLS_DIR`, check the selected Compose files and client mount. A `read …/ca.pem` error points to a missing or unreadable file under the exact authority directory. If TLS is configured, any HTTP entry makes the whole roster invalid.
- A TLS handshake failure can indicate an untrusted issuer, unsuitable certificate or mismatched server name. A gRPC `PERMISSION_DENIED` with `client certificate is not authorized` means the TLS handshake succeeded and the prover did not grant that leaf. Check its fingerprint against the running prover's grant set. A timeout alone cannot identify the cause.
- Verify a canonically settled batch and its threshold of verified proofs. When testing two active Composers, attribute a post to each by its transaction sender. Both can observe the same settlement. The [existing-rollup verification procedure](chiado-existing-rollup.md#3-start-catch-up-and-verify) shows the check.

For local development omit both TLS directory settings and use a literal loopback address such as `http://127.0.0.1:50061`. Older configurations using `http://localhost` must switch to a literal such as `127.0.0.1`. A prover addressed by container service name or private IP needs HTTPS and client credentials. Processes with access to the loopback socket are trusted. Keep it out of port forwards, proxies and published container ports. Setting a Composer TLS directory requires HTTPS for every member, including loopback.

### Rotation, withdrawal and rollback

Credentials and grants are snapshots loaded at startup. For a directory bind mount, replace files inside the mounted directory and restart the process. If you replace the bound directory itself or change its host path, recreate the container so it mounts the new directory. To renew a client, first provision both old and new fingerprints on each affected prover and restart those signers, preserving a viable quorum where possible. Switch the Composer to its new certificate and restart it. A new leaf owns new sessions. Then remove the old grant and restart every serving instance. For CA or server identity changes, stage the new trust anchors on the peer first and retain the previous valid chain during the transition. Verify the replacement name and issuer before removing old trust.

For emergency withdrawal, stop every serving instance that still trusts the withdrawn leaf, allow owned workers to drain, and restart with the reduced grant set. File edits, certificate expiry and removing an issuer alone do not terminate established channels. Confirm the former caller is denied on existing/reconnected and fresh channels and a retained caller still obtains a proof. Downtime can cost a settlement window. Signer sessions are volatile, so clients may need to Begin again and backfill within the original deadline. Composer restart may require resubmission of unsettled requests. A signature already issued remains valid.

Remote cutover requires compatible mTLS clients and servers. Stage a new endpoint with the intended grants, verify it, move the roster, and retire every old plaintext ingress. Roll back to a verified, protected pair or take the affected member offline. Never reduce the manager threshold to work around a transport failure.
