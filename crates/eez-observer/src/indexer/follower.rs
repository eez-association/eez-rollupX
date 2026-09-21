//! Optional exact-settlement comparison. Unproven local history is unknown,
//! including a healthy-looking Composer accidentally configured as a follower.
use super::{
    Config, Error, Rpc, SharedStatus,
    ledger::{self, Deadline},
};
use crate::{
    config::Endpoint,
    database::{Database, DatabaseConfig},
    rpc::Failure,
};
use alloy_consensus::Header;
use alloy_primitives::{Address, B256};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fmt, fmt::Write as _, time::Duration};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowerConfig {
    pub url: String,
    pub eez: Address,
}
impl fmt::Debug for FollowerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FollowerConfig")
            .field("eez", &self.eez)
            .finish_non_exhaustive()
    }
}
impl FollowerConfig {
    pub(super) fn validate(&self, config: &Config) -> Result<(), &'static str> {
        crate::config::validate_url(&self.url)?;
        if self.eez == Address::ZERO
            || !config
                .l1
                .contracts
                .iter()
                .any(|value| value.parse::<Address>().ok() == Some(self.eez))
        {
            return Err("follower EEZ contract must be a configured nonzero L1 posting contract");
        }
        Ok(())
    }
    fn endpoint(&self, config: &Config) -> Endpoint {
        Endpoint {
            url: self.url.clone(),
            ..config.exporter.l2.clone()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    at: Option<u64>,
    agreement: Option<bool>,
    reason: Option<&'static str>,
    lag: Option<u64>,
}
pub fn render(status: Option<&Status>, now: u64) -> String {
    let Some(status) = status else {
        return "eez_observer_follower_comparison_known 0\neez_observer_follower_unknown{reason=\"disabled\"} 1\n".into();
    };
    let fresh = status
        .at
        .is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000);
    let agreement = status.agreement.filter(|_| fresh);
    let mut result = format!(
        "eez_observer_follower_comparison_known {}\n",
        u8::from(agreement.is_some())
    );
    if let Some(value) = agreement {
        writeln!(
            result,
            "eez_observer_follower_state_root_agreement {}",
            u8::from(value)
        )
        .unwrap();
    } else {
        writeln!(
            result,
            "eez_observer_follower_unknown{{reason=\"{}\"}} 1",
            if fresh {
                status.reason.unwrap_or("unproven")
            } else {
                "stale"
            }
        )
        .unwrap();
    }
    if let Some(at) = status.at {
        writeln!(
            result,
            "eez_observer_follower_last_attempt_timestamp_seconds {}",
            at / 1000
        )
        .unwrap();
    }
    if fresh && let Some(lag) = status.lag {
        writeln!(result, "eez_observer_follower_settled_lag_blocks {lag}").unwrap();
    }
    result
}

pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.follower.is_none() {
        return;
    }
    let mut failures = 0u32;
    loop {
        failures = if poll(config, rpc, status).await {
            0
        } else {
            failures.saturating_add(1)
        };
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}
pub(super) async fn poll(config: &Config, rpc: &Rpc, status: &SharedStatus) -> bool {
    if config.follower.is_none() {
        return true;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), cycle(config, rpc))
        .await
        .unwrap_or(Err("timeout"));
    let successful = result.is_ok();
    let mut next = match result {
        Ok(state) => state,
        Err(reason) => Status {
            reason: Some(reason),
            ..Default::default()
        },
    };
    next.at = Some(crate::now_ms());
    status.write().await.follower = Some(next);
    successful
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    reference: Value,
    hash: B256,
}
const TARGET_SQL: &str = "SELECT b.l1_block_number::text,b.settlement_block_hash::bytea,b.settlement_transaction_hash::bytea,s.transaction_index::text,b.number::text,b.hash::bytea,COALESCE(b.source_valid,false) FROM eez.canonical_settled_l2_blocks b JOIN eez.settlements s ON s.id=b.settlement_id JOIN eez.posting_receipts p ON p.chain_instance_id=b.l1_chain_id AND p.transaction_hash=b.settlement_transaction_hash AND p.block_hash=b.settlement_block_hash WHERE b.l1_chain_id=$1 AND b.l2_chain_id=$2 AND p.recipient=$3::bytea ORDER BY b.l1_block_number DESC,s.transaction_index DESC,b.number DESC LIMIT 1";
async fn target(db: &Database, l1: i64, l2: i64, eez: Address) -> Result<Target, &'static str> {
    let row = db
        .client
        .query_opt(TARGET_SQL, &[&l1, &l2, &eez.as_slice()])
        .bounded()
        .await
        .map_err(|_| "database")?
        .ok_or("no_settlement")?;
    let number = |i: usize| -> Result<u64, &'static str> {
        row.try_get::<_, String>(i)
            .map_err(|_| "database")?
            .parse()
            .map_err(|_| "invalid_target")
    };
    let hash = |i: usize| -> Result<B256, &'static str> {
        let bytes: Vec<u8> = row.try_get(i).map_err(|_| "database")?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| "invalid_target")?;
        Ok(B256::from(bytes))
    };
    if !row.try_get::<_, bool>(6).map_err(|_| "database")? {
        return Err("stale_source");
    }
    Ok(Target {
        reference: json!({"l1BlockNumber":number(0)?,"l1BlockHash":hash(1)?,"transactionHash":hash(2)?,"transactionIndex":number(3)?,"l2BlockNumber":number(4)?}),
        hash: hash(5)?,
    })
}
fn digest(value: &Value) -> Result<B256, &'static str> {
    value
        .as_str()
        .ok_or("invalid_evidence")?
        .parse()
        .map_err(|_| "invalid_evidence")
}
fn header(value: Value) -> Result<Header, &'static str> {
    // Verify the header content against its returned hash, not only stateRoot.
    let expected = digest(&value["hash"])?;
    let header: Header = serde_json::from_value(value).map_err(|_| "invalid_header")?;
    if header.hash_slow() != expected {
        return Err("invalid_header");
    }
    Ok(header)
}
fn rpc_error(error: Failure) -> &'static str {
    match error {
        Failure::Unsupported => "unsupported",
        Failure::Identity => "identity",
        _ => "rpc",
    }
}
async fn remote_header(
    rpc: &Rpc,
    endpoint: &Endpoint,
    selector: Value,
) -> Result<Option<Header>, &'static str> {
    let value = rpc
        .block(endpoint, selector, false)
        .await
        .map_err(rpc_error)?;
    if value.is_null() {
        return Ok(None);
    }
    header(value).map(Some)
}
fn identity(value: &Value, config: &Config, follower: &FollowerConfig) -> Result<(), &'static str> {
    if value["version"] != 1
        || value["epoch"]
            .as_str()
            .is_none_or(|value| value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("unsupported");
    }
    let scope = &value["identity"];
    if scope["role"] != "follower" {
        return Err("wrong_role");
    }
    if scope["mode"] != "l1_only" {
        return Err("sequencer_overlay");
    }
    if scope["l1ChainId"] != config.exporter.l1.chain_id
        || scope["rollupId"] != config.rollup_id
        || scope["eez"]
            .as_str()
            .and_then(|value| value.parse::<Address>().ok())
            != Some(follower.eez)
    {
        return Err("scope");
    }
    Ok(())
}
fn compare(
    value: &Value,
    target: &Target,
    expected: &Header,
    actual: &Header,
) -> Result<bool, &'static str> {
    let evidence = &value["evidence"];
    if evidence.is_null() {
        return Err("reused_or_unproven");
    }
    if evidence["reference"] != target.reference {
        return Err("invalid_evidence");
    }
    let executed: Header =
        serde_json::from_value(evidence["header"].clone()).map_err(|_| "invalid_evidence")?;
    if executed != *actual || executed.number != expected.number {
        return Err("noncanonical_evidence");
    }
    if actual.parent_hash != expected.parent_hash {
        return Err("different_ancestry");
    }
    match evidence["outcome"].as_str() {
        Some("settled")
            if value["ready"] == true
                && !value["through"].is_null()
                && actual.hash_slow() == target.hash =>
        {
            Ok(actual.state_root == expected.state_root)
        }
        Some("claimed_state_mismatch")
            if evidence["ageMs"].as_u64().is_some_and(|age| age <= 90000)
                && actual.hash_slow() != target.hash =>
        {
            if actual.state_root == expected.state_root {
                Err("header_mismatch")
            } else {
                Ok(false)
            }
        }
        _ => Err("unproven"),
    }
}
fn compare_template_failure(
    value: &Value,
    target: &Target,
    expected: &Header,
    parent: &Header,
) -> Result<bool, &'static str> {
    let evidence = &value["evidence"];
    if evidence["outcome"] != "template_mismatch"
        || evidence["reference"] != target.reference
        || digest(&evidence["advertisedHash"])? != target.hash
        || evidence["ageMs"].as_u64().is_none_or(|age| age > 90000)
    {
        return Err("invalid_evidence");
    }
    let computed: Header =
        serde_json::from_value(evidence["header"].clone()).map_err(|_| "invalid_evidence")?;
    if computed.number != expected.number
        || computed.parent_hash != expected.parent_hash
        || parent.number.checked_add(1) != Some(expected.number)
        || parent.hash_slow() != expected.parent_hash
    {
        return Err("different_ancestry");
    }
    if computed.hash_slow() == target.hash || computed.state_root == expected.state_root {
        return Err("header_mismatch");
    }
    Ok(false)
}
fn same_generation(first: &Value, last: &Value) -> bool {
    ["epoch", "identity", "generation", "through", "ready"]
        .iter()
        .all(|key| first[*key] == last[*key])
        && first["generation"].as_u64().is_some()
        && ["reference", "header", "outcome", "advertisedHash"]
            .iter()
            .all(|key| first["evidence"][*key] == last["evidence"][*key])
}
async fn cycle(config: &Config, rpc: &Rpc) -> Result<Status, &'static str> {
    let follower = config.follower.as_ref().ok_or("disabled")?;
    let endpoint = follower.endpoint(config);
    let settings = DatabaseConfig::load(&config.database_config).map_err(|_| "database")?;
    let db = Database::connect(&settings).await.map_err(|_| "database")?;
    db.verify_writer().await.map_err(|_| "database")?;
    let l1 = ledger::find_chain(&db, config, &config.exporter.l1, "l1")
        .await
        .map_err(|_| "database")?
        .ok_or("no_settlement")?;
    let l2 = ledger::find_chain(&db, config, &config.exporter.l2, "l2")
        .await
        .map_err(|_| "database")?
        .ok_or("no_settlement")?;
    let target = target(&db, l1, l2, follower.eez).await?;
    let state = compare_remote(config, rpc, follower, &endpoint, &target).await?;
    // Never retain agreement after the receipt/indexer source changes/expires.
    if self::target(&db, l1, l2, follower.eez).await? != target {
        return Err("reorg");
    }
    Ok(state)
}
async fn compare_remote(
    config: &Config,
    rpc: &Rpc,
    follower: &FollowerConfig,
    endpoint: &Endpoint,
    target: &Target,
) -> Result<Status, &'static str> {
    for source in [&config.exporter.l1, &config.exporter.l2, endpoint] {
        rpc.validate_identity(source).await.map_err(rpc_error)?;
    }
    let number = target.reference["l2BlockNumber"]
        .as_u64()
        .ok_or("invalid_target")?;
    let l1_number = target.reference["l1BlockNumber"]
        .as_u64()
        .ok_or("invalid_target")?;
    let anchor = digest(&target.reference["l1BlockHash"])?;
    let selector = json!(format!("0x{number:x}"));
    let expected = remote_header(rpc, &config.exporter.l2, selector.clone())
        .await?
        .ok_or("no_settlement")?;
    if expected.number != number || expected.hash_slow() != target.hash {
        return Err("reorg");
    }
    let first = rpc
        .call(
            &endpoint.url,
            "eez_getDerivationEvidence",
            json!([target.reference]),
        )
        .await
        .map_err(rpc_error)?;
    identity(&first, config, follower)?;
    let (agreement, observed_selector, observed_header) =
        if first["evidence"]["outcome"] == "template_mismatch" {
            // STF rejected the computed block before engine commit. Verify its
            // authenticated expected template and current canonical parent instead
            // of demanding that the rejected candidate became canonical.
            let parent_selector = json!(format!(
                "0x{:x}",
                number.checked_sub(1).ok_or("invalid_evidence")?
            ));
            let parent = remote_header(rpc, endpoint, parent_selector.clone())
                .await?
                .ok_or("lag")?;
            (
                compare_template_failure(&first, target, &expected, &parent)?,
                parent_selector,
                parent,
            )
        } else {
            let latest = remote_header(rpc, endpoint, json!("latest"))
                .await?
                .ok_or("lag")?;
            if latest.number < number {
                return Ok(Status {
                    reason: Some("lag"),
                    lag: Some(number - latest.number),
                    ..Default::default()
                });
            }
            let actual = remote_header(rpc, endpoint, selector.clone())
                .await?
                .ok_or("lag")?;
            (
                compare(&first, target, &expected, &actual)?,
                selector.clone(),
                actual,
            )
        };
    if agreement {
        let safe = remote_header(rpc, endpoint, json!("safe"))
            .await?
            .ok_or("lag")?;
        let through = first["through"]["number"]
            .as_u64()
            .ok_or("invalid_evidence")?;
        if safe.number < number || through < l1_number {
            return Err("lag");
        }
        if super::remote_block(rpc, &config.exporter.l1, through)
            .await
            .map_err(index_error)?
            .hash
            != digest(&first["through"]["hash"])?
        {
            return Err("reorg");
        }
    }
    let last = rpc
        .call(
            &endpoint.url,
            "eez_getDerivationEvidence",
            json!([target.reference]),
        )
        .await
        .map_err(rpc_error)?;
    if !same_generation(&first, &last) {
        return Err("reorg");
    }
    if super::remote_block(rpc, &config.exporter.l1, l1_number)
        .await
        .map_err(index_error)?
        .hash
        != anchor
        || remote_header(rpc, &config.exporter.l2, selector.clone())
            .await?
            .as_ref()
            != Some(&expected)
        || remote_header(rpc, endpoint, observed_selector)
            .await?
            .as_ref()
            != Some(&observed_header)
    {
        return Err("reorg");
    }
    Ok(Status {
        agreement: Some(agreement),
        lag: agreement.then_some(0),
        ..Default::default()
    })
}
fn index_error(error: Error) -> &'static str {
    match error {
        Error::Rpc(error) => rpc_error(error),
        _ => "reorg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(json!({"exporter":{"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":format!("{:#x}",B256::ZERO)},"l2":{"url":"http://localhost:2","chain_id":2,"genesis_hash":format!("{:#x}",B256::ZERO)}},"environment":"test","deployment":"follower","rollup_id":1,"database_config":"unused.json","l1":{"bootstrap":0,"contracts":[format!("{:#x}",Address::repeat_byte(1))]},"l2":{"bootstrap":0,"include_all_transactions":true}})).unwrap()
    }
    fn fixture() -> (Config, FollowerConfig, Target, Header, Value) {
        let config = config();
        let follower = FollowerConfig {
            url: "http://localhost:3".into(),
            eez: Address::repeat_byte(1),
        };
        let header = Header {
            number: 2,
            parent_hash: B256::repeat_byte(2),
            state_root: B256::repeat_byte(3),
            ..Default::default()
        };
        let target = Target {
            reference: json!({"l1BlockNumber":10,"l1BlockHash":B256::repeat_byte(4),"transactionHash":B256::repeat_byte(5),"transactionIndex":0,"l2BlockNumber":2}),
            hash: header.hash_slow(),
        };
        let value = json!({"version":1,"epoch":"0123456789abcdef0123456789abcdef","identity":{"role":"follower","mode":"l1_only","l1ChainId":1,"rollupId":1,"eez":follower.eez},"ready":true,"generation":1,"through":{"number":10,"hash":B256::repeat_byte(4)},"evidence":{"reference":target.reference,"header":header,"outcome":"settled","ageMs":1}});
        (config, follower, target, header, value)
    }
    #[test]
    fn config_is_optional_explicit_validated_and_credentials_redacted() {
        let mut config = config();
        assert!(config.follower.is_none());
        config.validate().unwrap();
        config.follower = Some(FollowerConfig {
            url: "http://secret:password@localhost:3".into(),
            eez: Address::repeat_byte(1),
        });
        config.validate().unwrap();
        assert!(!format!("{config:?}").contains("password"));
        config.follower.as_mut().unwrap().eez = Address::ZERO;
        assert!(config.validate().is_err());
        config.follower.as_mut().unwrap().eez = Address::repeat_byte(1);
        config.follower.as_mut().unwrap().url = "file:///secret".into();
        assert!(config.validate().is_err());
    }
    #[test]
    fn role_mode_scope_and_legacy_endpoints_cannot_claim_independence() {
        let (config, follower, _, _, mut value) = fixture();
        identity(&value, &config, &follower).unwrap();
        value["identity"]["role"] = json!("composer");
        assert_eq!(identity(&value, &config, &follower), Err("wrong_role"));
        value["identity"]["role"] = json!("follower");
        value["identity"]["mode"] = json!("sequencer_overlay");
        assert_eq!(
            identity(&value, &config, &follower),
            Err("sequencer_overlay")
        );
        value["identity"]["mode"] = json!("l1_only");
        value["identity"]["rollupId"] = json!(2);
        assert_eq!(identity(&value, &config, &follower), Err("scope"));
        assert_eq!(
            identity(&Value::Null, &config, &follower),
            Err("unsupported")
        );
    }
    #[test]
    fn exact_provenance_and_canonical_headers_required_for_agreement() {
        let (_, _, target, header, mut value) = fixture();
        assert_eq!(compare(&value, &target, &header, &header), Ok(true));
        value["evidence"]["reference"]["transactionIndex"] = json!(1);
        assert_eq!(
            compare(&value, &target, &header, &header),
            Err("invalid_evidence")
        );
        value["evidence"]["reference"] = target.reference.clone();
        value["through"] = Value::Null;
        assert_eq!(compare(&value, &target, &header, &header), Err("unproven"));
        value["evidence"] = Value::Null;
        assert_eq!(
            compare(&value, &target, &header, &header),
            Err("reused_or_unproven")
        );
    }
    #[test]
    fn only_fresh_actually_executed_same_parent_mismatch_can_disagree() {
        let (_, _, target, expected, mut value) = fixture();
        let mut actual = expected.clone();
        actual.state_root = B256::repeat_byte(9);
        value["evidence"]["header"] = json!(actual);
        value["evidence"]["outcome"] = json!("claimed_state_mismatch");
        value["through"] = Value::Null;
        assert_eq!(compare(&value, &target, &expected, &actual), Ok(false));
        value["evidence"]["ageMs"] = json!(90001);
        assert_eq!(
            compare(&value, &target, &expected, &actual),
            Err("unproven")
        );
        value["evidence"]["ageMs"] = json!(1);
        actual.parent_hash = B256::ZERO;
        value["evidence"]["header"] = json!(actual);
        assert_eq!(
            compare(&value, &target, &expected, &actual),
            Err("different_ancestry")
        );
    }
    #[test]
    fn rejected_computed_candidate_is_compared_without_fabricating_canonical_commit() {
        let (_, _, mut target, mut expected, mut value) = fixture();
        let parent = Header {
            number: 1,
            ..Default::default()
        };
        expected.parent_hash = parent.hash_slow();
        target.hash = expected.hash_slow();
        let mut computed = expected.clone();
        computed.state_root = B256::repeat_byte(9);
        value["evidence"]["header"] = json!(computed);
        value["evidence"]["outcome"] = json!("template_mismatch");
        value["evidence"]["advertisedHash"] = json!(target.hash);
        value["through"] = Value::Null;
        assert_eq!(
            compare_template_failure(&value, &target, &expected, &parent),
            Ok(false)
        );
        value["evidence"]["advertisedHash"] = json!(B256::ZERO);
        assert_eq!(
            compare_template_failure(&value, &target, &expected, &parent),
            Err("invalid_evidence")
        );
        value["evidence"]["advertisedHash"] = json!(target.hash);
        value["evidence"]["ageMs"] = json!(90001);
        assert_eq!(
            compare_template_failure(&value, &target, &expected, &parent),
            Err("invalid_evidence")
        );
        value["evidence"]["ageMs"] = json!(1);
        assert_eq!(
            compare_template_failure(&value, &target, &expected, &Header::default()),
            Err("different_ancestry")
        );
    }
    #[test]
    fn forged_header_hash_reorg_epoch_and_same_height_replacement_invalidate() {
        let (_, _, _, header, first) = fixture();
        let mut json = json!(header);
        json["hash"] = json!(header.hash_slow());
        assert_eq!(super::header(json.clone()).unwrap(), header);
        json["stateRoot"] = json!(B256::ZERO);
        assert_eq!(super::header(json), Err("invalid_header"));
        assert!(same_generation(&first, &first));
        for key in ["epoch", "generation", "through"] {
            let mut changed = first.clone();
            changed[key] = Value::Null;
            assert!(!same_generation(&first, &changed));
        }
        let mut changed = first.clone();
        changed["evidence"]["header"]["stateRoot"] = json!(B256::ZERO);
        assert!(!same_generation(&first, &changed));
    }
    #[test]
    fn disabled_unknown_and_stale_never_emit_agreement_or_zero_as_data() {
        assert!(!render(None, 100).contains("state_root_agreement"));
        let state = Status {
            at: Some(1),
            agreement: Some(true),
            ..Default::default()
        };
        assert!(render(Some(&state), 2).contains("state_root_agreement 1"));
        assert!(!render(Some(&state), 90002).contains("state_root_agreement"));
        let failed = Status {
            at: Some(100),
            reason: Some("lag"),
            lag: Some(2),
            ..Default::default()
        };
        let rendered = render(Some(&failed), 100);
        assert!(!rendered.contains("state_root_agreement"));
        assert!(rendered.contains("settled_lag_blocks 2"));
    }
    #[tokio::test]
    async fn wire_comparison_agreement_mismatch_lag_reuse_roles_and_mid_poll_reorg() {
        use axum::{
            Router,
            body::Bytes,
            extract::{Path, State},
            routing::post,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Fixture {
            mode: AtomicUsize,
            reads: AtomicUsize,
            calls: AtomicUsize,
            diagnostic: Value,
            l1: Header,
            l2: Header,
            parent: Header,
        }
        async fn serve(
            State(state): State<Arc<Fixture>>,
            Path(role): Path<String>,
            body: Bytes,
        ) -> String {
            let request: Value = serde_json::from_slice(&body).unwrap();
            state.calls.fetch_add(1, Ordering::SeqCst);
            let mode = state.mode.load(Ordering::SeqCst);
            let result = match request["method"].as_str().unwrap() {
                "eth_chainId" => json!(if role == "l1" { "0x1" } else { "0x2" }),
                "eez_getDerivationEvidence" => {
                    if mode == 8 {
                        return json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601}}).to_string();
                    }
                    let mut diagnostic = state.diagnostic.clone();
                    match mode {
                        1 => diagnostic["identity"]["role"] = json!("composer"),
                        2 => diagnostic["identity"]["mode"] = json!("sequencer_overlay"),
                        3 => diagnostic["evidence"] = Value::Null,
                        5 => {
                            diagnostic["generation"] =
                                json!(state.reads.fetch_add(1, Ordering::SeqCst))
                        }
                        6 => {
                            let mut actual = state.l2.clone();
                            actual.state_root = B256::repeat_byte(9);
                            diagnostic["evidence"]["header"] = json!(actual);
                            diagnostic["evidence"]["outcome"] = json!("claimed_state_mismatch");
                            diagnostic["through"] = Value::Null;
                        }
                        9 => diagnostic["identity"]["rollupId"] = json!(99),
                        10 | 11 => {
                            let mut computed = state.l2.clone();
                            computed.state_root = B256::repeat_byte(9);
                            diagnostic["evidence"]["header"] = json!(computed);
                            diagnostic["evidence"]["outcome"] = json!("template_mismatch");
                            diagnostic["evidence"]["advertisedHash"] = json!(if mode == 10 {
                                state.l2.hash_slow()
                            } else {
                                B256::ZERO
                            });
                            diagnostic["through"] = Value::Null;
                        }
                        _ => {}
                    }
                    diagnostic
                }
                "eth_getBlockByNumber" => {
                    if role == "follower"
                        && matches!(mode, 10 | 11)
                        && request["params"][0] == "0x2"
                    {
                        return json!({"jsonrpc":"2.0","id":1,"result":null}).to_string();
                    }
                    let mut header = if request["params"][0] == "0x0" {
                        Header::default()
                    } else if role == "l1" {
                        state.l1.clone()
                    } else if request["params"][0] == "0x1" {
                        state.parent.clone()
                    } else {
                        state.l2.clone()
                    };
                    if role == "follower" && header.number != 0 {
                        if mode == 4 {
                            header.number = 1;
                        }
                        if mode == 6 {
                            header.state_root = B256::repeat_byte(9);
                        }
                    }
                    let mut value = json!(header);
                    value["hash"] = json!(header.hash_slow());
                    if role == "follower" && mode == 7 && header.number != 0 {
                        value["stateRoot"] = json!(B256::ZERO);
                    }
                    value
                }
                _ => panic!("unexpected follower observer RPC"),
            };
            json!({"jsonrpc":"2.0","id":1,"result":result}).to_string()
        }
        let (mut config, mut follower, mut target, mut expected, mut diagnostic) = fixture();
        let parent = Header {
            number: 1,
            ..Default::default()
        };
        expected.parent_hash = parent.hash_slow();
        target.hash = expected.hash_slow();
        diagnostic["evidence"]["header"] = json!(expected);
        let l1 = Header {
            number: 10,
            ..Default::default()
        };
        target.reference["l1BlockHash"] = json!(l1.hash_slow());
        diagnostic["evidence"]["reference"] = target.reference.clone();
        diagnostic["through"]["hash"] = json!(l1.hash_slow());
        let state = Arc::new(Fixture {
            mode: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            diagnostic,
            l1,
            l2: expected,
            parent,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{role}", post(serve))
            .with_state(Arc::clone(&state));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config.exporter.l1.url = format!("{base}/l1");
        config.exporter.l2.url = format!("{base}/l2");
        config.exporter.l1.genesis_hash = format!("{:#x}", Header::default().hash_slow());
        config.exporter.l2.genesis_hash = config.exporter.l1.genesis_hash.clone();
        follower.url = format!("{base}/follower");
        let endpoint = follower.endpoint(&config);
        let rpc = Rpc::new().unwrap();
        let disabled = Arc::new(tokio::sync::RwLock::new(
            super::super::RuntimeStatus::default(),
        ));
        poll(&config, &rpc, &disabled).await;
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            0,
            "disabled does no network or database work"
        );
        let healthy = compare_remote(&config, &rpc, &follower, &endpoint, &target)
            .await
            .unwrap();
        assert_eq!(healthy.agreement, Some(true));
        for (mode, reason) in [
            (1, "wrong_role"),
            (2, "sequencer_overlay"),
            (3, "reused_or_unproven"),
            (5, "reorg"),
            (7, "invalid_header"),
            (8, "unsupported"),
            (9, "scope"),
        ] {
            state.mode.store(mode, Ordering::SeqCst);
            assert_eq!(
                compare_remote(&config, &rpc, &follower, &endpoint, &target)
                    .await
                    .unwrap_err(),
                reason,
                "mode {mode}"
            );
        }
        state.mode.store(4, Ordering::SeqCst);
        let lag = compare_remote(&config, &rpc, &follower, &endpoint, &target)
            .await
            .unwrap();
        assert_eq!(lag.agreement, None);
        assert_eq!(lag.lag, Some(1));
        state.mode.store(6, Ordering::SeqCst);
        let mismatch = compare_remote(&config, &rpc, &follower, &endpoint, &target)
            .await
            .unwrap();
        assert_eq!(mismatch.agreement, Some(false));
        state.mode.store(10, Ordering::SeqCst);
        let rejected = compare_remote(&config, &rpc, &follower, &endpoint, &target)
            .await
            .unwrap();
        assert_eq!(rejected.agreement, Some(false));
        assert_eq!(rejected.lag, None);
        state.mode.store(11, Ordering::SeqCst);
        assert_eq!(
            compare_remote(&config, &rpc, &follower, &endpoint, &target)
                .await
                .unwrap_err(),
            "invalid_evidence"
        );
        server.abort();
    }
}
