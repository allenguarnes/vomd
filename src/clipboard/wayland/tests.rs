use std::io::Read;

use super::*;
use crate::copy::CHUNK;
use crate::doc::{BlockKind, Doc};

/// What a program reading the other end of the pipe gets.
fn pasted(content: &Content, as_latin1: bool) -> Vec<u8> {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let content = content.clone();
    // The pipe holds 64 KiB: the writer waits for the reader, as it does for a program.
    let thread = std::thread::spawn(move || write_to(OwnedFd::from(writer), &content, as_latin1));
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    thread.join().unwrap();
    bytes
}

#[test]
fn a_paste_is_the_text_in_every_piece() {
    let text = "word ".repeat(3 * CHUNK / 5 + 7);
    let content = Content::Text(text.as_str().into());
    assert!(text.len() > 2 * CHUNK);
    assert_eq!(pasted(&content, false), text.as_bytes());
    assert_eq!(pasted(&Content::Text("".into()), false), b"");
}

#[test]
fn a_string_paste_is_latin1() {
    let content = Content::Text("caf\u{e9} \u{65e5}".into());
    assert_eq!(pasted(&content, true), b"caf\xe9 ?");
    assert_eq!(pasted(&content, false), "caf\u{e9} \u{65e5}".as_bytes());
}

#[test]
fn a_program_that_closes_its_end_stops_the_writing() {
    // A long text and a pipe closed after the first piece: the writer ends instead of
    // failing, or waiting for ever.
    let content = Content::Text("x".repeat(20 * CHUNK).into());
    let (mut reader, writer) = std::io::pipe().unwrap();
    let thread = std::thread::spawn(move || write_to(OwnedFd::from(writer), &content, false));
    let mut first = vec![0; 100];
    reader.read_exact(&mut first).unwrap();
    drop(reader);
    thread.join().unwrap();
}

#[test]
fn a_program_that_never_reads_is_given_up_on() {
    // The read end stays open and is never read: the pipe fills and the writer stops waiting.
    let content = Content::Text("x".repeat(20 * CHUNK).into());
    let (reader, writer) = std::io::pipe().unwrap();
    let start = Instant::now();
    let thread = std::thread::spawn(move || {
        write_within(
            OwnedFd::from(writer),
            &content,
            false,
            Duration::from_millis(200),
        );
    });
    thread.join().unwrap();
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(200), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    drop(reader);
}

#[test]
fn a_slow_reader_that_keeps_taking_bytes_is_waited_for() {
    // Each wait is shorter than the limit, though the whole takes longer.
    let text = "y".repeat(10 * CHUNK);
    let content = Content::Text(text.as_str().into());
    let (mut reader, writer) = std::io::pipe().unwrap();
    let thread = std::thread::spawn(move || {
        write_within(
            OwnedFd::from(writer),
            &content,
            false,
            Duration::from_millis(500),
        );
    });
    let mut got = Vec::new();
    let mut buf = vec![0; 32 * 1024];
    while got.len() < text.len() {
        std::thread::sleep(Duration::from_millis(100));
        let n = reader.read(&mut buf).unwrap();
        assert!(n > 0);
        got.extend_from_slice(&buf[..n]);
    }
    thread.join().unwrap();
    assert_eq!(got, text.as_bytes());
}

#[test]
fn only_a_few_pastes_are_written_at_once() {
    let count = Arc::new(AtomicUsize::new(0));
    let writers: Vec<Writer> = (0..WRITERS)
        .map(|_| Writer::start(&count).expect("room"))
        .collect();
    assert!(
        Writer::start(&count).is_none(),
        "the one past the last is refused"
    );
    assert_eq!(
        count.load(Ordering::Acquire),
        WRITERS,
        "a refusal takes no room"
    );
    drop(writers);
    assert_eq!(count.load(Ordering::Acquire), 0);
    assert!(Writer::start(&count).is_some());
}

/// What a program pastes after a click on the copy button of the first code block of `src`.
fn pasted_code(src: &str) -> Vec<u8> {
    let doc = Arc::new(Doc::parse(src));
    let first = doc
        .blocks
        .iter()
        .position(|b| b.kind == BlockKind::Code)
        .expect("a code block") as u32;
    let last = crate::select::block_unit(&doc, first).1.block;
    pasted(&Content::code(&doc, first, last), false)
}

#[test]
fn a_pasted_code_block_is_its_code_to_the_byte() {
    // (A headless sway has no pointer to click the button with, so the path from the button
    // to a pasting program is checked here: the copy it makes, through the pipe's writer.)
    let code = "fn f() {\n\treturn 1;  // tab\n\n    deep   spaced \u{e9}\n}";
    let quoted: String = code.lines().map(|l| format!("> {l}\n")).collect();
    assert_eq!(
        pasted_code(&format!("```rust\n{code}\n```\n")),
        code.as_bytes()
    );
    assert_eq!(
        pasted_code(&format!("> ```\n{quoted}> ```\n")),
        code.as_bytes()
    );
    // Several MB, made as the pipe is read.
    let big: String = (0..150_000).map(|i| format!("line {i}\n")).collect();
    let big = big.trim_end_matches('\n');
    assert!(big.len() > 1_000_000);
    let got = pasted_code(&format!("```\n{big}\n```\n"));
    assert!(
        got == big.as_bytes(),
        "{} bytes, not {}",
        got.len(),
        big.len()
    );
}

#[test]
fn a_selection_read_through_a_pipe_is_all_of_it_up_to_the_end() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let thread = std::thread::spawn(move || {
        writer.write_all("héllo\nwörld".as_bytes()).unwrap();
    });
    let bytes = read_within(reader, READ_CAP, Duration::from_secs(5)).expect("the pipe ended");
    thread.join().unwrap();
    assert_eq!(decode(&bytes, false), "héllo\nwörld");
    // An empty answer is empty, not a failure.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(writer);
    assert_eq!(
        read_within(reader, READ_CAP, Duration::from_secs(5)),
        Some(Vec::new())
    );
}

#[test]
fn a_long_selection_is_cut_at_the_cap_whatever_the_writer_does() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    // More than the cap, written for as long as someone reads (the writer then fails).
    let thread = std::thread::spawn(move || {
        let piece = [b'x'; 4096];
        while writer.write_all(&piece).is_ok() {}
    });
    let bytes = read_within(reader, 10_000, Duration::from_secs(5)).expect("the cap was reached");
    assert_eq!(bytes.len(), 10_000);
    thread.join().unwrap();
}

#[test]
fn an_owner_that_holds_the_pipe_and_says_nothing_is_given_up_on() {
    let (reader, writer) = std::io::pipe().unwrap();
    let start = Instant::now();
    assert_eq!(
        read_within(reader, READ_CAP, Duration::from_millis(200)),
        None
    );
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(200) && took < Duration::from_secs(5));
    drop(writer);
    // One that writes some and then stalls is given up on as well: its half text is no text.
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"half").unwrap();
    assert_eq!(
        read_within(reader, READ_CAP, Duration::from_millis(200)),
        None
    );
    drop(writer);
}

#[test]
fn the_types_a_paste_is_asked_for_come_in_order_of_preference() {
    let pick = |offered: &[&str]| {
        TEXT_TYPES
            .into_iter()
            .find(|t| offered.iter().any(|m| m == t))
    };
    assert_eq!(
        pick(&["image/png", "STRING", "UTF8_STRING"]),
        Some("UTF8_STRING")
    );
    assert_eq!(
        pick(&["text/plain", "text/plain;charset=utf-8"]),
        Some("text/plain;charset=utf-8")
    );
    assert_eq!(pick(&["STRING"]), Some("STRING"));
    assert_eq!(pick(&["image/png", "text/html"]), None);
}
