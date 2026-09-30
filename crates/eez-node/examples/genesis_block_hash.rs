fn main() -> eyre::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| eyre::eyre!("usage: genesis_block_hash <genesis.json>"))?;
    let genesis = eez_l1::chain_config::load_l2_genesis(std::path::Path::new(&path))?;
    let chain_spec = eez_evm::profile::chain_spec(genesis.into_genesis())?;
    println!("{}", chain_spec.genesis_header().hash_slow());
    Ok(())
}
