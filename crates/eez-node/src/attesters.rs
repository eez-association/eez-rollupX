//! Immutable prover selection at startup. Each endpoint owns one v2 session.

use std::{str::FromStr, time::Duration};

use alloy_primitives::Address;

#[derive(Debug)]
pub(super) struct Config {
    pub members: Vec<Member>,
    pub grace: Duration,
    pub explicit: bool,
}

#[derive(Debug)]
pub(super) struct Member {
    pub url: String,
    pub attester: Address,
    pub proof_system: Address,
}

impl Config {
    pub fn from_env(proof_system: Address) -> eyre::Result<Self> {
        Self::read(proof_system, |name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(eyre::eyre!("{name} contains non-UTF-8 bytes")),
        })
    }

    fn read(
        proof_system: Address,
        read: impl Fn(&str) -> eyre::Result<Option<String>>,
    ) -> eyre::Result<Self> {
        let list = read("EEZ_PROVERS")?;
        let explicit = list.is_some();
        let members = match list {
            Some(list) => {
                let mut members = Vec::new();
                for entry in list.split(',') {
                    eyre::ensure!(
                        members.len() < eez_composer::attestation_quorum::MAX_ATTESTERS,
                        "EEZ_PROVERS exceeds the member limit"
                    );
                    let mut fields = entry.rsplitn(3, '=');
                    let (Some(system), Some(attester), Some(url)) =
                        (fields.next(), fields.next(), fields.next())
                    else {
                        eyre::bail!("EEZ_PROVERS requires url=attester=proof_system entries");
                    };
                    members.push(Member::new(
                        url,
                        attester,
                        address(system, "proof system")?,
                    )?);
                }
                members
            }
            None => vec![Member::new(
                &required(&read, "EEZ_PROVER_URL")?,
                &required(&read, "EEZ_ATTESTER_ADDRESS")?,
                proof_system,
            )?],
        };
        let grace = read("EEZ_ATTESTATION_GRACE_MS")?.map_or(
            Ok(eez_composer::DEFAULT_ATTESTATION_GRACE),
            |value| {
                value
                    .trim()
                    .parse::<u64>()
                    .map(Duration::from_millis)
                    .map_err(|_| {
                        eyre::eyre!("EEZ_ATTESTATION_GRACE_MS must be an unsigned integer")
                    })
            },
        )?;
        // Avoid Instant overflow and accidental unbounded post-threshold waits.
        eyre::ensure!(
            grace <= Duration::from_secs(60),
            "attestation grace exceeds 60000 ms"
        );
        Ok(Self {
            members,
            grace,
            explicit,
        })
    }
}

fn required(
    read: &impl Fn(&str) -> eyre::Result<Option<String>>,
    name: &str,
) -> eyre::Result<String> {
    read(name)?.ok_or_else(|| eyre::eyre!("{name} is required when EEZ_PROVERS is unset"))
}

fn address(value: &str, name: &str) -> eyre::Result<Address> {
    Address::from_str(value.trim())
        .ok()
        .filter(|address| !address.is_zero())
        .ok_or_else(|| eyre::eyre!("{name} must be a nonzero 20-byte address"))
}

impl Member {
    fn new(url: &str, attester: &str, proof_system: Address) -> eyre::Result<Self> {
        eyre::ensure!(!proof_system.is_zero(), "proof system must be nonzero");
        let url = url.trim();
        eyre::ensure!(!url.is_empty(), "prover URL must not be empty");
        Ok(Self {
            url: url.into(),
            attester: address(attester, "attester")?,
            proof_system,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SIGNER: &str = "0x1111111111111111111111111111111111111111";
    const SYSTEM: &str = "0x2222222222222222222222222222222222222222";

    fn config(values: &[(&str, &str)]) -> eyre::Result<Config> {
        Config::read(SYSTEM.parse().unwrap(), |name| {
            Ok(values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string()))
        })
    }

    #[test]
    fn explicit_list_overrides_legacy_and_preserves_url_equals() {
        let list = format!("https://host/prover?key=value={SIGNER}={SYSTEM}");
        let config = config(&[
            ("EEZ_PROVERS", &list),
            ("EEZ_PROVER_URL", ""),
            ("EEZ_ATTESTER_ADDRESS", "invalid"),
        ])
        .unwrap();
        assert!(config.explicit);
        assert_eq!(config.members[0].url, "https://host/prover?key=value");
        assert_eq!(
            config.members[0].proof_system,
            SYSTEM.parse::<Address>().unwrap()
        );
        assert_eq!(config.grace, eez_composer::DEFAULT_ATTESTATION_GRACE);
    }

    #[test]
    fn malformed_explicit_lists_never_fall_back() {
        for list in ["", " ", ",", "http://a", "http://a=bad=bad"] {
            assert!(
                config(&[
                    ("EEZ_PROVERS", list),
                    ("EEZ_PROVER_URL", "http://legacy"),
                    ("EEZ_ATTESTER_ADDRESS", SIGNER)
                ])
                .is_err(),
                "accepted {list:?}"
            );
        }
    }

    #[test]
    fn legacy_defaults_and_grace_bounds_are_checked() {
        let legacy = [
            ("EEZ_PROVER_URL", "http://legacy"),
            ("EEZ_ATTESTER_ADDRESS", SIGNER),
        ];
        assert!(!config(&legacy).unwrap().explicit);
        for value in ["", "-1", "60001", "18446744073709551615"] {
            assert!(config(&[legacy[0], legacy[1], ("EEZ_ATTESTATION_GRACE_MS", value)]).is_err());
        }
        assert!(config(&[legacy[0], legacy[1], ("EEZ_ATTESTATION_GRACE_MS", "0")]).is_ok());
        assert!(config(&[]).is_err());
    }
}
