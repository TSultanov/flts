use dbsync::{Arena, ColumnFile, ColumnFileWriter, Result, WriterId};
use std::time::Instant;

fn main() -> Result<()> {
    let chunks = std::env::var("BENCH_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let rounds = std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let rows = chunks * 8192;
    let block_bytes: usize = std::env::var("BENCH_BLOCK_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(65536);
    println!("workload,initial_bytes,added_bytes,commit_us,read_us");
    for workload in ["sparse", "repeated", "dense", "append", "reverted"] {
        let path =
            std::env::temp_dir().join(format!("dbsync-bench-{}-{workload}", std::process::id()));
        let writer = WriterId::from_raw(1);
        let mut data = Arena::<u64>::with_chunk_rows(block_bytes / 8);
        data.extend_from_slice(&vec![0; rows])?;
        let mut out = ColumnFileWriter::create(&path, writer)?;
        out.column("values", &data)?;
        out.finish()?;
        let initial = std::fs::metadata(&path)?.len();
        let mut file = ColumnFile::open(&path)?;
        let start = Instant::now();
        for round in 0..rounds {
            let mut data = file.column::<u64>("values")?;
            match workload {
                "sparse" => data.set((round * 7919) % rows, round as u64 + 1)?,
                "repeated" => data.set(0, round as u64 + 1)?,
                "dense" => {
                    for row in 0..8192 {
                        data.set(row, round as u64 + 1)?;
                    }
                }
                "append" => {
                    data.push(round as u64)?;
                }
                "reverted" => {
                    data.set(0, 1)?;
                    data.set(0, 0)?;
                }
                _ => unreachable!(),
            }
            let mut update = file.update(writer)?;
            update.column("values", &data)?;
            file = update.commit_file()?;
        }
        let elapsed = start.elapsed().as_micros();
        let size = std::fs::metadata(&path)?.len();
        let start = Instant::now();
        file.verify()?;
        let read = start.elapsed().as_micros();
        println!("{workload},{initial},{},{elapsed},{read}", size - initial);
        std::fs::remove_file(path)?;
    }
    Ok(())
}
