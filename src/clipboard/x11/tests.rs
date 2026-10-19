use super::*;

const WIN: Window = 0x4000_0001;
const PROP: Atom = 300;

fn transfer(content: &Content, latin1: bool, now: Instant) -> Transfer {
    Transfer {
        chunks: content.chunks(),
        ty: 77,
        latin1,
        since: now,
    }
}

/// Everything a requestor gets: a piece for each deletion, up to the empty one.
fn drain(transfers: &mut Transfers, now: Instant) -> Vec<Vec<u8>> {
    let mut pieces = Vec::new();
    while let Some((ty, bytes)) = transfers.next(WIN, PROP, now) {
        assert_eq!(ty, 77);
        let end = bytes.is_empty();
        pieces.push(bytes);
        if end {
            break;
        }
    }
    pieces
}

#[test]
fn an_incr_transfer_gives_a_piece_for_each_deletion_and_ends_with_an_empty_one() {
    let now = Instant::now();
    let text = "x".repeat(3 * CHUNK + 5);
    let content = Content::Text(text.as_str().into());
    let mut transfers = Transfers::default();
    assert!(transfers.is_empty());
    transfers.start(WIN, PROP, transfer(&content, false, now));
    assert!(!transfers.is_empty());
    let pieces = drain(&mut transfers, now);
    let sizes: Vec<usize> = pieces.iter().map(Vec::len).collect();
    assert_eq!(sizes, [CHUNK, CHUNK, CHUNK, 5, 0]);
    assert_eq!(pieces.concat(), text.as_bytes());
    // The empty piece closed it: another deletion finds nothing waiting.
    assert!(transfers.is_empty());
    assert_eq!(transfers.next(WIN, PROP, now), None);
}

#[test]
fn a_string_transfer_is_converted_a_piece_at_a_time() {
    let now = Instant::now();
    let text = "é日".repeat(CHUNK);
    let content = Content::Text(text.as_str().into());
    let mut transfers = Transfers::default();
    transfers.start(WIN, PROP, transfer(&content, true, now));
    let pieces = drain(&mut transfers, now);
    // One byte for each character, whichever piece it is in, and none split.
    let bytes = pieces.concat();
    assert_eq!(bytes.len(), 2 * CHUNK);
    assert!(bytes.chunks(2).all(|c| c == [0xe9, b'?']));
    assert!(pieces.len() > 2 && pieces.iter().all(|p| p.len() <= CHUNK));
}

#[test]
fn transfers_are_kept_apart_by_window_and_property() {
    let now = Instant::now();
    let a = Content::Text("a".repeat(CHUNK + 1).into());
    let b = Content::Text("b".repeat(CHUNK + 1).into());
    let mut transfers = Transfers::default();
    transfers.start(WIN, PROP, transfer(&a, false, now));
    transfers.start(WIN, PROP + 1, transfer(&b, false, now));
    transfers.start(WIN + 1, PROP, transfer(&b, false, now));
    assert_eq!(transfers.next(WIN, PROP, now).unwrap().1[0], b'a');
    assert_eq!(transfers.next(WIN, PROP + 1, now).unwrap().1[0], b'b');
    assert_eq!(transfers.next(WIN + 2, PROP, now), None);
    // A requestor that is destroyed takes its transfers with it, and nobody else's.
    transfers.closed(WIN);
    assert_eq!(transfers.next(WIN, PROP, now), None);
    assert_eq!(transfers.next(WIN, PROP + 1, now), None);
    assert_eq!(transfers.next(WIN + 1, PROP, now).unwrap().1[0], b'b');
}

#[test]
fn a_requestor_that_stops_taking_pieces_is_given_up_on() {
    let t0 = Instant::now();
    let content = Content::Text("z".repeat(2 * CHUNK).into());
    let mut transfers = Transfers::default();
    transfers.start(WIN, PROP, transfer(&content, false, t0));
    transfers.start(WIN, PROP + 1, transfer(&content, false, t0));
    let s = Duration::from_secs;
    assert_eq!(transfers.expire(t0 + s(4)), 0, "not yet");
    // One takes a piece (its clock starts again), the other stalls.
    assert!(transfers.next(WIN, PROP, t0 + s(4)).is_some());
    assert_eq!(transfers.expire(t0 + s(6)), 1, "5 s without a deletion");
    assert!(transfers.next(WIN, PROP + 1, t0 + s(6)).is_none());
    assert!(transfers.next(WIN, PROP, t0 + s(6)).is_some());
    assert_eq!(transfers.expire(t0 + s(10)), 0, "it took one at 6 s");
    assert_eq!(transfers.expire(t0 + s(12)), 1);
    assert!(transfers.is_empty());
}

#[test]
fn a_selection_is_answered_with_the_targets_that_serve_text() {
    let a = Atoms {
        CLIPBOARD: 1,
        TARGETS: 2,
        TIMESTAMP: 3,
        MULTIPLE: 4,
        SAVE_TARGETS: 5,
        INCR: 6,
        UTF8_STRING: 7,
        TEXT: 8,
        ATOM_PAIR: 9,
        CLIPBOARD_MANAGER: 10,
        TEXT_PLAIN_UTF8: 11,
        TEXT_PLAIN: 12,
        VOMD_SELECTION: 13,
    };
    let list = targets(&a);
    for t in [
        a.TARGETS,
        a.TIMESTAMP,
        a.MULTIPLE,
        a.SAVE_TARGETS,
        a.UTF8_STRING,
        a.TEXT_PLAIN_UTF8,
        a.TEXT,
        AtomEnum::STRING.into(),
    ] {
        assert!(list.contains(&t), "{t}");
    }
    let mut unique = list.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), list.len());
}
