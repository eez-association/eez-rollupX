use super::*;

#[test]
fn native_gnosis_execution_configuration_fails_before_upstream_constructor_panics() {
    let mut genesis = Genesis::default();
    assert!(validate_gnosis_execution_config(&genesis).is_err());
    genesis.config.deposit_contract_address = Some(alloy_primitives::Address::ZERO);
    for name in ["eip1559collector", "blockRewardsContract"] {
        genesis.config.extra_fields.insert(
            name.into(),
            serde_json::json!(alloy_primitives::Address::ZERO),
        );
    }
    validate_gnosis_execution_config(&genesis).unwrap();
    genesis.config.extra_fields.insert(
        "blockRewardsContract".into(),
        serde_json::json!("bad-address"),
    );
    assert!(validate_gnosis_execution_config(&genesis).is_err());
}

#[cfg(feature = "chain-spec")]
#[test]
fn private_gnosis_profile_preserves_native_rules_without_accepting_unknown_extensions() {
    use reth_chainspec::EthChainSpec as _;
    let mut genesis = Genesis::default();
    genesis.config.chain_id = 7331;
    genesis.config.deposit_contract_address = Some(alloy_primitives::Address::ZERO);
    genesis.config.cancun_time = Some(0);
    genesis.config.prague_time = Some(0);
    for name in ["eip1559collector", "blockRewardsContract"] {
        genesis.config.extra_fields.insert(
            name.into(),
            serde_json::json!(alloy_primitives::Address::ZERO),
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("genesis.json");
    fs::write(&file, serde_json::to_vec(&genesis).unwrap()).unwrap();
    let profile = format!("gnosis:{}", file.display());
    let spec = load_l1_chain_spec(Path::new(&profile)).unwrap();
    let params = spec.blob_params_at_timestamp(1).unwrap();
    assert_eq!(params.max_blobs_per_tx, 2);
    assert_eq!(params.min_blob_fee, 1_000_000_000);
    assert_eq!(spec.chain().id(), 7331);
    assert!(
        load_l1_chain_spec(&file).is_err(),
        "native extension must never silently load as Ethereum"
    );
    genesis
        .config
        .extra_fields
        .insert("typo".into(), serde_json::json!(1));
    fs::write(&file, serde_json::to_vec(&genesis).unwrap()).unwrap();
    assert!(load_l1_chain_spec(Path::new(&profile)).is_err());
}

#[test]
fn kurtosis_mixhash_alias_is_preserved_and_conflicts_fail() {
    let expected = alloy_primitives::B256::with_last_byte(42);
    let mut document = serde_json::to_value(Genesis::default()).unwrap();
    document.as_object_mut().unwrap().remove("mixHash");
    document["mixhash"] = serde_json::json!(expected);
    assert_eq!(
        parse_chain_document(&serde_json::to_vec(&document).unwrap())
            .unwrap()
            .0
            .mix_hash,
        expected
    );
    document["mixHash"] = serde_json::json!(expected);
    assert_eq!(
        parse_chain_document(&serde_json::to_vec(&document).unwrap())
            .unwrap()
            .0
            .mix_hash,
        expected
    );
    document["mixHash"] = serde_json::json!(alloy_primitives::B256::ZERO);
    assert!(parse_chain_document(&serde_json::to_vec(&document).unwrap()).is_err());
}

#[cfg(feature = "chain-spec")]
#[test]
fn l1_presets_keep_gnosis_rules_distinct_from_ethereum_at_each_fork() {
    use reth_chainspec::{EthChainSpec as _, EthereumHardfork};
    let chiado = load_l1_chain_spec(Path::new("chiado")).unwrap();
    let ethereum = load_l1_chain_spec(Path::new("mainnet")).unwrap();
    assert_eq!(chiado.chain().id(), 10200);
    assert_eq!(ethereum.chain().id(), 1);
    let config = &chiado.genesis.config;
    assert!(
        chiado
            .blob_params_at_timestamp(config.cancun_time.unwrap() - 1)
            .is_none()
    );
    for timestamp in [
        config.cancun_time.unwrap(),
        config.prague_time.unwrap(),
        config.osaka_time.unwrap(),
    ] {
        let params = chiado.blob_params_at_timestamp(timestamp).unwrap();
        assert_eq!(params.max_blob_count, 2);
        assert_eq!(params.max_blobs_per_tx, 2);
        assert_eq!(params.target_blob_count, 1);
        assert_eq!(params.min_blob_fee, 1_000_000_000);
    }
    let ethereum_prague = ethereum
        .blob_params_at_timestamp(
            ethereum
                .hardforks
                .fork(EthereumHardfork::Prague)
                .as_timestamp()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(ethereum_prague.max_blobs_per_tx, 9);
    assert_eq!(ethereum_prague.min_blob_fee, 1);
    assert_eq!(
        ethereum
            .blob_params_at_timestamp(
                ethereum
                    .hardforks
                    .fork(EthereumHardfork::Osaka)
                    .as_timestamp()
                    .unwrap()
            )
            .unwrap()
            .max_blobs_per_tx,
        6
    );
}

#[test]
fn accepts_full_genesis_and_bare_chain_config_documents() {
    let full_genesis = br#"{
        "config": {
            "chainId": 1,
            "homesteadBlock": 0,
            "londonBlock": 0,
            "shanghaiTime": 0,
            "cancunTime": 0,
            "pragueTime": 0,
            "osakaTime": 0
        },
        "nonce": "0x0",
        "timestamp": "0x6490fdd2",
        "extraData": "0x",
        "gasLimit": "0x1c9c380",
        "difficulty": "0x0",
        "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "coinbase": "0x0000000000000000000000000000000000000000",
        "alloc": {
            "0x0000000000000000000000000000000000000001": {
                "balance": "0x1",
                "nonce": "0x1",
                "code": "0x00",
                "storage": {"0x00": "0x01"}
            }
        },
        "number": "0x0",
        "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "baseFeePerGas": "0x7",
        "blobGasUsed": "0x0",
        "excessBlobGas": "0x0"
    }"#;
    let (genesis, kind) = parse_chain_document(full_genesis).unwrap();
    assert_eq!(kind, ChainDocumentKind::Genesis);
    assert_ne!(genesis.timestamp, 0);
    assert_eq!(genesis.config.chain_id, 1);
    assert_eq!(genesis.config.homestead_block, Some(0));
    assert_eq!(genesis.config.london_block, Some(0));
    assert_eq!(genesis.config.shanghai_time, Some(0));
    assert_eq!(genesis.config.cancun_time, Some(0));
    assert_eq!(genesis.config.prague_time, Some(0));
    assert_eq!(genesis.config.osaka_time, Some(0));
    assert_eq!(genesis.alloc.len(), 1);

    let expected_config = genesis.config.clone();
    let encoded = serde_json::to_vec(&expected_config).unwrap();
    let (bare, kind) = parse_chain_document(&encoded).unwrap();
    assert_eq!(kind, ChainDocumentKind::BareChainConfig);
    assert_eq!(bare.config, expected_config);
    assert_eq!(bare.timestamp, 0);

    let nested = br#"{
        "chainId": 1,
        "clique": {"period": 5, "epoch": 30000},
        "blobSchedule": {
            "cancun": {"target": 3, "max": 6, "baseFeeUpdateFraction": 3338477}
        }
    }"#;
    let (nested, kind) = parse_chain_document(nested).unwrap();
    assert_eq!(kind, ChainDocumentKind::BareChainConfig);
    assert_eq!(nested.config.clique.unwrap().period, Some(5));
    assert_eq!(nested.config.blob_schedule.len(), 1);
}

#[test]
fn rejects_ambiguous_or_default_only_chain_documents() {
    let invalid: &[&[u8]] = &[
        b"{}",
        b"[]",
        br#"{"config": {}}"#,
        br#"{"timestamp": "0x0", "alloc": {}}"#,
        br#"{"confg": {"chainId": 1}, "timestamp": "0x0"}"#,
    ];
    for encoded in invalid {
        assert!(parse_chain_document(encoded).is_err());
    }
}

#[test]
fn rejects_unknown_chain_document_fields_and_reports_them_deterministically() {
    let cases: &[(&[u8], &str)] = &[
        (
            br#"{"chainId": 1, "shangaiTime": 0}"#,
            "bare ChainConfig contains unsupported fields: `shangaiTime`",
        ),
        (
            br#"{"config": {"chainId": 1, "cancuunTime": 0}}"#,
            "Genesis `config` contains unsupported fields: `cancuunTime`",
        ),
        (
            br#"{"config": {"chainId": 1}, "mysteryGenesisField": true}"#,
            "Genesis contains unsupported fields: `mysteryGenesisField`",
        ),
        (
            br#"{"chainId": 1, "zUnknown": 0, "aUnknown": 0}"#,
            "bare ChainConfig contains unsupported fields: `aUnknown`, `zUnknown`",
        ),
        (
            br#"{"chainId": 1, "clique": {"perod": 5}}"#,
            "bare ChainConfig.clique contains unsupported fields: `perod`",
        ),
        (
            br#"{"config": {"chainId": 1, "parlia": {"epoc": 10}}}"#,
            "Genesis `config`.parlia contains unsupported fields: `epoc`",
        ),
        (
            br#"{"chainId": 1, "ethash": {"unexpected": true}}"#,
            "bare ChainConfig.ethash contains unsupported fields: `unexpected`",
        ),
        (
            br#"{"chainId": 1, "blobSchedule": {"cancun": {"targt": 3}}}"#,
            "bare ChainConfig.blobSchedule.cancun contains unsupported fields: `targt`",
        ),
        (
            br#"{"chainId": 1, "blobSchedule": {"prauge": {"target": 6}}}"#,
            "bare ChainConfig.blobSchedule contains unsupported fields: `prauge`",
        ),
    ];

    for (encoded, expected) in cases {
        let error = parse_chain_document(encoded).unwrap_err();
        assert_eq!(error.to_string(), *expected);
    }
}
