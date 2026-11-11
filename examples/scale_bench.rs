//! How open, commit and merge cost grow with the rows a book-shaped store holds.

use dbsync::schema::Store;
use dbsync::{CompactionPolicy, Result, WriterId};
use std::time::Instant;

#[dbsync::entity]
struct Book {
    title: String,
    paragraphs: Many<Paragraph>,
}

#[dbsync::entity]
struct Paragraph {
    book: One<Book>,
    #[order(book)]
    number: u32,
    text: String,
    translation: Option<String>,
    words: Many<Word>,
}

#[dbsync::entity]
struct Word {
    paragraph: One<Paragraph>,
    #[order(paragraph)]
    number: u32,
    original: String,
    translation: String,
    note: String,
}

#[dbsync::container(schema = "scale", version = 1)]
struct Db {
    books: Set<Book>,
    paragraphs: Set<Paragraph>,
    words: Set<Word>,
}

fn copy_store(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_store(&entry.path(), &target);
        } else if entry.file_name() != "lock" {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn bytes_in(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                bytes_in(&e.path())
            } else {
                e.metadata().unwrap().len()
            }
        })
        .sum()
}
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() -> Result<()> {
    let w = WriterId::from_raw(1);
    println!(
        "layout,paragraphs,words,build_ms,first_commit_ms,file_kib,open_ms,walk_ms,edit_commit_ms_avg,translate_commit_ms_avg,growth_kib_per_commit,merge_ms,snapshot_ms,snapshot_edit_read_ms,remove_ms"
    );
    for (paras, words_per) in [
        (500usize, 0usize),
        (2000, 0),
        (8000, 0),
        (500, 20),
        (2000, 20),
        (4000, 20),
    ] {
        let path = std::env::temp_dir().join(format!(
            "dbsync-scale-{}-{paras}-{words_per}",
            std::process::id()
        ));
        let peer_path = path.with_extension("peer");
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_dir_all(&peer_path);
        let mut db = Db::create(&path, w)?;
        db.set_compaction_policy(CompactionPolicy::Disabled)?;
        let t = Instant::now();
        let book = db.books.push("A book")?;
        let mut ids = Vec::new();
        let mut word_ids = Vec::new();
        for n in 0..paras {
            let text = format!("Paragraph {n} {}", "lorem ipsum dolor sit amet ".repeat(12));
            let p = db.paragraphs.push(book, n as u32, &text, None)?;
            ids.push(p);
            for k in 0..words_per {
                word_ids.push(db.words.push(
                    p,
                    k as u32,
                    &format!("word{k}"),
                    &format!("слово{k}"),
                    "noun, nominative, a slightly longer grammar note",
                )?);
            }
        }
        let build = ms(t);
        let t = Instant::now();
        db.commit(w)?;
        let first = ms(t);
        let size0 = bytes_in(&path);
        copy_store(&path, &peer_path);
        let t = Instant::now();
        let cold = Db::open(&path)?;
        let open = ms(t);
        let t = Instant::now();
        let mut n = 0usize;
        for p in cold.at(book)?.paragraphs()? {
            n += p.text()?.len();
        }
        let walk = ms(t);
        assert!(n > 0);
        let rounds = 5;
        let t = Instant::now();
        for r in 0..rounds {
            db.paragraphs
                .set_number(ids[r * 7 % paras], (paras + r) as u32)?;
            db.commit(w)?;
        }
        let edit = ms(t) / rounds as f64;
        let before = bytes_in(&path);
        let t = Instant::now();
        for r in 0..rounds {
            db.paragraphs
                .set_translation(ids[r * 11 % paras], Some(&"перевод абзаца ".repeat(12)))?;
            db.commit(w)?;
        }
        let tr = ms(t) / rounds as f64;
        let growth = (bytes_in(&path) - before) as f64 / 1024.0 / rounds as f64;
        let mut peer = Db::open(&peer_path)?;
        peer.paragraphs
            .set_translation(ids[3], Some("peer translation"))?;
        peer.commit(WriterId::from_raw(2))?;
        let t = Instant::now();
        db.merge_from(&peer_path, w)?;
        let merge = ms(t);
        let t = Instant::now();
        let mut kept = Vec::new();
        for _ in 0..20 {
            kept.push(db.snapshot());
        }
        let snapshot = ms(t) / 20.0;
        drop(kept);
        let t = Instant::now();
        for (n, id) in ids.iter().take(20).enumerate() {
            let held = db.snapshot();
            db.paragraphs.set_number(*id, (paras + 100 + n) as u32)?;
            let _ = db.at(*id)?.number()?;
            drop(held);
        }
        let touched = ms(t) / 20.0;
        let t = Instant::now();
        for id in ids.iter().skip(30).take(10) {
            db.remove(*id)?;
        }
        let removed = ms(t) / 10.0;
        println!(
            "shards,{paras},{},{build:.0},{first:.0},{},{open:.0},{walk:.0},{edit:.0},{tr:.0},{growth:.1},{merge:.0},{snapshot:.2},{touched:.2},{removed:.2}",
            paras * words_per,
            size0 / 1024
        );
        let _ = word_ids;
        std::fs::remove_dir_all(&path)?;
        std::fs::remove_dir_all(&peer_path)?;
    }
    Ok(())
}
