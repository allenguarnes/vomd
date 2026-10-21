use super::*;

#[test]
fn string_is_latin1_with_question_marks_for_the_rest() {
    assert_eq!(latin1("plain"), b"plain");
    assert_eq!(latin1("caf\u{e9} \u{ff}"), b"caf\xe9 \xff");
    assert_eq!(
        latin1("a\u{100}b"),
        b"a?b",
        "the first character past Latin-1"
    );
    assert_eq!(
        latin1("日本 🙂"),
        b"?? ?",
        "one byte for each character, not for each byte"
    );
    assert_eq!(latin1(""), b"");
}

#[test]
fn a_backend_starts_once_and_one_that_cannot_isnt_tried_again() {
    // (A compositor with no seat, or no libxcb: the copy does nothing, and says so once.)
    let mut tries = 0;
    let mut failing: Backend<u8> = Backend::Idle;
    assert_eq!(
        failing.get(|| {
            tries += 1;
            Err("no seat".into())
        }),
        Err("no seat")
    );
    assert_eq!(
        failing.get(|| {
            tries += 1;
            Ok(1)
        }),
        Err("no seat"),
        "once failed, it stays failed, and says why each time"
    );
    assert_eq!(tries, 1);
    let mut working: Backend<u8> = Backend::Idle;
    assert_eq!(working.get(|| Ok(7)), Ok(&7));
    assert_eq!(working.get(|| panic!("started again")), Ok(&7));
}

#[test]
fn a_display_with_no_clipboard_says_why_a_copy_did_nothing() {
    // (Not a real display: nothing here connects to the desktop this runs on.)
    use winit::raw_window_handle::AndroidDisplayHandle;
    let mut clipboard = Clipboard::new(RawDisplayHandle::Android(AndroidDisplayHandle::new()));
    let why = clipboard
        .set(Which::Clipboard, Content::Text("text".into()))
        .expect_err("no clipboard to copy to");
    assert_eq!(why, "this display has no clipboard support");
}

#[test]
fn the_bytes_of_a_selection_are_text_as_its_type_says() {
    assert_eq!(decode("héllo 日本".as_bytes(), false), "héllo 日本");
    assert_eq!(decode(b"caf\xe9 \xff", true), "caf\u{e9} \u{ff}");
    // Bad UTF-8, and a character cut off by the cap, show as the replacement character.
    assert_eq!(decode(b"a\xffb", false), "a\u{fffd}b");
    assert_eq!(decode(&"日".as_bytes()[..2], false), "\u{fffd}");
    assert_eq!(decode(b"", false), "");
}

/// What a callback was called with, in order.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>>;

/// A callback that records what it was called with, and how many times.
fn recorder() -> (impl FnOnce(Option<String>) + Send + 'static, Seen) {
    let seen = Seen::default();
    let mine = seen.clone();
    (move |text| mine.lock().unwrap().push(text), seen)
}

#[test]
fn a_reply_says_something_once_and_nothing_if_it_is_dropped() {
    let (done, seen) = recorder();
    Reply::new(done).send(Some("text".into()));
    assert_eq!(*seen.lock().unwrap(), [Some("text".to_owned())]);
    // Dropped unanswered: nothing.
    let (done, seen) = recorder();
    drop(Reply::new(done));
    assert_eq!(*seen.lock().unwrap(), [None]);
    // Empty text is no text.
    let (done, seen) = recorder();
    Reply::new(done).send(Some(String::new()));
    assert_eq!(*seen.lock().unwrap(), [None]);
}

#[test]
fn a_read_on_a_display_with_no_clipboard_finds_nothing_and_leaves_the_next_free() {
    use winit::raw_window_handle::AndroidDisplayHandle;
    let mut clipboard = Clipboard::new(RawDisplayHandle::Android(AndroidDisplayHandle::new()));
    for which in [Which::Clipboard, Which::Primary, Which::Clipboard] {
        let (done, seen) = recorder();
        clipboard.read(which, done);
        assert_eq!(*seen.lock().unwrap(), [None], "{which:?}");
    }
    // One read at a time: while one is under way another finds nothing, at once.
    clipboard.reading.store(true, Ordering::Release);
    let (done, seen) = recorder();
    clipboard.read(Which::Clipboard, done);
    assert_eq!(*seen.lock().unwrap(), [None]);
    assert!(
        clipboard.reading.load(Ordering::Acquire),
        "the one under way is still"
    );
}
