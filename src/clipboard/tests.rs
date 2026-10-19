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
