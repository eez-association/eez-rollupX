//! Canonical sibling replacement and continued execution on closed cross-version copies.
use super::*;
use crate::testkit::engine_child_sload::{alternative_fixture, extended_fixture, fixture};

async fn close(factory: Factory) -> PathBuf {
    tokio::time::timeout(
        WATCHDOG,
        tokio::task::spawn_blocking(move || close_database(factory)),
    )
    .await
    .expect("reorg database closure deadline")
    .unwrap()
}

fn checked_fixture_root(balance: u64) -> PathBuf {
    let root = PathBuf::from(std::env::var_os("EEZ_TEST_UPGRADE_DATADIRS").expect("fixture root"));
    let path = root.join(balance.to_string());
    assert_eq!(
        std::fs::read_to_string(path.join("issue158-disposable-fixture")).unwrap(),
        balance.to_string()
    );
    path
}

fn check_alternative_oracle(alternative: &Fixture, balance: u64) {
    let expected: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../alternative-child-reth27-vectors.json")).unwrap();
    let row = expected
        .iter()
        .find(|row| row["balance"] == balance)
        .unwrap();
    assert_eq!(alternative.blocks.len(), 7);
    for (index, block) in alternative.blocks.iter().enumerate() {
        assert_eq!(
            hex::encode(&block.rlp),
            row["blocks"][index].as_str().unwrap()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual reorg check: EEZ_TEST_UPGRADE_DATADIRS must contain copied closed height-7 fixtures"]
async fn replace_persisted_child_and_restore_original() {
    for balance in [0u64, 1] {
        let path = checked_fixture_root(balance);
        let original = fixture(balance);
        let alternative = alternative_fixture(balance);
        check_alternative_oracle(&alternative, balance);
        assert_ne!(original.blocks[6].hash, alternative.blocks[6].hash);
        let spec = Arc::new(eez_evm::profile::chain_spec(original.genesis.clone()).unwrap());
        let mut factory = reopen(&path, spec.clone());
        let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
        check_disk(&factory, &original, 7, balance);
        drop(diagnostic);
        for (next, child_storage) in [(&alternative, U256::ZERO), (&original, U256::from(99))] {
            let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
            let old_hash = factory.block_hash(7).unwrap().unwrap();
            assert_ne!(old_hash.as_slice(), next.blocks[6].hash.as_slice());
            persist_stage(&factory, next, 6, 7).await;
            check_disk_with_child_storage(&factory, next, 7, balance, child_storage);
            drop(diagnostic);
            let closed = close(factory).await;
            assert_eq!(closed, path);
            factory = reopen(&path, spec.clone());
            let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
            check_disk_with_child_storage(&factory, next, 7, balance, child_storage);
            drop(diagnostic);
            println!(
                "REORG_REOPEN balance={balance} old={old_hash} new={} child_slot={child_storage}",
                B256::from_slice(&next.blocks[6].hash)
            );
        }
        assert_eq!(close(factory).await, path);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual old-writer check: EEZ_TEST_UPGRADE_DATADIRS must contain copied closed height-7 fixtures"]
async fn append_after_cross_version_writes() {
    for balance in [0u64, 1] {
        let path = checked_fixture_root(balance);
        let fixture = extended_fixture(balance);
        let expected: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../extended-child-reth27-vectors.json")).unwrap();
        let row = expected
            .iter()
            .find(|row| row["balance"] == balance)
            .unwrap();
        assert_eq!(fixture.blocks.len(), 8);
        for (index, block) in fixture.blocks.iter().enumerate() {
            assert_eq!(
                hex::encode(&block.rlp),
                row["blocks"][index].as_str().unwrap()
            );
        }
        let spec = Arc::new(eez_evm::profile::chain_spec(fixture.genesis.clone()).unwrap());
        let factory = reopen(&path, spec.clone());
        let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
        assert_eq!(factory.last_block_number().unwrap(), 7);
        persist_stage(&factory, &fixture, 7, 8).await;
        check_disk_with_child_storage(&factory, &fixture, 8, balance, U256::from(99));
        drop(diagnostic);
        assert_eq!(close(factory).await, path);
        let factory = reopen(&path, spec);
        let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
        check_disk_with_child_storage(&factory, &fixture, 8, balance, U256::from(99));
        drop(diagnostic);
        assert_eq!(close(factory).await, path);
        println!(
            "APPENDED_EMPTY_CHILD balance={balance} rlp={}",
            hex::encode(&fixture.blocks[7].rlp)
        );
    }
}
