//! `cargo run --release --example parse_bench -- fixtures/huge.md`
//! The 5 MB fixture parses in ≤ 50 ms.
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let src = std::fs::read_to_string(&path).expect("read");
    let mut best = f64::MAX;
    let mut doc = None;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        doc = Some(vomd::doc::Doc::parse(&src));
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    let doc = doc.unwrap();
    println!(
        "{path}: {} bytes → {} blocks, {} spans, {} links, {} images; best of 5: {best:.1} ms",
        src.len(),
        doc.blocks.len(),
        doc.spans.len(),
        doc.links.len(),
        doc.images.len()
    );
}
