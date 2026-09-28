//! Preserve the chain document used by both L2 roles through fixed-rule loading.

use std::sync::{Arc, Mutex};

use alloy_primitives::B256;
use clap::{CommandFactory as _, FromArgMatches as _};
use eez_l1::chain_config::parse_l2_genesis;
use reth_chainspec::ChainSpec;
use reth_ethereum_cli::{chainspec, interface::Cli};

type NodeCli<Ext> = Cli<chainspec::EthereumChainSpecParser, Ext>;

/// Parse the existing CLI with a strict chain value parser. The returned digest
/// belongs to the same read that constructed the node's actual execution rules.
/// Non-node subcommands retain their existing parser and return no digest.
pub fn node_cli_with_chain_document<Ext>() -> eyre::Result<(NodeCli<Ext>, Option<B256>)>
where
    Ext: clap::Args + std::fmt::Debug,
{
    parse(super::node_cli_arguments())
}

fn parse<Ext>(argv: Vec<String>) -> eyre::Result<(NodeCli<Ext>, Option<B256>)>
where
    Ext: clap::Args + std::fmt::Debug,
{
    // This cell is local to a single synchronous parse, not a runtime/global
    // registry. Clap's value-parser interface requires a Send + Sync closure.
    let source = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&source);
    let command = NodeCli::<Ext>::command().mut_subcommand("node", |command| {
        command.mut_arg("chain", |argument| {
            argument.value_parser(move |value: &str| -> eyre::Result<Arc<ChainSpec>> {
                let (spec, digest) = parse_chain(value)?;
                *captured
                    .lock()
                    .map_err(|_| eyre::eyre!("chain parser identity lock poisoned"))? =
                    Some(digest);
                Ok(spec)
            })
        })
    });
    let matches = command.try_get_matches_from(argv)?;
    let cli = NodeCli::<Ext>::from_arg_matches(&matches)?;
    let digest = *source
        .lock()
        .map_err(|_| eyre::eyre!("chain parser identity lock poisoned"))?;
    Ok((cli, digest))
}

fn parse_chain(value: &str) -> eyre::Result<(Arc<ChainSpec>, B256)> {
    let preset = chainspec::SUPPORTED_CHAINS
        .contains(&value)
        .then(|| chainspec::chain_value_parser(value))
        .transpose()?;
    let encoded = if let Some(spec) = &preset {
        serde_json::to_vec(spec.genesis())?
    } else {
        match std::fs::read(value) {
            Ok(encoded) => encoded,
            Err(_) if value.contains('{') => value.as_bytes().to_vec(),
            Err(error) => return Err(error.into()),
        }
    };
    let loaded = parse_l2_genesis(&encoded)?;
    let digest = loaded.digest();
    let spec = Arc::new(eez_evm::profile::chain_spec(loaded.into_genesis())?);
    if let Some(preset) = preset {
        // A preset with rules outside its genesis cannot share a document-only
        // namespace with the signer. Fail explicitly instead of guessing rules.
        eyre::ensure!(
            *preset == *spec,
            "chain preset has rules outside its Genesis document; configure an explicit new-network genesis"
        );
    }
    Ok((spec, digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_cli::interface::Commands;

    #[test]
    fn actual_clap_chain_selection_preserves_document_and_execution_rules() {
        let mut genesis: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../genesis.json")).unwrap();
        genesis["config"]["chainId"] = 31337.into();
        let encoded = serde_json::to_string(&genesis).unwrap();
        let source = encoded.as_str();
        let expected = parse_l2_genesis(source.as_bytes()).unwrap();
        for arguments in [
            vec!["node", "--chain", source],
            vec!["node", "--dev", "--chain", source],
        ] {
            let mut argv = vec!["eez-composer".to_owned()];
            argv.extend(arguments.into_iter().map(str::to_owned));
            let (cli, digest) = parse::<super::super::NoRoleArgs>(argv).unwrap();
            assert_eq!(digest, Some(expected.digest()));
            let Commands::Node(node) = cli.command else {
                panic!("node command")
            };
            assert_eq!(node.chain.genesis.config.chain_id, 31337);
            assert_eq!(node.chain.genesis.config.osaka_time, Some(0));
            assert_eq!(
                node.chain.base_fee_params_at_timestamp(0),
                reth_chainspec::BaseFeeParams::new(50, 2)
            );
        }
    }

    #[test]
    fn cli_rejects_duplicate_or_unsupported_chain_fields_before_launch() {
        for source in [
            r#"{"config":{"chainId":1,"chainId":2},"alloc":{}}"#,
            r#"{"config":{"chainId":1,"pragueTiem":0},"alloc":{}}"#,
        ] {
            let argv = ["eez-composer", "node", "--chain", source]
                .map(str::to_owned)
                .to_vec();
            assert!(parse::<super::super::NoRoleArgs>(argv).is_err());
        }
    }
}
