use alloy_primitives::{Address, B256};
use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http_url: String,
    pub ws_url: String,
    pub chain_id: u64,
    pub genesis_hash: B256,
    pub registry: Address,
    pub rollup_id: u64,
    pub start_block: u64,
    #[serde(default)]
    pub details: Option<DetailsConfig>,
    #[serde(default)]
    pub posting_logs_dir: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailsConfig {
    #[serde(default)]
    pub l1_http_url: Option<String>,
    pub l2_http_url: String,
    pub l2_chain_id: u64,
    pub l2_genesis_hash: B256,
    pub beacon_url: String,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if let Some(path) = &self.posting_logs_dir {
            ensure!(
                path.is_absolute() && path.is_dir(),
                "posting_logs_dir must be an existing absolute directory"
            );
            std::fs::read_dir(path)?;
        }
        if let Some(details) = &self.details {
            ensure!(
                details.l2_chain_id > 0 && details.l2_genesis_hash != B256::ZERO,
                "invalid L2 identity"
            );
            for value in [&details.l2_http_url, &details.beacon_url]
                .into_iter()
                .chain(details.l1_http_url.iter())
            {
                let url = reqwest::Url::parse(value)?;
                ensure!(
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.fragment().is_none(),
                    "invalid details HTTP URL"
                );
            }
        }
        for (value, schemes) in [
            (&self.http_url, ["http", "https"]),
            (&self.ws_url, ["ws", "wss"]),
        ] {
            let url = reqwest::Url::parse(value)?;
            ensure!(
                schemes.contains(&url.scheme()) && url.host_str().is_some(),
                "invalid RPC URL scheme/host"
            );
            ensure!(
                url.fragment().is_none(),
                "RPC URL must not contain a fragment"
            );
        }
        ensure!(
            self.chain_id > 0 && self.rollup_id > 0,
            "chain and rollup IDs must be positive"
        );
        ensure!(
            self.start_block > 0 && self.start_block < i64::MAX as u64,
            "start_block must be in 1..i64::MAX"
        );
        ensure!(
            self.registry != Address::ZERO && self.genesis_hash != B256::ZERO,
            "registry and genesis must be nonzero"
        );
        Ok(())
    }
}
