//! Configuration for read-only L1 access and signed batch submission,
//! populated from `EEZ_*` environment variables.
//!
//! Per-rollup composer/orchestration knobs remain in
//! `eez-composer::RollupConfig`; the registry deploy block lives here because
//! it defines the lower bound of the L1 reader's historical scan.

use std::{env, path::PathBuf, str::FromStr, time::Duration};

use alloy_primitives::Address;
use alloy_signer_local::PrivateKeySigner;
use url::Url;

use crate::error::{L1Error, L1Result};

const ENV_RPC_URL: &str = "EEZ_L1_RPC_URL";
const ENV_BEACON_URL: &str = "EEZ_L1_BEACON_URL";
const ENV_BUILDER_RPC_URL: &str = "EEZ_L1_BUILDER_RPC_URL";
/// Not a supported setting. Rejected so a plausible misspelling of the builder
/// list cannot be silently ignored in favor of `EEZ_L1_BUILDER_RPC_URL`.
const ENV_BUILDER_RPC_URLS_UNSUPPORTED: &str = "EEZ_L1_BUILDER_RPC_URLS";
const ENV_TARGET_RPC_URL: &str = "EEZ_L1_TARGET_RPC_URL";
const ENV_POSTER_KEY: &str = "EEZ_L1_POSTER_KEY";
const ENV_EEZ_ADDRESS: &str = "EEZ_REGISTRY_ADDRESS";
const ENV_ROLLUP_ID: &str = "EEZ_ROLLUP_ID";
const ENV_BUNDLE_CAPTURE_DIR: &str = "EEZ_BUNDLE_CAPTURE_DIR";
const ENV_BUNDLE_SUBMISSION_LEAD_MS: &str = "EEZ_BUNDLE_SUBMISSION_LEAD_MS";

/// Default earliest-send offset before an L1 target timestamp. The 500 ms
/// difference from rbuilder's common T-8s gate is deliberate clock/network
/// margin: a request emitted at T-7.5s reaches a correctly synchronized
/// builder after its gate has opened.
pub const DEFAULT_BUNDLE_SUBMISSION_LEAD_MS: u64 = 7_500;
const ENV_REGISTRY_DEPLOY_BLOCK: &str = "EEZ_REGISTRY_DEPLOY_BLOCK";

/// Read-only L1 connectivity used by the Deriver's canonical-chain scans.
#[derive(Clone)]
pub struct L1ReaderConfig {
    /// L1 RPC endpoint (HTTP / HTTPS).
    pub rpc_url: Url,
    /// Consensus-layer REST endpoint used to retrieve mandatory native blob
    /// sidecars. Environment construction always populates this value.
    pub beacon_url: Option<Url>,
    /// Deployed `EEZ` (rollups registry) address.
    pub eez: Address,
    /// Our rollup's id. Used by the batch-log scanner to filter the
    /// `L2ExecutionPerformed(rollupId indexed, ...)` event topic so
    /// each historical batch is tagged winner / loser.
    pub rollup_id: u64,
    /// L1 block where `EEZ` was deployed. Lower bound for historical batch
    /// scans and boot-time source-readiness checks.
    pub deploy_block: u64,
}

impl std::fmt::Debug for L1ReaderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L1ReaderConfig")
            .field("rpc_url", &self.rpc_url.as_str())
            .field("beacon_url", &self.beacon_url.as_ref().map(Url::as_str))
            .field("eez", &self.eez)
            .field("rollup_id", &self.rollup_id)
            .field("deploy_block", &self.deploy_block)
            .finish()
    }
}

impl L1ReaderConfig {
    /// Read the canonical-chain scan configuration from `EEZ_*` env vars.
    ///
    /// # Errors
    ///
    /// Returns [`L1Error::Config`] for any missing required var or
    /// malformed value.
    pub fn from_env() -> L1Result<Self> {
        Ok(Self {
            rpc_url: parse_url(ENV_RPC_URL)?,
            beacon_url: Some(parse_url(ENV_BEACON_URL)?),
            eez: parse_address(ENV_EEZ_ADDRESS)?,
            rollup_id: parse_u64(ENV_ROLLUP_ID)?,
            deploy_block: parse_u64(ENV_REGISTRY_DEPLOY_BLOCK)?,
        })
    }
}

/// Signed submission configuration. Read-side settings are grouped in
/// [`L1ReaderConfig`]; the remaining fields exist only for posting batches.
#[derive(Clone)]
pub struct SubmitterConfig {
    /// Canonical L1 source used for reads and as the default target-tip RPC.
    pub reader: L1ReaderConfig,
    /// L1 builder relays accepting `eth_sendBundle`. Every postBatch bundle is
    /// sent, byte-identical, to all of them; [`L1ReaderConfig::rpc_url`] is
    /// used for reads only.
    pub builder_relays: BuilderRelays,
    /// Optional canonical target-chain RPC used for target selection,
    /// pre-dispatch revalidation, and bundle observation. The embedded L1 can
    /// lag the canonical tip by 2-3 blocks, so point this at a tip-following
    /// node when submission goes through an external builder. `None` falls
    /// back to [`L1ReaderConfig::rpc_url`].
    pub target_rpc_url: Option<Url>,
    /// EOA that signs L1 txs (pays gas).
    pub poster: PrivateKeySigner,
    /// Optional directory for exact `eth_sendBundle` request captures when an
    /// attempt is dropped or rejected. Unset keeps production writes off.
    pub bundle_capture_dir: Option<PathBuf>,
    /// Do not dispatch a bundle before `target_timestamp - lead`. This avoids
    /// relays/builders that acknowledge early bundles but discard the only
    /// candidate produced before their bidding gate opens.
    pub bundle_submission_lead: Duration,
}

impl std::fmt::Debug for SubmitterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubmitterConfig")
            .field("reader", &self.reader)
            .field("builder_relays", &self.builder_relays)
            .field(
                "target_rpc_url",
                &self.target_rpc_url.as_ref().map(Url::as_str),
            )
            .field("poster", &self.poster.address())
            .field("bundle_capture_dir", &self.bundle_capture_dir)
            .field("bundle_submission_lead", &self.bundle_submission_lead)
            .finish()
    }
}

impl SubmitterConfig {
    /// Read from `EEZ_*` env vars.
    ///
    /// # Errors
    ///
    /// Returns [`L1Error::Config`] for any missing required var or
    /// malformed value.
    pub fn from_env() -> L1Result<Self> {
        let target_rpc_url = match env::var(ENV_TARGET_RPC_URL) {
            Ok(raw) if !raw.is_empty() => Some(
                Url::parse(&raw)
                    .map_err(|e| L1Error::Config(format!("{ENV_TARGET_RPC_URL}: {e}")))?,
            ),
            _ => None,
        };
        // Builder relay endpoints. On chiado this is an external
        // Flashbots-style relay. On the embedded dev/testing L1 the node
        // serves `eth_sendBundle` itself (see `eez-node::bundle_rpc`), so
        // set this to the same value as EEZ_L1_RPC_URL.
        let builder_relays = resolve_builder_relays(
            env::var(ENV_BUILDER_RPC_URL),
            env::var_os(ENV_BUILDER_RPC_URLS_UNSUPPORTED).is_some(),
        )?;
        Ok(Self {
            reader: L1ReaderConfig::from_env()?,
            builder_relays,
            target_rpc_url,
            poster: parse_key(ENV_POSTER_KEY)?,
            bundle_capture_dir: optional_path(ENV_BUNDLE_CAPTURE_DIR)?,
            bundle_submission_lead: Duration::from_millis(parse_u64_or(
                ENV_BUNDLE_SUBMISSION_LEAD_MS,
                DEFAULT_BUNDLE_SUBMISSION_LEAD_MS,
            )?),
        })
    }
}

/// Upper bound on configured builder relays. Every dispatch opens one bounded
/// HTTP request per relay concurrently, each carrying the full blob bundle, so
/// the fan-out width is a resource limit rather than a protocol rule.
pub const MAX_BUILDER_RELAYS: usize = 16;

/// Validated, ordered `eth_sendBundle` relay endpoints.
///
/// Configured as `EEZ_L1_BUILDER_RPC_URL`: one absolute HTTP(S) URL, or several
/// separated by commas (surrounding whitespace is ignored). A literal comma
/// inside a URL must be percent-encoded as `%2C`. The list is non-empty, has
/// no empty entries or duplicates, and holds at most [`MAX_BUILDER_RELAYS`]
/// endpoints. A single URL keeps the previous single-relay configuration.
///
/// URLs may embed credentials or API keys, so `Debug`, errors and logs never
/// print them: a relay is identified by its position and host only (see
/// [`Self::label`]).
#[derive(Clone, PartialEq, Eq)]
pub struct BuilderRelays(Vec<Url>);

impl BuilderRelays {
    /// Parse the comma-separated relay list.
    ///
    /// # Errors
    ///
    /// Returns [`L1Error::Config`] for an empty list or entry, a malformed or
    /// non-HTTP(S) URL, a duplicate, or more than [`MAX_BUILDER_RELAYS`]
    /// entries. Messages name entries by position and never echo the URL.
    pub fn parse(raw: &str) -> L1Result<Self> {
        if raw.trim().is_empty() {
            return Err(L1Error::Config(format!(
                "{ENV_BUILDER_RPC_URL} must list at least one builder URL"
            )));
        }
        let urls = raw
            .split(',')
            .enumerate()
            .map(|(index, entry)| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return Err(L1Error::Config(format!(
                        "{ENV_BUILDER_RPC_URL} entry {index} is empty; \
                         separate builder URLs with single commas"
                    )));
                }
                Url::parse(entry).map_err(|error| {
                    L1Error::Config(format!("{ENV_BUILDER_RPC_URL} entry {index}: {error}"))
                })
            })
            .collect::<L1Result<Vec<_>>>()?;
        Self::new(urls)
    }

    /// Validate an explicit relay list.
    ///
    /// # Errors
    ///
    /// Same validation as [`Self::parse`] after URL syntax.
    pub fn new(urls: Vec<Url>) -> L1Result<Self> {
        if urls.is_empty() {
            return Err(L1Error::Config(format!(
                "{ENV_BUILDER_RPC_URL} must list at least one builder URL"
            )));
        }
        if urls.len() > MAX_BUILDER_RELAYS {
            return Err(L1Error::Config(format!(
                "{ENV_BUILDER_RPC_URL} lists {} builders; at most {MAX_BUILDER_RELAYS} are supported",
                urls.len()
            )));
        }
        for (index, url) in urls.iter().enumerate() {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(L1Error::Config(format!(
                    "{ENV_BUILDER_RPC_URL} entry {index} must be an absolute HTTP(S) URL"
                )));
            }
            // `Url` equality compares the normalized serialization, so host
            // case and default-port spelling differences still collide.
            if let Some(first) = urls[..index].iter().position(|other| other == url) {
                return Err(L1Error::Config(format!(
                    "{ENV_BUILDER_RPC_URL} entries {first} and {index} name the same builder"
                )));
            }
        }
        Ok(Self(urls))
    }

    /// Endpoints in configured order. They may contain secrets; never log them.
    #[must_use]
    pub fn urls(&self) -> &[Url] {
        &self.0
    }

    /// Number of configured relays (always at least one).
    #[must_use]
    #[allow(clippy::len_without_is_empty)]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Host of the relay at `index`, the only URL component that is logged.
    #[must_use]
    pub fn host(&self, index: usize) -> &str {
        self.0
            .get(index)
            .and_then(Url::host_str)
            .unwrap_or("<unknown>")
    }

    /// Log-safe identifier: `builder[<index>] (<host>)`.
    #[must_use]
    pub fn label(&self, index: usize) -> String {
        format!("builder[{index}] ({})", self.host(index))
    }
}

impl std::fmt::Debug for BuilderRelays {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries((0..self.len()).map(|index| self.label(index)))
            .finish()
    }
}

/// Resolve the relay list from the raw `EEZ_L1_BUILDER_RPC_URL` read and
/// whether the unsupported plural name is present in the environment.
fn resolve_builder_relays(
    single: Result<String, env::VarError>,
    plural_present: bool,
) -> L1Result<BuilderRelays> {
    if plural_present {
        return Err(L1Error::Config(format!(
            "{ENV_BUILDER_RPC_URLS_UNSUPPORTED} is not supported; list every builder, \
             comma-separated, in {ENV_BUILDER_RPC_URL} and unset {ENV_BUILDER_RPC_URLS_UNSUPPORTED}"
        )));
    }
    match single {
        Ok(raw) => BuilderRelays::parse(&raw),
        Err(env::VarError::NotPresent) => Err(L1Error::Config(format!(
            "{ENV_BUILDER_RPC_URL} is required (see .env.example)"
        ))),
        Err(env::VarError::NotUnicode(_)) => Err(L1Error::Config(format!(
            "{ENV_BUILDER_RPC_URL} contains non-UTF-8 bytes"
        ))),
    }
}

fn optional_path(name: &str) -> L1Result<Option<PathBuf>> {
    match env::var(name) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(PathBuf::from(raw))),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(L1Error::Config(format!("{name} contains non-UTF-8 bytes")))
        }
    }
}

fn require(name: &str) -> L1Result<String> {
    env::var(name).map_err(|_| L1Error::Config(format!("{name} is required (see .env.example)")))
}

fn parse_url(name: &str) -> L1Result<Url> {
    Url::parse(&require(name)?).map_err(|e| L1Error::Config(format!("{name}: {e}")))
}

fn parse_address(name: &str) -> L1Result<Address> {
    Address::from_str(&require(name)?).map_err(|e| L1Error::Config(format!("{name}: {e}")))
}

fn parse_key(name: &str) -> L1Result<PrivateKeySigner> {
    let raw = require(name)?;
    PrivateKeySigner::from_str(raw.trim_start_matches("0x"))
        .map_err(|e| L1Error::Config(format!("{name}: {e}")))
}

fn parse_u64(name: &str) -> L1Result<u64> {
    require(name)?
        .parse::<u64>()
        .map_err(|e| L1Error::Config(format!("{name}: {e}")))
}

fn parse_u64_or(name: &str, default: u64) -> L1Result<u64> {
    match env::var(name) {
        Ok(raw) => raw
            .parse::<u64>()
            .map_err(|e| L1Error::Config(format!("{name}: {e}"))),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => {
            Err(L1Error::Config(format!("{name} contains non-UTF-8 bytes")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "fixture-secret-token";

    fn relays(raw: &str) -> L1Result<BuilderRelays> {
        resolve_builder_relays(Ok(raw.to_owned()), false)
    }

    fn config_error(result: L1Result<BuilderRelays>) -> String {
        match result {
            Err(L1Error::Config(message)) => message,
            other => panic!("expected configuration error, got {other:?}"),
        }
    }

    #[test]
    fn single_builder_url_is_unchanged() {
        let parsed = relays("https://builder.chiado.gcd.ovh/").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.urls()[0].as_str(), "https://builder.chiado.gcd.ovh/");
        assert_eq!(parsed.label(0), "builder[0] (builder.chiado.gcd.ovh)");
    }

    #[test]
    fn comma_separated_builders_keep_order_and_trim_whitespace() {
        let parsed =
            relays(" https://a.example/rpc , http://127.0.0.1:8645,https://b.example/?k=v ")
                .unwrap();
        let urls: Vec<_> = parsed.urls().iter().map(Url::as_str).collect();
        assert_eq!(
            urls,
            [
                "https://a.example/rpc",
                "http://127.0.0.1:8645/",
                "https://b.example/?k=v"
            ]
        );
        assert_eq!(parsed.host(1), "127.0.0.1");
    }

    #[test]
    fn absent_empty_and_blank_entries_are_rejected() {
        let missing = config_error(resolve_builder_relays(
            Err(env::VarError::NotPresent),
            false,
        ));
        assert!(
            missing.contains("EEZ_L1_BUILDER_RPC_URL is required"),
            "{missing}"
        );
        for raw in ["", "   "] {
            assert!(config_error(relays(raw)).contains("at least one builder"));
        }
        for (raw, entry) in [
            (",", "entry 0 is empty"),
            (" ,https://a.example", "entry 0 is empty"),
            ("https://a.example,,https://b.example", "entry 1 is empty"),
            ("https://a.example,", "entry 1 is empty"),
        ] {
            let error = config_error(relays(raw));
            assert!(error.contains(entry), "{raw:?}: {error}");
        }
    }

    #[test]
    fn invalid_and_non_http_entries_are_rejected_without_echoing_them() {
        for raw in [
            format!("https://a.example,not a url {SECRET}"),
            format!("https://a.example,ws://{SECRET}.example"),
            format!("https://a.example,unix:/{SECRET}"),
        ] {
            let error = config_error(relays(&raw));
            assert!(error.contains("entry 1"), "{error}");
            assert!(!error.contains(SECRET), "{error}");
        }
    }

    #[test]
    fn duplicate_builders_are_rejected_after_normalization() {
        let error = config_error(relays(&format!(
            "https://user:{SECRET}@A.example:443/rpc,https://user:{SECRET}@a.example/rpc"
        )));
        assert!(error.contains("entries 0 and 1"), "{error}");
        assert!(!error.contains(SECRET), "{error}");
        // Distinct paths on one host are distinct relays.
        assert_eq!(
            relays("https://a.example/one,https://a.example/two")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn builder_count_is_bounded() {
        let list = |count: usize| {
            (0..count)
                .map(|index| format!("https://b{index}.example"))
                .collect::<Vec<_>>()
                .join(",")
        };
        assert!(config_error(relays(&list(MAX_BUILDER_RELAYS + 1))).contains("at most"));
        assert_eq!(
            relays(&list(MAX_BUILDER_RELAYS)).unwrap().len(),
            MAX_BUILDER_RELAYS
        );
    }

    #[test]
    fn unsupported_plural_variable_conflicts_with_any_configuration() {
        for single in [
            Ok("https://a.example".to_owned()),
            Err(env::VarError::NotPresent),
        ] {
            let error = config_error(resolve_builder_relays(single, true));
            assert!(
                error.contains("EEZ_L1_BUILDER_RPC_URLS is not supported"),
                "{error}"
            );
        }
    }

    #[test]
    fn debug_identifies_builders_by_position_and_host_only() {
        let parsed = relays(&format!(
            "https://user:{SECRET}@a.example/{SECRET}?key={SECRET},http://b.example:8645/"
        ))
        .unwrap();
        let debug = format!("{parsed:?}");
        assert_eq!(
            debug,
            r#"["builder[0] (a.example)", "builder[1] (b.example)"]"#
        );
        assert!(!debug.contains(SECRET));
    }
}
