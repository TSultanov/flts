use super::*;
use crate::test_utils::TempDir;

fn writer(raw: u64) -> WriterId {
    WriterId::from_raw(raw)
}

fn seen(dots: &[(u64, u64)]) -> VersionVector {
    VersionVector::from_dots(
        &dots
            .iter()
            .map(|(raw, counter)| Dot::new(writer(*raw), *counter))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn what_a_writer_published_is_what_another_reads() {
    let dir = TempDir::new("peers_round_trip");
    let ours = Peer {
        writer: writer(0xa),
        seen: seen(&[(0xa, 7), (0xb, 3)]),
    };
    publish(&dir.path, &ours).unwrap();

    assert_eq!(read(&dir.path, writer(0xa)).unwrap(), Some(ours.clone()));
    assert_eq!(read(&dir.path, writer(0xb)).unwrap(), None);
    assert_eq!(read_all(&dir.path).unwrap(), [ours]);
}

#[test]
fn publishing_again_replaces_what_was_said_before() {
    let dir = TempDir::new("peers_replace");
    let mut ours = Peer {
        writer: writer(0xa),
        seen: seen(&[(0xa, 1)]),
    };
    publish(&dir.path, &ours).unwrap();
    ours.seen = seen(&[(0xa, 9), (0xb, 4)]);
    publish(&dir.path, &ours).unwrap();

    assert_eq!(read(&dir.path, writer(0xa)).unwrap(), Some(ours));
}

#[test]
fn files_that_are_not_a_writers_are_passed_over() {
    let dir = TempDir::new("peers_strangers");
    let ours = Peer {
        writer: writer(0xa),
        seen: seen(&[(0xa, 1)]),
    };
    publish(&dir.path, &ours).unwrap();
    let folder = dir.path.join(FOLDER);
    std::fs::write(folder.join("000000000000000b.dbs"), b"cut short").unwrap();
    std::fs::write(folder.join("000000000000000a.sync-conflict.dbs"), b"junk").unwrap();
    std::fs::write(folder.join("notes.txt"), b"junk").unwrap();

    assert_eq!(read_all(&dir.path).unwrap(), [ours]);
    assert!(read_all(&dir.path.join("nowhere")).unwrap().is_empty());
}
