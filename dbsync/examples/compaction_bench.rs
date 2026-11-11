use dbsync::{Arena, ColumnFile, ColumnFileWriter, Result, WriterId};
use std::time::Instant;

fn main() -> Result<()> {
    println!("live_kib,rewrites,before_bytes,after_bytes,reclaimed_bytes,compact_us");
    for kib in [64usize, 256, 1024, 8192] {
        for multiple in [1, 2, 4] {
            let path =
                std::env::temp_dir().join(format!("dbsync-compact-bench-{}", std::process::id()));
            let writer = WriterId::from_raw(1);
            let mut data = Arena::<u64>::new();
            data.extend_from_slice(&vec![0; kib * 128])?;
            let chunks = data.chunk_count();
            let mut out = ColumnFileWriter::create(&path, writer)?;
            out.column("values", &data)?;
            out.finish()?;
            let mut file = ColumnFile::open(&path)?;
            for round in 0..chunks * multiple / 2 {
                let mut data = file.column::<u64>("values")?;
                let chunk = round % chunks;
                for row in chunk * 8192..((chunk + 1) * 8192).min(data.len()) {
                    data.set(row, round as u64 + 1)?;
                }
                let mut updater = file.update(writer)?;
                updater.column("values", &data)?;
                file = updater.commit_file()?;
            }
            let start = Instant::now();
            let (packed, report) = file.compact()?;
            let elapsed = start.elapsed().as_micros();
            println!(
                "{kib},{multiple},{},{},{},{elapsed}",
                report.before_bytes,
                report.after_bytes,
                report.reclaimed_bytes()
            );
            packed.verify()?;
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}
