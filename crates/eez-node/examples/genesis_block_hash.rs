fn main() -> eyre::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| eyre::eyre!("usage: genesis_block_hash <genesis.json>"))?;
    let genesis: alloy_genesis::Genesis = serde_json::from_slice(&std::fs::read(path)?)?;
    let chain_spec = reth_chainspec::ChainSpec::from_genesis(genesis);
    println!("{}", chain_spec.genesis_header().hash_slow());
    Ok(())
}
