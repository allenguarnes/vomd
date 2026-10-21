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
        VOMD_PASTE: 14,
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

fn reading() -> Reading {
    Reading {
        reply: Some(Reply::new(|_| {})),
        selection: 1,
        time: 1000,
        target: 2,
        asked: 1,
        deadline: Instant::now() + READ_WAIT,
        incr: false,
        ended: false,
        draining: false,
        data: Vec::new(),
        latin1: false,
    }
}

#[test]
fn only_the_answer_to_this_request_is_taken_for_it() {
    let r = reading();
    assert!(r.answered_by(1, 2, 1000));
    // A late answer to a paste that timed out had the time of that one.
    assert!(!r.answered_by(1, 2, 400));
    assert!(!r.answered_by(1, 3, 1000));
    assert!(!r.answered_by(3, 2, 1000));
}

#[test]
fn the_pieces_of_a_paste_are_joined_and_the_empty_one_ends_it() {
    let mut r = reading();
    assert!(!r.push(b"hello "));
    assert!(!r.push("wörld".as_bytes()));
    assert!(r.push(b""));
    assert_eq!(decode(&r.data, false), "hello wörld");
}

#[test]
fn a_paste_is_cut_at_the_cap_whatever_the_size_of_the_pieces() {
    let mut r = reading();
    let piece = vec![b'x'; CHUNK];
    let mut taken = 0;
    while !r.push(&piece) {
        taken += 1;
        assert!(taken < 100, "it never ends");
    }
    assert_eq!(r.data.len(), READ_CAP);
    // One piece that is too big alone.
    let mut r = reading();
    assert!(r.push(&vec![b'y'; 3 * READ_CAP]));
    assert_eq!(r.data.len(), READ_CAP);
    // And anything after the cap adds nothing.
    assert!(r.push(b"more"));
    assert_eq!(r.data.len(), READ_CAP);
}

#[test]
fn a_property_that_a_paste_gave_up_on_is_left_alone_until_it_has_been_quiet() {
    let t0 = Instant::now();
    let mut names = Pastes::new(10);
    assert_eq!(names.take(t0), Some(10));
    names.retire(10, t0);
    assert!(names.is_retired(10));
    // No name left to use but the room for more.
    assert_eq!(names.take(t0), None);
    assert!(names.room());
    names.add(11);
    assert_eq!(names.take(t0), Some(11));
    // A late write to the retired one starts the quiet over.
    let t1 = t0 + QUIET - Duration::from_secs(1);
    names.retire(10, t1);
    names.retire(11, t1);
    assert_eq!(names.take(t0 + QUIET), None);
    assert_eq!(names.take(t1 + QUIET), Some(10));
    // Back in use, no longer retired.
    assert!(!names.is_retired(10));
}

#[test]
fn only_a_few_names_are_made_and_a_paste_finds_none_when_all_are_retired() {
    let t0 = Instant::now();
    let mut names = Pastes::new(1);
    for atom in 2..=PASTE_NAMES as Atom {
        assert!(names.room());
        names.add(atom);
    }
    assert!(!names.room());
    for atom in 1..=PASTE_NAMES as Atom {
        assert_eq!(names.take(t0), Some(atom));
        names.retire(atom, t0);
    }
    assert_eq!(names.take(t0 + Duration::from_secs(1)), None);
    assert_eq!(names.next_name(), format!("VOMD_PASTE_{PASTE_NAMES}"));
    assert!(names.has(2) && !names.has(99));
}
