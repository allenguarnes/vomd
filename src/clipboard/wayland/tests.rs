use std::io::Read;

use super::*;
use crate::copy::CHUNK;

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
