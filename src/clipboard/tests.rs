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
    assert!(
        failing
            .get(|| {
                tries += 1;
                Err("no seat".into())
            })
            .is_none()
    );
    assert!(
        failing
            .get(|| {
                tries += 1;
                Ok(1)
            })
            .is_none(),
        "once failed, it stays failed"
    );
    assert_eq!(tries, 1);
    let mut working: Backend<u8> = Backend::Idle;
    assert_eq!(working.get(|| Ok(7)), Some(&7));
    assert_eq!(working.get(|| panic!("started again")), Some(&7));
}
