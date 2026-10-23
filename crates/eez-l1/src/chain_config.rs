//! Strict loading of the operator-configured execution-chain trust input.
//!
//! Deployments may provide either a complete Genesis document or a bare
//! `ChainConfig`. Both forms require an explicit chain ID and reject unknown
//! fields instead of silently accepting misspelled or unsupported settings.

use std::fs;
use std::path::Path;

use alloy_genesis::{ChainConfig, Genesis};
use alloy_primitives::{B256, keccak256};
use eyre::WrapErr as _;

mod canonical;
pub use canonical::{CANONICAL_JSON_VERSION, canonicalize_chain_json};
mod l2;
pub use l2::{LoadedL2Genesis, load_l2_genesis, parse_l2_genesis};
#[cfg(feature = "chain-spec")]
mod offline;
#[cfg(feature = "chain-spec")]
pub use offline::{OfflineL1Rules, offline_l1_rules};

const GENESIS_FIELDS: &[&str] = &[
    "alloc",
    "baseFeePerGas",
    "blobGasUsed",
    "coinbase",
    "config",
    "difficulty",
    "excessBlobGas",
    "extraData",
    "gasLimit",
    "mixHash",
    "nonce",
    "number",
    "parentHash",
    "timestamp",
];

/// Resolve the existing network presets or a strict custom execution profile.
/// Chiado must use GnosisChainSpec: deserializing its fork dates as Ethereum
/// would lose Gnosis's blob maxima and minimum blob fee.
#[cfg(feature = "chain-spec")]
pub fn load_l1_chain_spec(path: &Path) -> eyre::Result<std::sync::Arc<reth_chainspec::ChainSpec>> {
    match path.to_str() {
        Some(profile) if profile.starts_with("gnosis:") => {
            let genesis = load_private_gnosis_genesis(Path::new(&profile["gnosis:".len()..]))?;
            eyre::ensure!(
                !matches!(genesis.config.chain_id, 100 | 10200),
                "private Gnosis profiles require a distinct chain ID; use the canonical preset for public networks"
            );
            Ok(std::sync::Arc::new(
                reth_gnosis::spec::gnosis_spec::GnosisChainSpec::from(genesis).inner,
            ))
        }
        Some("mainnet") => Ok(reth_chainspec::MAINNET.clone()),
        // Public Ethereum test networks share mainnet's execution rules; they
        // mirror the embedded node's EEZ_L1_CHAIN kinds for rehearsals.
        Some("sepolia") => Ok(reth_chainspec::SEPOLIA.clone()),
        Some("hoodi") => Ok(reth_chainspec::HOODI.clone()),
        Some("chiado") => {
            use reth_gnosis::spec::{chains::CHIADO_GENESIS, gnosis_spec::GnosisChainSpec};
            Ok(std::sync::Arc::new(
                GnosisChainSpec::from(CHIADO_GENESIS.clone()).inner,
            ))
        }
        _ => {
            let (genesis, _) = load_chain_document(path)?;
            eyre::ensure!(
                !matches!(genesis.config.chain_id, 100 | 10200),
                "Gnosis/Chiado requires its native profile; use EEZ_L1_CHAIN_CONFIG=chiado for Chiado"
            );
            Ok(std::sync::Arc::new(reth_chainspec::ChainSpec::from(
                genesis,
            )))
        }
    }
}

/// Native execution requires these addresses even on a private PoS fixture.
/// Validate before GnosisEvmConfig, whose upstream constructor assumes them.
pub fn validate_gnosis_execution_config(genesis: &Genesis) -> eyre::Result<()> {
    eyre::ensure!(
        genesis.config.deposit_contract_address.is_some(),
        "native Gnosis profile requires depositContractAddress"
    );
    for name in ["eip1559collector", "blockRewardsContract"] {
        let value = genesis
            .config
            .extra_fields
            .get(name)
            .ok_or_else(|| eyre::eyre!("native Gnosis profile requires {name}"))?;
        serde_json::from_value::<alloy_primitives::Address>(value.clone())
            .wrap_err_with(|| format!("native Gnosis {name} must be an address"))?;
    }
    Ok(())
}

#[cfg(feature = "chain-spec")]
fn load_private_gnosis_genesis(path: &Path) -> eyre::Result<Genesis> {
    let canonical = canonicalize_chain_json(&fs::read(path)?)?;
    let mut document: serde_json::Value = serde_json::from_slice(&canonical)?;
    let config = document
        .get_mut("config")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| eyre::eyre!("private Gnosis profile requires a full Genesis document"))?;
    let mut native_fields = Vec::new();
    for name in ["eip1559collector", "blockRewardsContract"] {
        if let Some(value) = config.remove(name) {
            native_fields.push((name.to_owned(), value));
        }
    }
    // Every other field retains the common strict validation, including fork
    // dates, blob parameters, unknown keys and the genesis spelling alias.
    let (mut genesis, _) = parse_chain_document(&serde_json::to_vec(&document)?)?;
    genesis.config.extra_fields.extend(native_fields);
    validate_gnosis_execution_config(&genesis)?;
    Ok(genesis)
}
const CONSENSUS_CONFIG_FIELDS: &[&str] = &["epoch", "period"];
const BLOB_PARAMETER_FIELDS: &[&str] = &["baseFeeUpdateFraction", "max", "target"];
// Exact keys consumed by Alloy 2.1's `blob_schedule_blob_params`. Amsterdam is
// intentionally capitalized in that pinned implementation.
const BLOB_SCHEDULE_FORKS: &[&str] = &[
    "Amsterdam",
    "bpo1",
    "bpo2",
    "bpo3",
    "bpo4",
    "bpo5",
    "cancun",
    "osaka",
    "prague",
];

/// JSON shape used to obtain the chain configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainDocumentKind {
    BareChainConfig,
    Genesis,
}

impl ChainDocumentKind {
    /// Stable source label used by startup tracing.
    pub const fn label(self) -> &'static str {
        match self {
            Self::BareChainConfig => "chain_config",
            Self::Genesis => "genesis",
        }
    }
}

/// Read and strictly parse one operator-configured chain document.
pub fn load_chain_document(path: &Path) -> eyre::Result<(Genesis, ChainDocumentKind)> {
    let loaded = load_chain_document_with_digest(path)?;
    Ok((loaded.genesis, loaded.kind))
}

/// Strictly parsed rules and the digest of the same loaded source document.
/// The digest precedes spelling normalization and deserialization defaults;
/// reserializing Genesis is not a substitute for this source identity.
#[derive(Debug)]
pub struct LoadedChainDocument {
    genesis: Genesis,
    kind: ChainDocumentKind,
    digest: B256,
}

impl LoadedChainDocument {
    pub const fn digest(&self) -> B256 {
        self.digest
    }

    pub const fn kind(&self) -> ChainDocumentKind {
        self.kind
    }

    pub fn into_genesis(self) -> Genesis {
        self.genesis
    }
}

/// Read once so the configuration digest and execution rules cannot observe
/// different versions of a file replaced during startup.
pub fn load_chain_document_with_digest(path: &Path) -> eyre::Result<LoadedChainDocument> {
    let encoded =
        fs::read(path).wrap_err_with(|| format!("read chain configuration {}", path.display()))?;
    parse_chain_document_with_digest(&encoded)
        .wrap_err_with(|| format!("parse chain configuration {}", path.display()))
}

/// Parse either a complete Genesis document or a bare `ChainConfig`.
#[cfg(any(test, feature = "chain-spec"))]
fn parse_chain_document(encoded: &[u8]) -> eyre::Result<(Genesis, ChainDocumentKind)> {
    let loaded = parse_chain_document_with_digest(encoded)?;
    Ok((loaded.genesis, loaded.kind))
}

/// Parse the execution rules and their exact canonical source identity together.
pub fn parse_chain_document_with_digest(encoded: &[u8]) -> eyre::Result<LoadedChainDocument> {
    let canonical = canonicalize_chain_json(encoded)?;
    let (genesis, kind) = parse_document_value(serde_json::from_slice(&canonical)?)?;
    Ok(LoadedChainDocument {
        genesis,
        kind,
        digest: keccak256(canonical),
    })
}

fn parse_document_value(
    mut document: serde_json::Value,
) -> eyre::Result<(Genesis, ChainDocumentKind)> {
    // The pinned ethereum-genesis-generator emits the geth spelling
    // `mixhash`; Alloy's Genesis field is `mixHash`. Normalize explicitly so
    // the trust input is retained instead of silently discarded by serde.
    if let Some(object) = document.as_object_mut()
        && object.contains_key("config")
        && let Some(alias) = object.remove("mixhash")
    {
        if let Some(canonical) = object.get("mixHash") {
            eyre::ensure!(canonical == &alias, "Genesis mixhash and mixHash disagree");
        } else {
            object.insert("mixHash".into(), alias);
        }
    }
    let object = document
        .as_object()
        .ok_or_else(|| eyre::eyre!("chain configuration must be a JSON object"))?;
    if let Some(config_value) = object.get("config") {
        ensure_only_known_genesis_fields(object)?;
        let config_object = config_value
            .as_object()
            .ok_or_else(|| eyre::eyre!("Genesis `config` must be a JSON object"))?;
        eyre::ensure!(
            config_object.contains_key("chainId"),
            "Genesis `config` must contain `chainId`"
        );
        ensure_known_nested_chain_config_fields(config_object, "Genesis `config`")?;
        let genesis: Genesis = serde_json::from_value(document)?;
        ensure_no_chain_config_extensions(&genesis.config, "Genesis `config`")?;
        Ok((genesis, ChainDocumentKind::Genesis))
    } else {
        if let Some(field) = GENESIS_FIELDS
            .iter()
            .copied()
            .find(|field| object.contains_key(*field))
        {
            eyre::bail!("Genesis-like document contains `{field}` but no top-level `config`");
        }
        eyre::ensure!(
            object.contains_key("chainId"),
            "bare ChainConfig must contain top-level `chainId`"
        );
        ensure_known_nested_chain_config_fields(object, "bare ChainConfig")?;
        let config: ChainConfig = serde_json::from_value(document)?;
        ensure_no_chain_config_extensions(&config, "bare ChainConfig")?;
        Ok((
            Genesis {
                config,
                ..Default::default()
            },
            ChainDocumentKind::BareChainConfig,
        ))
    }
}

/// Reject unknown top-level Genesis fields before deserialization drops them.
fn ensure_only_known_genesis_fields(
    object: &serde_json::Map<String, serde_json::Value>,
) -> eyre::Result<()> {
    let unknown = object
        .keys()
        .filter(|field| !GENESIS_FIELDS.contains(&field.as_str()))
        .map(String::as_str);
    ensure_no_unsupported_fields("Genesis", unknown)
}

/// Reject fields captured by `ChainConfig`'s extension map.
fn ensure_no_chain_config_extensions(config: &ChainConfig, context: &str) -> eyre::Result<()> {
    ensure_no_unsupported_fields(context, config.extra_fields.keys().map(String::as_str))
}

/// Reject unsupported fields inside extension-friendly nested objects.
fn ensure_known_nested_chain_config_fields(
    config: &serde_json::Map<String, serde_json::Value>,
    context: &str,
) -> eyre::Result<()> {
    for field in ["clique", "parlia"] {
        ensure_nested_object_fields(config, field, CONSENSUS_CONFIG_FIELDS, context)?;
    }
    ensure_nested_object_fields(config, "ethash", &[], context)?;

    if let Some(serde_json::Value::Object(schedule)) = config.get("blobSchedule") {
        ensure_no_unsupported_fields(
            &format!("{context}.blobSchedule"),
            schedule
                .keys()
                .filter(|fork| !BLOB_SCHEDULE_FORKS.contains(&fork.as_str()))
                .map(String::as_str),
        )?;
        for (fork, params) in schedule {
            if let serde_json::Value::Object(params) = params {
                let nested_context = format!("{context}.blobSchedule.{fork}");
                ensure_no_unsupported_fields(
                    &nested_context,
                    params
                        .keys()
                        .filter(|field| !BLOB_PARAMETER_FIELDS.contains(&field.as_str()))
                        .map(String::as_str),
                )?;
            }
        }
    }
    Ok(())
}

/// Validate the fields of one nested object when that field is present.
fn ensure_nested_object_fields(
    config: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    supported: &[&str],
    context: &str,
) -> eyre::Result<()> {
    let Some(serde_json::Value::Object(object)) = config.get(field) else {
        return Ok(());
    };
    let nested_context = format!("{context}.{field}");
    ensure_no_unsupported_fields(
        &nested_context,
        object
            .keys()
            .filter(|field| !supported.contains(&field.as_str()))
            .map(String::as_str),
    )
}

/// Report unsupported fields in deterministic lexical order.
fn ensure_no_unsupported_fields<'a>(
    context: &str,
    fields: impl IntoIterator<Item = &'a str>,
) -> eyre::Result<()> {
    let mut fields = fields.into_iter().collect::<Vec<_>>();
    if fields.is_empty() {
        return Ok(());
    }
    fields.sort_unstable();
    eyre::bail!(
        "{context} contains unsupported fields: `{}`",
        fields.join("`, `"),
    )
}

#[cfg(test)]
mod tests;
