//! Composer discovery JSON-RPC (`eez_composerInfo` and virtual `eth_call`).
//!
//! Served only by the Composer's two cross-chain fronts (the L1 and the L2
//! entry points clients submit through), not by the L2 node RPC: clients
//! discover the deployment from the endpoint they will use. Followers have no
//! fronts. Detection does not imply submission readiness or gas-estimator
//! compatibility, and is not authentication of an untrusted provider.
//! Method and field names match upstream `eez-association/eez-rollup0` PR #164
//! so clients can share one discovery parser.
//!
//! Every advertised value is read once at startup from the same configuration
//! the Composer uses for submission and cross-chain wiring. The response is
//! static for the process lifetime; restart the Composer after changing it.

use std::env;
use std::str::FromStr as _;

use alloy_primitives::{Address, B256, Bytes, U256};
use eez_protocol::discovery;
use serde::Serialize;
use serde_json::{Value, json};

/// JSON-RPC method name, shared with upstream clients.
pub(crate) const METHOD: &str = "eez_composerInfo";
const INVALID_PARAMS: i64 = -32602;

/// Match either calldata alias before validating it: a valid selector in one
/// alias must not let a conflicting/malformed second alias escape upstream.
pub(crate) fn is_discovery_call(request: &Value) -> bool {
    if request.get("method").and_then(Value::as_str) != Some("eth_call") {
        return false;
    }
    let Some(tx) = request.get("params").and_then(|params| params.get(0)) else {
        return false;
    };
    if tx
        .get("to")
        .and_then(Value::as_str)
        .and_then(|to| to.parse::<Address>().ok())
        != Some(discovery::ADDRESS)
    {
        return false;
    }
    ["data", "input"].iter().any(|field| {
        tx.get(field).and_then(Value::as_str).is_some_and(|data| {
            data.strip_prefix("0x")
                .and_then(|hex| hex.get(..8))
                .is_some_and(|selector| {
                    alloy_primitives::hex::decode(selector)
                        .is_ok_and(|bytes| bytes == discovery::SELECTOR)
                })
        })
    })
}

fn discovery_nonce(request: &Value) -> Result<B256, &'static str> {
    let params = request["params"]
        .as_array()
        .ok_or("expected positional eth_call params")?;
    if params.len() > 2 {
        return Err("Composer discovery does not support state or block overrides");
    }
    if params
        .get(1)
        .is_some_and(|block| block.as_str() != Some("latest"))
    {
        return Err("Composer discovery supports only the latest block tag");
    }
    let tx = params
        .first()
        .and_then(Value::as_object)
        .ok_or("expected call object")?;
    let decode = |value: &Value| -> Result<Bytes, &'static str> {
        let raw = value
            .as_str()
            .ok_or("discovery calldata must be a hex string")?;
        if !raw.starts_with("0x") || raw.len() != 74 {
            return Err("discovery calldata must contain the selector and exactly 32 nonce bytes");
        }
        raw.parse::<Bytes>()
            .map_err(|_| "invalid discovery calldata hex")
    };
    let data = tx.get("data").map(decode).transpose()?;
    let input = tx.get("input").map(decode).transpose()?;
    if let (Some(data), Some(input)) = (&data, &input)
        && data != input
    {
        return Err("conflicting data and input calldata");
    }
    let calldata = data.or(input).ok_or("missing discovery calldata")?;
    if calldata[..4] != discovery::SELECTOR {
        return Err("invalid discovery selector");
    }
    if let Some(from) = tx.get("from") {
        serde_json::from_value::<Address>(from.clone()).map_err(|_| "invalid from address")?;
    }
    for field in [
        "value",
        "gas",
        "gasPrice",
        "maxFeePerGas",
        "maxPriorityFeePerGas",
    ] {
        if let Some(raw) = tx.get(field) {
            let value = serde_json::from_value::<U256>(raw.clone())
                .map_err(|_| "invalid call value or gas quantity")?;
            if field == "value" && !value.is_zero() {
                return Err("Composer discovery does not accept nonzero value");
            }
        }
    }
    Ok(B256::from_slice(&calldata[4..]))
}

/// Answer an already matched virtual call without touching the upstream or
/// submission readiness. Serialize the very same JSON value as the custom RPC.
pub(crate) fn respond_eth_call(request: &Value, info: &Value, source_chain_id: u64) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    match discovery_nonce(request) {
        Ok(nonce) => json!({
            "jsonrpc": "2.0", "id": id,
            "result": Bytes::from(discovery::encode_response(nonce, source_chain_id, info.to_string().as_bytes())),
        }),
        Err(message) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": INVALID_PARAMS, "message": message },
        }),
    }
}

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

/// Answer one `eez_composerInfo` request or batch member. Ingress owns
/// notification suppression.
///
/// `info` is the serialized [`ComposerInfo`]. The standard zero-argument forms
/// (absent, `null`, `[]`, `{}`) are accepted; actual arguments are rejected with
/// `-32602` instead of being silently ignored.
pub(crate) fn respond(request: &Value, info: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let empty = match request.get("params") {
        None | Some(Value::Null) => true,
        Some(Value::Array(values)) => values.is_empty(),
        Some(Value::Object(values)) => values.is_empty(),
        Some(_) => false,
    };
    if empty {
        json!({"jsonrpc": "2.0", "id": id, "result": info})
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": INVALID_PARAMS,
                "message": "eez_composerInfo takes no parameters",
                "data": request.get("params"),
            },
        })
    }
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

    #[test]
    fn respond_returns_config_and_rejects_params() {
        let info = serde_json::to_value(info(false)).unwrap();
        for request in [
            json!({"jsonrpc": "2.0", "method": METHOD, "id": 1}),
            json!({"jsonrpc": "2.0", "method": METHOD, "params": null, "id": 1}),
            json!({"jsonrpc": "2.0", "method": METHOD, "params": [], "id": 1}),
            json!({"jsonrpc": "2.0", "method": METHOD, "params": {}, "id": 1}),
        ] {
            assert_eq!(
                respond(&request, &info),
                json!({"jsonrpc": "2.0", "id": 1, "result": info}),
                "{request}"
            );
        }
        for params in [json!([1]), json!({"verbose": true}), json!("x")] {
            let request = json!({"jsonrpc": "2.0", "method": METHOD, "params": params, "id": "a"});
            let response = respond(&request, &info);
            assert_eq!(response["id"], "a");
            assert_eq!(response["error"]["code"], INVALID_PARAMS, "{request}");
            assert_eq!(response["error"]["data"], params);
            assert!(response.get("result").is_none());
        }
    }

    fn probe() -> Value {
        json!({"jsonrpc":"2.0", "id":"wallet", "method":"eth_call", "params":[{
            "to": discovery::ADDRESS, "data": format!("0x98da5085{}", "a5".repeat(32))
        }, "latest"]})
    }

    #[test]
    fn discovery_aliases_wallet_fields_and_response_match_custom_rpc() {
        let info = serde_json::to_value(info(true)).unwrap();
        for chain_id in [1, 10_201] {
            for alias in ["data", "input", "both"] {
                let mut request = probe();
                if alias != "data" {
                    request["params"][0]["input"] = request["params"][0]["data"].clone();
                }
                if alias == "input" {
                    request["params"][0].as_object_mut().unwrap().remove("data");
                }
                request["params"][0]["from"] = json!(Address::repeat_byte(0x99));
                for field in [
                    "value",
                    "gas",
                    "gasPrice",
                    "maxFeePerGas",
                    "maxPriorityFeePerGas",
                ] {
                    request["params"][0][field] = json!("0x0");
                }
                assert!(is_discovery_call(&request));
                let response = respond_eth_call(&request, &info, chain_id);
                assert_eq!(response["id"], "wallet");
                let bytes: Bytes = serde_json::from_value(response["result"].clone()).unwrap();
                assert_eq!(&bytes[..32], discovery::MARKER.as_slice());
                assert_eq!(U256::from_be_slice(&bytes[32..64]), U256::from(1));
                assert_eq!(&bytes[64..96], &[0xa5; 32]);
                assert_eq!(U256::from_be_slice(&bytes[96..128]), U256::from(chain_id));
                assert_eq!(U256::from_be_slice(&bytes[128..160]), U256::from(160));
                let len = U256::from_be_slice(&bytes[160..192]).to::<usize>();
                let decoded: Value = serde_json::from_slice(&bytes[192..192 + len]).unwrap();
                assert_eq!(decoded, respond(&json!({"id":1}), &info)["result"]);
            }
        }
        let mut omitted_block = probe();
        omitted_block["params"].as_array_mut().unwrap().pop();
        assert_eq!(
            discovery_nonce(&omitted_block).unwrap(),
            B256::repeat_byte(0xa5)
        );
        let mut case = probe();
        case["params"][0]["input"] = json!(format!("0x98DA5085{}", "A5".repeat(32)));
        assert!(discovery_nonce(&case).is_ok());
    }

    #[test]
    fn discovery_rejects_malformed_or_conflicting_calldata() {
        for data in [
            json!("0x98da5085"),
            json!(format!("0x98da5085{}", "a5".repeat(31))),
            json!(format!("0x98da5085{}0", "a5".repeat(32))),
            json!(format!("0x98da5085{}", "zz".repeat(32))),
            json!(format!("0x98da5085{}", "a5".repeat(33))),
            json!(format!("0x98da5085{}", "b6".repeat(32))),
            json!(false),
            json!(null),
            json!([0]),
        ] {
            for alias in ["data", "input"] {
                let mut request = probe();
                request["params"][0]["input"] = request["params"][0]["data"].clone();
                request["params"][0][alias] = data.clone();
                assert!(is_discovery_call(&request), "{request}");
                let response = respond_eth_call(&request, &json!({}), 1);
                assert_eq!(response["error"]["code"], -32602, "{request}");
                assert_eq!(response["id"], "wallet");
            }
        }
    }

    #[test]
    fn discovery_rejects_history_overrides_value_and_invalid_wallet_fields() {
        let mut invalid = Vec::new();
        for tag in [
            json!("pending"),
            json!("earliest"),
            json!("safe"),
            json!("finalized"),
            json!("0x1"),
            json!(null),
            json!({"blockHash": B256::ZERO}),
            json!({"blockNumber":"latest"}),
        ] {
            let mut request = probe();
            request["params"][1] = tag;
            invalid.push(request);
        }
        for overrides in [json!(null), json!({}), json!({"ignored":"override"})] {
            let mut request = probe();
            request["params"].as_array_mut().unwrap().push(overrides);
            invalid.push(request);
        }
        for (field, value) in [
            ("value", "0x1"),
            ("value", "garbage"),
            ("from", "0x01"),
            ("gas", "bad"),
            ("gasPrice", "-1"),
            ("maxFeePerGas", "bad"),
            ("maxPriorityFeePerGas", "bad"),
        ] {
            let mut request = probe();
            request["params"][0][field] = json!(value);
            invalid.push(request);
        }
        for request in invalid {
            assert!(is_discovery_call(&request));
            assert_eq!(
                respond_eth_call(&request, &json!({}), 1)["error"]["code"],
                -32602,
                "{request}"
            );
        }
    }

    #[test]
    fn discovery_does_not_match_other_methods_addresses_or_selectors() {
        let mut request = probe();
        request["method"] = json!("eth_estimateGas");
        assert!(!is_discovery_call(&request));
        request = probe();
        request["params"][0]["to"] = json!(Address::ZERO);
        assert!(!is_discovery_call(&request));
        request = probe();
        request["params"][0]["data"] = json!("0x12345678");
        assert!(!is_discovery_call(&request));
        request["params"][0]["data"] = json!("0x");
        assert!(!is_discovery_call(&request));
    }

    #[test]
    fn discovery_address_is_not_a_pinned_evm_precompile() {
        // The pinned Ethereum and Gnosis EVMs use this REVM registry.
        assert!(!reth_evm::revm::precompile::Precompiles::latest().contains(&discovery::ADDRESS));
    }
}
