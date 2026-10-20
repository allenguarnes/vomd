//! `cargo run --release --example find_bench -- fixtures/huge.md [query...]`
//! What one keystroke of the find bar costs: the best of 5 searches of the document's text for
//! each query (a rare one, a common one and one that is nowhere by default).
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("path");
    let mut queries: Vec<String> = args.collect();
    if queries.is_empty() {
        queries = ["e", "the", "qzxj", "lorem ipsum"].map(String::from).into();
    }
    let src = std::fs::read_to_string(&path).expect("read");
    let doc = vomd::doc::Doc::parse(&src);
    for q in &queries {
        let mut best = f64::MAX;
        let mut found = None;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            found = Some(vomd::find::search(&doc, q));
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        let f = found.unwrap();
        println!(
            "{q:?}: {} matches ({} kept) in {} KB of text; best of 5: {best:.2} ms",
            f.total,
            f.matches.len(),
            doc.text.len() >> 10
        );
    }
}
