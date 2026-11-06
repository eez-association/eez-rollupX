//! Native process fixture: separate real signers and the unchanged Core manager.

use super::*;

sol! {
    function addProofSystem(address proofSystem, bytes32 vkey) external;
    function setThreshold(uint256 newThreshold) external;
}

pub struct TestQuorumConfig {
    /// Public fixture keys, additional to the fixture's primary attester.
    pub additional_keys: Vec<&'static str>,
    pub threshold: u64,
    /// Register this additional member with a key for a different profile.
    pub wrong_profile_member: Option<usize>,
}

pub struct TestQuorum {
    pub members: Vec<(Address, Address)>,
    pub signers: Vec<ProofSignerHandle>,
    entries: Vec<String>,
    keys: Vec<B256>,
    threshold: u64,
}

impl TestQuorum {
    pub(super) async fn start(
        cfg: &CrossChainConfig,
        profile: NetworkProfile,
        primary: &str,
        config: &TestQuorumConfig,
    ) -> Result<Self> {
        ensure!(
            !config.additional_keys.is_empty() && config.additional_keys.len() < 16,
            "quorum fixture needs 2 to 16 members"
        );
        ensure!(
            config.threshold > 0 && config.threshold <= config.additional_keys.len() as u64 + 1,
            "invalid fixture threshold"
        );
        let primary_attester = signer_address(cfg.attester_key)?;
        let mut fixture = Self {
            members: vec![(cfg.proof_system_address, primary_attester)],
            signers: Vec::new(),
            entries: vec![format!(
                "{primary}={primary_attester:#x}={:#x}",
                cfg.proof_system_address
            )],
            keys: Vec::new(),
            threshold: config.threshold,
        };
        let deployer = signer_address(cfg.deployer_key)?;
        for (index, &key) in config.additional_keys.iter().enumerate() {
            // The existing five deployments plus registerRollup consume 0..5.
            // Deploy all extra proof systems before their registration calls.
            let system = deployer.create(6 + index as u64);
            let attester = signer_address(key)?;
            let vkey = profile.verification_key(cfg.eez_address, system, attester);
            let signer = ProofSignerHandle::spawn(&ProofSignerConfig {
                rollup_manager_address: cfg.rollup_manager_address,
                registry_address: cfg.eez_address,
                chain_config: &cfg.l2_genesis.0,
                l1_chain_config: cfg.l1_chain_config(),
                rollup_id: cfg.rollup_id,
                signer_key: key,
                vkey,
                proof_system: system,
                max_request_blocks: None,
            })
            .await?;
            fixture
                .entries
                .push(format!("{}={attester:#x}={system:#x}", signer.endpoint()));
            fixture
                .keys
                .push(if config.wrong_profile_member == Some(index) {
                    B256::repeat_byte(0x77)
                } else {
                    vkey
                });
            fixture.members.push((system, attester));
            fixture.signers.push(signer);
        }
        Ok(fixture)
    }

    pub(super) fn configuration(&self) -> String {
        self.entries.join(",")
    }

    pub(super) async fn register(&self, rpc: &str, dep: &Deployment, deployer: &str) -> Result<()> {
        let mut nonce = onchain_nonce(rpc, deployer).await?;
        ensure!(nonce == 6, "fixture protocol deployment nonce changed");
        let artifact = repo_root().join("contracts/out/ECDSAProofSystem.sol/ECDSAProofSystem.json");
        let mut hashes = Vec::new();
        for &(system, attester) in &self.members[1..] {
            ensure!(
                signer_address(deployer)?.create(nonce) == system,
                "unexpected verifier CREATE address"
            );
            let hash = sign_and_send(
                rpc,
                deployer,
                DEV_CHAIN_ID,
                nonce,
                None,
                U256::ZERO,
                deployment_input(&artifact, attester.abi_encode())?,
                1_000_000,
            )
            .await?;
            hashes.push(hash);
            nonce += 1;
        }
        for hash in hashes {
            wait_for_successful_receipt(rpc, hash, "quorum verifier deployment").await?;
        }
        let mut hashes = Vec::new();
        for (&(system, _), &vkey) in self.members[1..].iter().zip(&self.keys) {
            let hash = sign_and_send(
                rpc,
                deployer,
                DEV_CHAIN_ID,
                nonce,
                Some(dep.rollup_manager_address),
                U256::ZERO,
                addProofSystemCall {
                    proofSystem: system,
                    vkey,
                }
                .abi_encode(),
                500_000,
            )
            .await?;
            hashes.push(hash);
            nonce += 1;
        }
        for hash in hashes {
            wait_for_successful_receipt(rpc, hash, "quorum member registration").await?;
        }
        let hash = sign_and_send(
            rpc,
            deployer,
            DEV_CHAIN_ID,
            nonce,
            Some(dep.rollup_manager_address),
            U256::ZERO,
            setThresholdCall {
                newThreshold: U256::from(self.threshold),
            }
            .abi_encode(),
            500_000,
        )
        .await?;
        wait_for_successful_receipt(rpc, hash, "quorum threshold").await?;
        Ok(())
    }
}
