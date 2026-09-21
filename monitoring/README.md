# Minimal monitoring foundation

This optional stack runs Grafana and Prometheus on the same Linux machine as the
packaged Chiado Composer. It scrapes only Reth's existing localhost metrics
endpoint and adds no EEZ runtime instrumentation, dashboards, alerts, traces,
logs, or database observer.

Grafana, Prometheus, and the Composer metrics endpoint all bind to localhost.
The monitoring services use host networking solely to reach that private Reth
endpoint without exposing it on another interface. Prometheus is provisioned as
Grafana's default datasource.

## Start

From this directory:

```bash
cp .env.monitoring.example .env
umask 077
chmod 700 secrets
openssl rand -out secrets/grafana-admin-password -base64 32
chmod 644 secrets/grafana-admin-password
docker compose config --quiet
docker compose up --detach
docker compose ps
```

Open Grafana at `http://127.0.0.1:19300`. The default username is `admin`; its
password is the contents of `secrets/grafana-admin-password`. Prometheus is at
`http://127.0.0.1:19090`.

The packaged `docker-compose.chiado-node.yml` enables Reth metrics at
`http://127.0.0.1:19091/metrics`. If the Composer is not running from that
definition, Prometheus will correctly report its only target as down.

Change the local ports, retention, or admin username in `.env` before starting.
The `.env` file and password are ignored by Git. The password file must be
readable by Grafana's unprivileged container user; its private parent directory
prevents other host users from traversing to it.

## Operate

Check service health:

```bash
docker compose ps
curl --fail --silent --show-error http://127.0.0.1:19090/-/ready
curl --fail --silent --show-error http://127.0.0.1:19300/api/health
curl --fail --silent --show-error http://127.0.0.1:19090/api/v1/targets
```

Stop the services while retaining their data:

```bash
docker compose down
```

The named `prometheus-data` and `grafana-data` volumes survive ordinary Compose
restarts and `docker compose down`. Prometheus contains only the existing Reth
metrics from one Composer, and the Grafana datasource is reproducible from the
checked-in provisioning file. Portable backup and restore procedures will be
added before the stack holds custom EEZ metrics or user-created dashboards.

## Upgrade

Image tags and digests are pinned in `compose.yaml`. Change one image at a time,
review its release notes, render the Compose configuration, and verify health
before removing the previous image from the host.

## Scope boundary

The next change should inventory the observed Reth series and select a few stable
signals for one small dashboard. Custom EEZ metrics, signer or follower metrics,
PostgreSQL, OpenTelemetry, tracing, log collection, public access, and alerts
remain separate decisions.
