use alloy_primitives::B256;
use anyhow::{Context, Result, ensure};
use tokio_postgres::{Client, NoTls};

use crate::{
    config::Config,
    model::{Block, Event, Receipt},
};

pub struct Database {
    pub client: Client,
}

impl Database {
    pub async fn connect(url: &str, config: &Config) -> Result<Self> {
        let mut settings: tokio_postgres::Config = url.parse()?;
        settings.connect_timeout(std::time::Duration::from_secs(10));
        let (mut client, connection) = settings.connect(NoTls).await?;
        tokio::spawn(async move {
            if connection.await.is_err() {
                tracing::error!("PostgreSQL connection closed");
            }
        });
        client
            .batch_execute("SET statement_timeout='10s'; SET lock_timeout='5s'")
            .await?;
        // This database is dedicated to one source. The session lock prevents
        // two instances racing cursor advancement or reorg deletion.
        let locked: bool = client
            .query_one("SELECT pg_try_advisory_lock(519763211)", &[])
            .await?
            .get(0);
        ensure!(locked, "another eez-indexer writer holds this database");
        let tx = client.transaction().await?;
        tx.batch_execute(include_str!("../schema.sql")).await?;
        let identity = [
            config.chain_id.to_string(),
            config.genesis_hash.to_string(),
            config.registry.to_string(),
            config.rollup_id.to_string(),
        ];
        let start = i64::try_from(config.start_block)?;
        tx.execute("INSERT INTO eez_indexer.source(schema_version,chain_id,genesis_hash,registry,rollup_id,start_block) VALUES(1,$1,$2,$3,$4,$5) ON CONFLICT DO NOTHING", &[&identity[0],&identity[1],&identity[2],&identity[3],&start]).await?;
        let row = tx.query_one("SELECT schema_version,chain_id,genesis_hash,registry,rollup_id,start_block FROM eez_indexer.source", &[]).await?;
        ensure!(
            row.get::<_, i32>(0) == 1
                && (0..4).all(|i| row.get::<_, String>(i + 1) == identity[i])
                && row.get::<_, i64>(5) == start,
            "database source/configuration mismatch"
        );
        if let Some(details) = &config.details {
            let chain = details.l2_chain_id.to_string();
            let genesis = details.l2_genesis_hash.to_string();
            tx.execute("INSERT INTO eez_indexer.details_source(chain_id,genesis_hash) VALUES($1,$2) ON CONFLICT DO NOTHING", &[&chain,&genesis]).await?;
            let row = tx
                .query_one(
                    "SELECT chain_id,genesis_hash FROM eez_indexer.details_source",
                    &[],
                )
                .await?;
            ensure!(
                row.get::<_, String>(0) == chain && row.get::<_, String>(1) == genesis,
                "database L2 identity mismatch"
            );
        }
        tx.commit().await?;
        Ok(Self { client })
    }
    pub async fn cursor(&self) -> Result<Option<(u64, B256)>> {
        self.client
            .query_opt(
                "SELECT height,hash FROM eez_indexer.blocks ORDER BY height DESC LIMIT 1",
                &[],
            )
            .await?
            .map(|r| {
                Ok((
                    u64::try_from(r.get::<_, i64>(0))?,
                    r.get::<_, String>(1).parse()?,
                ))
            })
            .transpose()
    }
    pub async fn hash(&self, height: u64) -> Result<Option<B256>> {
        self.client
            .query_opt(
                "SELECT hash FROM eez_indexer.blocks WHERE height=$1",
                &[&i64::try_from(height)?],
            )
            .await?
            .map(|r| Ok(r.get::<_, String>(0).parse()?))
            .transpose()
    }
    pub async fn rewind(&mut self, ancestor: u64) -> Result<()> {
        let tx = self.client.transaction().await?;
        tx.execute(
            "DELETE FROM eez_indexer.blocks WHERE height > $1",
            &[&i64::try_from(ancestor)?],
        )
        .await?;
        tx.execute("UPDATE eez_indexer.source SET checked_at=NULL", &[])
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn store(
        &mut self,
        block: &Block,
        events: &[Event],
        receipts: &[Receipt],
        start: u64,
    ) -> Result<()> {
        if let Some((number, hash)) = self.cursor().await? {
            if block.number == number && block.hash == hash {
                return Ok(());
            }
            ensure!(
                block.number == number + 1 && block.parent == hash,
                "noncontiguous database append"
            );
        } else {
            ensure!(block.number == start, "wrong initial block");
        }
        let tx = self.client.transaction().await?;
        let block_hash = block.hash.to_string();
        tx.execute(
            "INSERT INTO eez_indexer.blocks(height,hash,parent_hash,timestamp) VALUES($1,$2,$3,$4)",
            &[
                &i64::try_from(block.number)?,
                &block_hash,
                &block.parent.to_string(),
                &i64::try_from(block.timestamp)?,
            ],
        )
        .await?;
        for r in receipts {
            tx.execute("INSERT INTO eez_indexer.transactions VALUES($1,$2,$3,$4::text::numeric,$5::text::numeric,$6::text::numeric,$7::text::numeric,$8::text::numeric,$9::text::numeric)", &[&block_hash,&r.transaction.to_string(),&i64::try_from(r.index)?,&r.gas_used.to_string(),&r.gas_price.to_string(),&r.execution_fee.to_string(),&r.blob_gas_used.to_string(),&r.blob_gas_price.to_string(),&r.blob_fee.to_string()]).await?;
        }
        for e in events {
            ensure!(
                e.block_hash == block.hash && e.block_number == block.number,
                "event attribution changed"
            );
            tx.execute(
                "INSERT INTO eez_indexer.settlements VALUES($1,$2,$3,$4)",
                &[
                    &block_hash,
                    &e.transaction.to_string(),
                    &i64::try_from(e.log_index)?,
                    &e.endpoint.to_string(),
                ],
            )
            .await?;
        }
        tx.commit().await.context("commit indexed block")?;
        Ok(())
    }
    pub async fn checked(&self, head: u64) -> Result<()> {
        self.client
            .execute(
                "UPDATE eez_indexer.source SET checked_at=now(),observed_head=$1",
                &[&i64::try_from(head)?],
            )
            .await?;
        Ok(())
    }
}
