//! Private devnet adapter reusing the process-test v2 capture implementation.

use anyhow::{Context, Result, ensure};
use eez_testkit::ProverProxyHandle;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: capture_prover LISTEN_ADDRESS UPSTREAM_URL CAPTURE_DIRECTORY"
    );
    let proxy = ProverProxyHandle::capture_on(
        args[0].parse().context("listen address")?,
        args[1].clone(),
        args[2].clone().into(),
    )
    .await?;
    println!("v2 test capture listening at {}", proxy.endpoint());
    tokio::signal::ctrl_c().await?;
    Ok(())
}
