#!/bin/sh
set -eu
# psql quotes the value as an SQL literal; passwords are never interpolated as SQL.
psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" --set=reader_password="$INDEXER_GRAFANA_PASSWORD" --set=ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE eez_grafana LOGIN PASSWORD :'reader_password';
ALTER ROLE eez_grafana SET default_transaction_read_only = on;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
CREATE SCHEMA eez_indexer AUTHORIZATION eez_indexer;
GRANT CONNECT ON DATABASE eez_indexer TO eez_grafana;
GRANT USAGE ON SCHEMA eez_indexer TO eez_grafana;
ALTER DEFAULT PRIVILEGES FOR ROLE eez_indexer IN SCHEMA eez_indexer GRANT SELECT ON TABLES TO eez_grafana;
SQL
