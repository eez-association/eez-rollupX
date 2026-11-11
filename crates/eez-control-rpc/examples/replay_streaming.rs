//! Wire-only replay against a separately configured RollupX fixture prover.
#[path = "../conformance/replay.rs"]
mod replay;

#[tokio::main]
async fn main() -> Result<(), replay::Error> {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().ok_or("expected URL and vector JSON path")?;
    let path = args.next().ok_or("expected vector JSON path")?;
    let identity = args
        .next()
        .ok_or("expected independently configured validation identity")?
        .parse()?;
    if args.next().is_some() {
        return Err("usage: replay_streaming URL VECTOR.json VALIDATION_IDENTITY".into());
    }
    let vector: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    println!(
        "Required fixture/profile: {} / {}",
        vector["fixture"], vector["server_profile"]
    );
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        replay::replay(&vector, &endpoint, identity),
    )
    .await??;
    println!("PASS {}", vector["case"]);
    Ok(())
}
