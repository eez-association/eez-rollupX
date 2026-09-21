//! Interactive public-API fixture for the isolated physical-volume drill.
//! No protocol runtime and no durability/performance claim is made here.
use eez_telemetry::{
    SpoolConfig, Writer, reader,
    schema::{BlockRef, Event, Hash32, Identity, Record},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
    time::{Duration, Instant},
};

fn descriptors(directory: &Path) -> io::Result<usize> {
    Ok(fs::read_dir("/proc/self/fd")?
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter(|path| path.starts_with(directory))
        .count())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .ok_or("expected private spool directory")?;
    let directory = Path::new(&directory);
    let config: SpoolConfig = serde_json::from_value(json!({"directory":directory,
        "segment_bytes":65536,"retained_bytes":131072}))?;
    let (publisher, writer) = Writer::start(
        Identity {
            environment: "fixture".into(),
            deployment: "physical-spool".into(),
            service: "composer".into(),
            instance: "one".into(),
        },
        config,
    )?;
    let mut writer = Some(writer);
    println!("{}", json!({"ready":true}));
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let command: Value = serde_json::from_str(&line?)?;
        let response = match command["command"].as_str().ok_or("missing command")? {
            "publish" => {
                let count = command["count"]
                    .as_u64()
                    .filter(|n| *n <= 10000)
                    .ok_or("count outside fixture bound")?;
                let started = Instant::now();
                let mut accepted = 0u64;
                for _ in 0..count {
                    let ticket = publisher.capture().ok_or("writer stopped accepting")?;
                    accepted += u64::from(publisher.publish(
                        ticket,
                        Event::Reorg {
                            details: None,
                            ancestor: BlockRef {
                                number: 12,
                                hash: Hash32([7; 32]),
                            },
                        },
                        None,
                    ));
                }
                json!({"accepted":accepted,"calls":count,"elapsed_ns":started.elapsed().as_nanos().to_string(),"stats":publisher.stats()})
            }
            "stats" => json!({"stats":publisher.stats()}),
            "read" => {
                let before = descriptors(directory)?;
                let mut lines = 0u64;
                let mut controls = Vec::new();
                for id in reader::inventory(directory)? {
                    // Deliberately lag: read just one bounded pass per segment,
                    // retaining returned buffers while checking descriptor release.
                    let batch = reader::read_segment(directory, id, 0)?;
                    for line in &batch.lines {
                        if let Ok(Record::Control { stats, .. }) =
                            serde_json::from_slice(&line.bytes)
                        {
                            controls.push(stats);
                        }
                    }
                    lines += u64::try_from(batch.lines.len())?;
                    if descriptors(directory)? != before {
                        return Err("reader retained descriptor after bounded pass".into());
                    }
                    drop(batch);
                }
                json!({"descriptors_before":before,"descriptors_after":descriptors(directory)?,"lines":lines,"controls":controls})
            }
            "shutdown" => {
                json!({"drained":writer.take().ok_or("writer already stopped")?.shutdown(Duration::from_secs(8)),"stats":publisher.stats()})
            }
            _ => return Err("unknown fixture command".into()),
        };
        println!("{response}");
        io::stdout().flush()?;
    }
    if let Some(writer) = writer {
        writer.shutdown(Duration::from_secs(8));
    }
    Ok(())
}
