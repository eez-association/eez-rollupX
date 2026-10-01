//! Composer discovery JSON-RPC (`eez_composerInfo`).
//!
//! Only the Composer binary installs this method, so its presence also acts as
//! a cheap capability probe for clients connecting to an EEZ node. Method and
//! field names match upstream `eez-association/eez-rollup0` PR #164 so clients
//! can share one discovery parser.
//!
//! Every advertised value is read once at startup from the same configuration
//! the Composer uses for submission and cross-chain wiring. The response is
//! static for the process lifetime; restart the Composer after changing it.

use std::env;
use std::str::FromStr as _;

use alloy_primitives::Address;
use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use serde::Serialize;

/// Optional deployment bindings advertised when configured. Absent or empty
/// values are omitted from the response.
pub(crate) const ROLLUP_MANAGER_ENV: &str = "EEZ_ROLLUP_MANAGER_ADDRESS";
pub(crate) const L1_BRIDGE_SENDER_ENV: &str = "EEZ_L1_BRIDGE_SENDER";
pub(crate) const L2_BRIDGE_RECEIVER_ENV: &str = "EEZ_L2_BRIDGE_RECEIVER";

/// Runtime configuration advertised by `eez_composerInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposerInfo {
    /// EEZ contracts configured for this Composer.
    pub eez_contracts: ComposerContracts,
    /// Chain IDs served by this Composer.
    pub supported_networks: SupportedNetworks,
    /// Composer implementation version (the workspace package version).
    pub version: &'static str,
}

impl ComposerInfo {
    /// Build the discovery response for the Composer's configured networks.
    pub(crate) const fn new(
        eez_contracts: ComposerContracts,
        supported_networks: SupportedNetworks,
    ) -> Self {
        Self {
            eez_contracts,
            supported_networks,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// EVM chain IDs served by the Composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupportedNetworks {
    /// Chain ID reported by the embedded L1 node.
    pub eez_l1: u64,
    /// Chain ID of the L2 chain specification.
    pub eez_l2: u64,
}

/// Contract addresses advertised by the discovery endpoint.
///
/// The fork deploys the L1 registry, the rollup manager and the `EEZL2`
/// predeploy behind proxies. Clients interact with the stable proxy addresses,
/// which are exactly the configured values; implementation addresses are
/// upgrade records and are never advertised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ComposerContracts {
    /// L1 EEZ registry proxy (`EEZ_REGISTRY_ADDRESS`).
    #[serde(rename = "eezRegistryAddress")]
    pub registry: Address,
    /// L1 rollup manager proxy, when configured (`EEZ_ROLLUP_MANAGER_ADDRESS`).
    #[serde(
        rename = "eezRollupManagerAddress",
        skip_serializing_if = "Option::is_none"
    )]
    pub rollup_manager: Option<Address>,
    /// L1 bridge sender, when a bridge deployment is configured.
    #[serde(rename = "eezL1BridgeSender", skip_serializing_if = "Option::is_none")]
    pub l1_bridge_sender: Option<Address>,
    /// L2 `EEZL2` proxy predeploy (`EEZL2_ADDRESS`).
    #[serde(rename = "eezL2Address")]
    pub l2_predeploy: Address,
    /// L2 bridge receiver, when a bridge deployment is configured.
    #[serde(
        rename = "eezL2BridgeReceiver",
        skip_serializing_if = "Option::is_none"
    )]
    pub l2_bridge_receiver: Option<Address>,
}

/// Read an optional advertised contract address.
///
/// Absent or whitespace-only values mean "not configured" because Compose
/// forwards unset optional variables as empty strings. A present value must be
/// a nonzero address: advertising the zero address would direct clients to a
/// contract that does not exist.
///
/// # Errors
///
/// Returns an error naming the variable for malformed, zero or non-UTF-8 input.
pub(crate) fn optional_address_from_env(name: &str) -> eyre::Result<Option<Address>> {
    match env::var(name) {
        Ok(value) => parse_optional_address(name, &value),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(eyre::eyre!("{name} contains non-UTF-8 bytes")),
    }
}

fn parse_optional_address(name: &str, value: &str) -> eyre::Result<Option<Address>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let address = Address::from_str(value).map_err(|error| eyre::eyre!("{name}: {error}"))?;
    if address.is_zero() {
        return Err(eyre::eyre!(
            "{name} must be a deployed contract address, not the zero address; unset it to omit"
        ));
    }
    Ok(Some(address))
}

/// Build the RPC module serving `eez_composerInfo`.
///
/// # Errors
///
/// Returns an error if the method cannot be registered.
pub(crate) fn module(info: ComposerInfo) -> eyre::Result<RpcModule<ComposerInfo>> {
    let mut module = RpcModule::new(info);
    module.register_method("eez_composerInfo", |params, info, _| {
        // Accept the standard zero-argument forms (absent, `[]`, `{}`) while
        // rejecting actual arguments instead of silently ignoring a client
        // mistake.
        if let Some(raw) = params.as_str() {
            let value: serde_json::Value = params.parse()?;
            let empty = matches!(&value, serde_json::Value::Array(values) if values.is_empty())
                || matches!(&value, serde_json::Value::Object(values) if values.is_empty());
            if !empty {
                return Err(ErrorObjectOwned::owned(
                    jsonrpsee::types::error::INVALID_PARAMS_CODE,
                    "eez_composerInfo takes no parameters",
                    Some(raw),
                ));
            }
        }
        Ok::<_, ErrorObjectOwned>(info.clone())
    })?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(optional: bool) -> ComposerInfo {
        ComposerInfo::new(
            ComposerContracts {
                registry: Address::repeat_byte(0x11),
                rollup_manager: optional.then(|| Address::repeat_byte(0x33)),
                l1_bridge_sender: optional.then(|| Address::repeat_byte(0x44)),
                l2_predeploy: Address::repeat_byte(0x22),
                l2_bridge_receiver: optional.then(|| Address::repeat_byte(0x55)),
            },
            SupportedNetworks {
                eez_l1: 10_200,
                eez_l2: 10_201,
            },
        )
    }

    #[test]
    fn composer_info_uses_the_upstream_camel_case_schema() {
        let value = serde_json::to_value(info(true)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "eezContracts": {
                    "eezRegistryAddress": format!("{:#x}", Address::repeat_byte(0x11)),
                    "eezRollupManagerAddress": format!("{:#x}", Address::repeat_byte(0x33)),
                    "eezL1BridgeSender": format!("{:#x}", Address::repeat_byte(0x44)),
                    "eezL2Address": format!("{:#x}", Address::repeat_byte(0x22)),
                    "eezL2BridgeReceiver": format!("{:#x}", Address::repeat_byte(0x55)),
                },
                "supportedNetworks": {"eezL1": 10_200, "eezL2": 10_201},
                "version": env!("CARGO_PKG_VERSION"),
            })
        );
    }

    #[test]
    fn composer_info_omits_unconfigured_optional_contracts() {
        let value = serde_json::to_value(info(false)).unwrap();
        let contracts = value["eezContracts"].as_object().unwrap();
        assert_eq!(
            contracts.keys().collect::<Vec<_>>(),
            ["eezL2Address", "eezRegistryAddress"]
        );
    }

    #[test]
    fn optional_addresses_accept_absent_or_empty_and_reject_invalid_or_zero() {
        assert_eq!(parse_optional_address("X", "").unwrap(), None);
        assert_eq!(parse_optional_address("X", "  ").unwrap(), None);
        let address = Address::repeat_byte(0x44);
        assert_eq!(
            parse_optional_address("X", &format!(" {address:#x} ")).unwrap(),
            Some(address)
        );
        let error = parse_optional_address("EEZ_L1_BRIDGE_SENDER", "0x1234")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("EEZ_L1_BRIDGE_SENDER: "), "{error}");
        let error = parse_optional_address(
            "EEZ_ROLLUP_MANAGER_ADDRESS",
            &format!("{:#x}", Address::ZERO),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("zero address"), "{error}");
        // A variable that cannot exist in the test process is absent.
        assert_eq!(
            optional_address_from_env("EEZ_COMPOSER_INFO_TEST_UNSET_ADDRESS").unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn composer_info_rpc_returns_config_and_rejects_params() {
        let module = module(info(false)).unwrap();
        for params in ["", r#","params":[]"#, r#","params":{}"#] {
            let request =
                format!(r#"{{"jsonrpc":"2.0","method":"eez_composerInfo","id":1{params}}}"#);
            let (response, _) = module.raw_json_request(&request, 1).await.unwrap();
            let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();
            assert_eq!(
                response["result"],
                serde_json::to_value(info(false)).unwrap(),
                "{request}"
            );
        }
        for params in ["[1]", r#"{"verbose":true}"#] {
            let request = format!(
                r#"{{"jsonrpc":"2.0","method":"eez_composerInfo","params":{params},"id":2}}"#
            );
            let (response, _) = module.raw_json_request(&request, 1).await.unwrap();
            let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();
            assert_eq!(response["error"]["code"], -32602, "{request}");
            assert!(response.get("result").is_none());
        }
    }
}
