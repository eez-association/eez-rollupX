//! Explicit administrative migrations and bounded private database connections.
use std::{fmt, fs::File, io::Read, path::Path, str::FromStr, time::Duration};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_postgres::{
    Client, NoTls,
    config::{Host, SslMode},
};

pub const SCHEMA_VERSION: i32 = 1;
const MIGRATIONS: &[(i32, &str)] = &[(
    1,
    include_str!("../../../observability/migrations/0001-canonical.sql"),
)];
const CONFIG_LIMIT: u64 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub connection_string: String,
    /// Explicitly attest private-network/tunnel transport when using TCP.
    /// Unix sockets need no TCP plaintext opt-in. TLS requests are never ignored.
    #[serde(default)]
    pub allow_private_plaintext: bool,
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseConfig")
            .finish_non_exhaustive()
    }
}

impl DatabaseConfig {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| "cannot open observer database config")?
            .take(CONFIG_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read observer database config")?;
        if bytes.len() as u64 > CONFIG_LIMIT {
            return Err("observer database config exceeds 64 KiB");
        }
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|_| "invalid observer database config")?;
        config.parsed()?;
        Ok(config)
    }

    fn parsed(&self) -> Result<tokio_postgres::Config, &'static str> {
        let mut config = tokio_postgres::Config::from_str(&self.connection_string)
            .map_err(|_| "invalid PostgreSQL connection settings")?;
        if config.get_hosts().len() != 1
            || !config.get_hostaddrs().is_empty()
            || config.get_user().is_none_or(str::is_empty)
            || config.get_dbname().is_none_or(str::is_empty)
            || config.get_ports().len() > 1
            || config.get_ports().contains(&0)
        {
            return Err(
                "PostgreSQL requires one explicit host, user and database, without hostaddr overrides",
            );
        }
        let tcp = matches!(config.get_hosts()[0], Host::Tcp(_));
        if matches!(&config.get_hosts()[0], Host::Tcp(host) if host.is_empty()) {
            return Err("PostgreSQL host must not be empty");
        }
        if config.get_ssl_mode() != SslMode::Disable || (tcp && !self.allow_private_plaintext) {
            return Err(
                "PostgreSQL requires sslmode=disable over a Unix socket or explicitly approved private-network/tunnel transport",
            );
        }
        config.connect_timeout(CONNECT_TIMEOUT);
        config.application_name("eez-observer");
        Ok(config)
    }
}

pub struct Database {
    pub(crate) client: Client,
    connection: tokio::task::JoinHandle<()>,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Database").finish_non_exhaustive()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

fn history_matches(rows: &[tokio_postgres::Row]) -> bool {
    !rows.is_empty()
        && rows.len() <= MIGRATIONS.len()
        && rows.iter().zip(MIGRATIONS).all(|(row, (expected, sql))| {
            let version = row.try_get::<_, i32>(0);
            let stored = row.try_get::<_, Vec<u8>>(1);
            let checksum: [u8; 32] = Sha256::digest(sql.as_bytes()).into();
            version.is_ok_and(|value| value == *expected)
                && stored.is_ok_and(|value| value == checksum)
        })
}

impl Database {
    pub async fn connect(config: &DatabaseConfig) -> Result<Self, &'static str> {
        let (client, connection) =
            tokio::time::timeout(CONNECT_TIMEOUT, config.parsed()?.connect(NoTls))
                .await
                .map_err(|_| "PostgreSQL connection timed out")?
                .map_err(|_| "PostgreSQL connection failed")?;
        let task = tokio::spawn(async move {
            if connection.await.is_err() {
                tracing::warn!(
                    reason = "connection_failed",
                    "observer database connection ended"
                );
            }
        });
        let database = Self {
            client,
            connection: task,
        };
        // Set server-side deadlines as well as the connection timeout. If the
        // caller is cancelled, closing this dedicated connection rolls back its
        // transaction; no unbounded orphan query is left on the server.
        tokio::time::timeout(Duration::from_secs(10), database.client.batch_execute("SET statement_timeout = '10s'; SET lock_timeout = '5s'; SET idle_in_transaction_session_timeout = '10s'; SET search_path = pg_catalog, eez;"))
            .await.map_err(|_| "observer database session setup timed out")?
            .map_err(|_| "cannot configure observer database session")?;
        Ok(database)
    }

    /// Must be called by normal indexing startup. Never performs DDL.
    pub async fn verify_schema(&self) -> Result<(), &'static str> {
        let rows = tokio::time::timeout(
            Duration::from_secs(10),
            self.client.query(
                "SELECT version, checksum::bytea FROM eez.schema_migrations ORDER BY version",
                &[],
            ),
        )
        .await
        .map_err(|_| "observer schema verification timed out")?
        .map_err(|_| "observer schema missing; run the explicit migrate command")?;
        if rows.len() != MIGRATIONS.len() || !history_matches(&rows) {
            return Err("observer schema version or migration checksum is incompatible");
        }
        Ok(())
    }

    pub async fn verify_writer(&self) -> Result<(), &'static str> {
        self.verify_schema().await?;
        let row = tokio::time::timeout(Duration::from_secs(10), self.client.query_one("SELECT pg_has_role(current_user, 'eez_observer_writer', 'MEMBER') AND NOT (rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls) FROM pg_roles WHERE rolname = current_user", &[]))
            .await.map_err(|_| "observer database role verification timed out")?
            .map_err(|_| "cannot verify observer database role")?;
        if !row.try_get::<_, bool>(0).unwrap_or(false) {
            return Err("indexer requires the dedicated unprivileged observer writer role");
        }
        Ok(())
    }

    /// Only the CLI's administrative subcommand calls this. Transaction-scoped
    /// advisory locking makes repeated/concurrent migration invocations safe.
    pub async fn migrate(&mut self) -> Result<(), &'static str> {
        match tokio::time::timeout(Duration::from_secs(30), self.migrate_inner()).await {
            Ok(result) => result,
            Err(_) => {
                self.connection.abort();
                Err(
                    "observer schema migration timed out; reconnect to verify its committed version",
                )
            }
        }
    }

    async fn migrate_inner(&mut self) -> Result<(), &'static str> {
        let transaction = self
            .client
            .transaction()
            .await
            .map_err(|_| "cannot begin schema migration")?;
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtext('eez-observer-migrations'))",
                &[],
            )
            .await
            .map_err(|_| "cannot acquire migration lock")?;
        let present: bool = transaction
            .query_one("SELECT to_regnamespace('eez') IS NOT NULL", &[])
            .await
            .map_err(|_| "cannot inspect observer schema")?
            .get(0);
        let applied = if present {
            let rows = transaction
                .query(
                    "SELECT version, checksum::bytea FROM eez.schema_migrations ORDER BY version",
                    &[],
                )
                .await
                .map_err(|_| "existing eez schema has no compatible migration history")?;
            if !history_matches(&rows) {
                return Err(
                    "existing observer schema is incompatible; automatic downgrade or rewrite is prohibited",
                );
            }
            rows.len()
        } else {
            0
        };
        for (version, sql) in MIGRATIONS.iter().skip(applied) {
            transaction
                .batch_execute(sql)
                .await
                .map_err(|_| "observer schema migration failed")?;
            let checksum: [u8; 32] = Sha256::digest(sql.as_bytes()).into();
            transaction
                .execute(
                    "INSERT INTO eez.schema_migrations(version, checksum) VALUES ($1, $2::bytea)",
                    &[version, &&checksum[..]],
                )
                .await
                .map_err(|_| "cannot record observer schema version")?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| "cannot commit observer schema migration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_redacted_and_transport_must_be_explicit() {
        let mut config = DatabaseConfig { connection_string:"host=localhost user=eez_observer_writer dbname=eez_observer password=never-print-this sslmode=disable".into(), allow_private_plaintext:false };
        assert!(!format!("{config:?}").contains("never-print-this"));
        assert!(config.parsed().is_err());
        config.allow_private_plaintext = true;
        assert!(config.parsed().is_ok());
        config.connection_string = config
            .connection_string
            .replace("sslmode=disable", "sslmode=require");
        assert!(config.parsed().is_err());
        config.connection_string = "host=localhost sslmode=disable".into();
        assert!(config.parsed().is_err());
        config.connection_string =
            "host=localhost,other user=writer dbname=observer sslmode=disable".into();
        assert!(config.parsed().is_err());
    }
}
