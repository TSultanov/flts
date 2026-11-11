//! Whether some bytes are anywhere in a directory, under compression or not.

use std::path::{Path, PathBuf};

const FRAME: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const LARGEST: usize = 1 << 28;

fn within(held: &[u8], sought: &[u8]) -> bool {
    held.windows(sought.len()).any(|window| window == sought)
}

// Every zstd frame is opened wherever it starts, so bytes a commit no longer points at count too.
fn holds_bytes(bytes: &[u8], sought: &[u8]) -> bool {
    if within(bytes, sought) {
        return true;
    }
    for at in 0..bytes.len().saturating_sub(FRAME.len()) {
        if bytes[at..at + FRAME.len()] != FRAME {
            continue;
        }
        let Ok(len) = zstd::zstd_safe::find_frame_compressed_size(&bytes[at..]) else {
            continue;
        };
        if zstd::bulk::decompress(&bytes[at..at + len], LARGEST)
            .is_ok_and(|opened| within(&opened, sought))
        {
            return true;
        }
    }
    false
}

pub fn files_holding(dir: &Path, sought: &[u8]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else if holds_bytes(&std::fs::read(&path).unwrap(), sought) {
                found.push(path.strip_prefix(dir).unwrap().to_owned());
            }
        }
    }
    found.sort();
    found
}
