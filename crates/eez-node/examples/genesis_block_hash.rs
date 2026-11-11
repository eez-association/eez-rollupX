fn main() -> eyre::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or_else(|| {
        eyre::eyre!("usage: genesis_block_hash <genesis.json> [--network-profile <L1 profile>]")
    })?;
    let genesis = eez_l1::chain_config::load_l2_genesis(std::path::Path::new(&path))?;
    let chain_spec = eez_evm::profile::chain_spec(genesis.into_genesis())?;
    match args.next().as_deref() {
        None => println!("{}", chain_spec.genesis_header().hash_slow()),
        Some("--network-profile") => {
            let l1_path = args
                .next()
                .ok_or_else(|| eyre::eyre!("missing L1 profile"))?;
            eyre::ensure!(args.next().is_none(), "unexpected genesis tool argument");
            let l1 = eez_l1::chain_config::load_l1_chain_spec(std::path::Path::new(&l1_path))?;
            let rules = eez_l1::chain_config::offline_l1_rules(&l1)?;
            let profile = rules.profile(
                eez_evm::profile::schedule(&chain_spec)?,
                chain_spec.chain().id(),
            )?;
            println!("{}", profile.commitment());
        }
        Some(_) => eyre::bail!("unexpected genesis tool argument"),
    }
    Ok(())
}
