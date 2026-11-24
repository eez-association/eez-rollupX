//! Immutable prover selection at startup. Each endpoint owns one v2 session.

use std::{num::NonZeroUsize, str::FromStr};

use alloy_primitives::Address;

#[derive(Debug)]
pub(super) struct Config {
    pub members: Vec<Member>,
    pub explicit: bool,
    pub stream_max_in_flight: NonZeroUsize,
}

#[derive(Debug)]
pub(super) struct Member {
    pub url: String,
    pub attester: Address,
    pub proof_system: Address,
}

impl Config {
    pub fn from_env() -> eyre::Result<Self> {
        Self::read(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(eyre::eyre!("{name} contains non-UTF-8 bytes")),
        })
    }

    fn read(read: impl Fn(&str) -> eyre::Result<Option<String>>) -> eyre::Result<Self> {
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
                let mut urls = std::collections::HashSet::new();
                for member in &members {
                    let url = reqwest::Url::parse(&member.url)
                        .map_err(|_| eyre::eyre!("invalid prover endpoint"))?;
                    eyre::ensure!(
                        matches!(url.scheme(), "http" | "https") && url.host().is_some(),
                        "invalid prover endpoint"
                    );
                    eyre::ensure!(urls.insert(url.origin()), "duplicate prover origin");
                }
                members
            }
            None => vec![Member::new(
                &required(&read, "EEZ_PROVER_URL")?,
                &required(&read, "EEZ_ATTESTER_ADDRESS")?,
                address(
                    &required(&read, "EEZ_ECDSA_PROOF_SYSTEM_ADDRESS")?,
                    "proof system",
                )?,
            )?],
        };
        let stream_max_in_flight = match read("EEZ_PROVER_STREAM_MAX_IN_FLIGHT")? {
            Some(value) => value.parse::<NonZeroUsize>().map_err(|_| {
                eyre::eyre!(
                    "EEZ_PROVER_STREAM_MAX_IN_FLIGHT must be a positive integer from 1 through {}",
                    usize::MAX
                )
            })?,
            None => eez_prover_client::DEFAULT_STREAM_MAX_IN_FLIGHT,
        };
        Ok(Self {
            members,
            explicit,
            stream_max_in_flight,
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
        Config::read(|name| {
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
        assert_eq!(config.stream_max_in_flight.get(), 32);
        assert_eq!(config.members[0].url, "https://host/prover?key=value");
        assert_eq!(
            config.members[0].proof_system,
            SYSTEM.parse::<Address>().unwrap()
        );
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
    fn legacy_tuple_is_checked() {
        let legacy = [
            ("EEZ_ECDSA_PROOF_SYSTEM_ADDRESS", SYSTEM),
            ("EEZ_PROVER_URL", "http://legacy"),
            ("EEZ_ATTESTER_ADDRESS", SIGNER),
        ];
        let parsed = config(&legacy).unwrap();
        assert!(!parsed.explicit);
        assert_eq!(parsed.stream_max_in_flight.get(), 32);
        assert!(config(&[]).is_err());
    }

    #[test]
    fn stream_window_accepts_positive_representable_counts() {
        let list = format!("http://remote={SIGNER}={SYSTEM}");
        let config_with_limit = |value| {
            config(&[
                ("EEZ_PROVERS", &list),
                ("EEZ_PROVER_STREAM_MAX_IN_FLIGHT", value),
            ])
        };
        let max = usize::MAX.to_string();
        for (raw, expected) in [
            ("1", 1),
            ("8", 8),
            ("32", 32),
            ("128", 128),
            (&max, usize::MAX),
        ] {
            assert_eq!(
                config_with_limit(raw).unwrap().stream_max_in_flight.get(),
                expected
            );
        }
        for raw in [
            "",
            "0",
            "eight",
            "-1",
            " 8",
            "8 ",
            "340282366920938463463374607431768211455",
        ] {
            let error = config_with_limit(raw).unwrap_err().to_string();
            assert!(
                error.contains("EEZ_PROVER_STREAM_MAX_IN_FLIGHT")
                    && error.contains("positive integer"),
                "{error}"
            );
        }
    }
}
