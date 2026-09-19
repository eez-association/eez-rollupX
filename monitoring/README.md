# Minimal monitoring foundation

This optional stack runs Grafana and Prometheus on one machine. It does not
modify an EEZ node and initially has no EEZ scrape targets, dashboards, alerts,
traces, logs, or database observer.

Both ports bind to localhost. Prometheus is provisioned as Grafana's default
datasource, ready for later metric endpoints.

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
```

Stop the services while retaining their data:

```bash
docker compose down
```

The named `prometheus-data` and `grafana-data` volumes survive ordinary Compose
restarts and `docker compose down`. At this stage Prometheus contains no EEZ data,
and the Grafana datasource is reproducible from the checked-in provisioning file.
Portable backup and restore procedures will be added when the stack starts
holding operational data or user-created dashboards.

## Upgrade

Image tags and digests are pinned in `compose.yaml`. Change one image at a time,
review its release notes, render the Compose configuration, and verify health
before removing the previous image from the host.

## Scope boundary

The next change should define one small native metric contract and an optional,
localhost-only endpoint for one EEZ service. PostgreSQL, OpenTelemetry, tracing,
log collection, public access, dashboards, and alerts remain separate decisions.
