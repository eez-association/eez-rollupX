#!/bin/sh
# First cluster initialization creates identities only. The observer schema is
# installed separately by `eez-observer migrate` with administrative credentials.
set -eu
EEZ_WRITER_PASSWORD=$(cat /run/secrets/postgres_writer_password)
EEZ_READER_PASSWORD=$(cat /run/secrets/postgres_reader_password)
export EEZ_WRITER_PASSWORD EEZ_READER_PASSWORD
psql -v ON_ERROR_STOP=1 --username postgres --dbname eez_observer <<'SQL'
\getenv writer_password EEZ_WRITER_PASSWORD
\getenv reader_password EEZ_READER_PASSWORD
CREATE ROLE eez_observer_writer LOGIN PASSWORD :'writer_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
CREATE ROLE eez_observer_reader LOGIN PASSWORD :'reader_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
SQL
unset EEZ_WRITER_PASSWORD EEZ_READER_PASSWORD
