//! Explicit deployment identity; no ambient node credentials or deployment files.
use std::{fmt, net::SocketAddr, path::Path};

use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub url: String,
    pub chain_id: u64,
    pub genesis_hash: String,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("chain_id", &self.chain_id)
            .field("genesis_hash", &self.genesis_hash)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub l1: Endpoint,
    pub l2: Endpoint,
    pub reference_l1_url: Option<String>,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub allow_non_loopback: bool,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .finish_non_exhaustive()
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 19100))
}

pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_url(value: &str) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid RPC URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err("RPC URLs must be HTTP(S) URLs without fragments");
    }
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let bytes = std::fs::read(path).map_err(|_| "cannot read observer configuration")?;
        // Do not include serde's error: an invalid value can contain a credential.
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|_| "invalid observer JSON configuration")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        for endpoint in [&self.l1, &self.l2] {
            validate_url(&endpoint.url)?;
            if !valid_hash(&endpoint.genesis_hash) {
                return Err("expected genesis_hash must be a 32-byte 0x-prefixed hash");
            }
        }
        if let Some(url) = &self.reference_l1_url {
            validate_url(url)?;
        }
        if !self.listen.ip().is_loopback() && !self.allow_non_loopback {
            return Err("non-loopback metrics binding requires allow_non_loopback=true");
        }
        Ok(())
    }
}
