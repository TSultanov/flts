use super::*;

fn token(writer_id: WriterId, generation: u64) -> CommitToken {
    CommitToken {
        generation,
        content_id: 0xc0ffee,
        writer_id,
        file_uuid: 0xfeed,
    }
}

#[test]
fn our_own_commit_is_an_echo() {
    let mine = WriterId::from_device_id("mine");
    assert!(token(mine, 4).is_echo_of(mine, 4));
    assert!(
        token(mine, 3).is_echo_of(mine, 4),
        "a stale event is still ours"
    );
}

#[test]
fn a_peers_commit_is_never_an_echo() {
    let mine = WriterId::from_device_id("mine");
    let peer = WriterId::from_device_id("peer");
    assert!(!token(peer, 4).is_echo_of(mine, 4));
    assert!(!token(peer, 1).is_echo_of(mine, 4));
}

#[test]
fn our_own_id_on_a_newer_generation_is_not_an_echo() {
    let mine = WriterId::from_device_id("mine");
    assert!(
        !token(mine, 5).is_echo_of(mine, 4),
        "a generation we have not written yet came from elsewhere"
    );
}
