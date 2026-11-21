//! Bytes → Doc IR: never panics, the segmented parse equals the one parser's, and the table rows
//! and sections are sound.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::doc::{BlockKind, Doc};

/// What the layout, copy and row splitting take for granted: a row's cells in ascending columns
/// within the table's, their spans in the row's and apart; sections sorted, each a summary of a
/// block or more with its body right after it, nested whole (in a body or in a summary).
fn check(doc: &Doc) {
    for block in &doc.blocks {
        let BlockKind::TableRow { table, .. } = block.kind else {
            continue;
        };
        let cols = doc.tables[table as usize].aligns.len() as u32;
        let (mut col, mut at) = (0, block.spans.start);
        for cell in doc.row_cell_list(block) {
            assert!(cell.col >= col && cell.span >= 1, "columns ascend");
            col = cell.col + u32::from(cell.span);
            assert!(col <= cols.max(1), "a cell within the table's columns");
            assert!(cell.spans.start >= at && cell.spans.end <= block.spans.end, "cells apart");
            at = cell.spans.end;
        }
    }
    let n = doc.blocks.len() as u32;
    for (k, s) in doc.sections.iter().enumerate() {
        assert!(!s.summary.is_empty() && s.summary.end == s.body.start && s.body.end <= n);
        for t in &doc.sections[k + 1..] {
            assert!(t.summary.start >= s.summary.start, "sections sorted");
            let in_body = t.summary.start >= s.body.start && t.body.end <= s.body.end;
            let in_summary = t.body.end <= s.summary.end;
            assert!(in_body || in_summary || t.summary.start >= s.body.end, "nested whole");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // The first byte picks the segment size, so small inputs are cut too (documents are cut
    // only past 1 MB in use, at about 16 KB).
    let Some((&b, rest)) = data.split_first() else {
        return;
    };
    let src = String::from_utf8_lossy(rest);
    let doc = Doc::parse(&src);
    for span in &doc.spans {
        let _ = doc.span_text(span);
    }
    check(&doc);
    let budget = 16 + usize::from(b) * 8;
    if let Some((whole, parts)) = Doc::fuzz_segments(&src, budget) {
        assert!(whole == parts, "segmented parse at {budget} differs");
    }
});
