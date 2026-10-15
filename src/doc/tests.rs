use super::*;

/// Human-readable IR dump for snapshots, one block per line:
/// `<kind> [list=N] [quote=N] [qstart=N] [qlists=[..]] [marker] [joined] | [style]"text" ...`
fn dump(doc: &Doc) -> String {
    let mut out = String::new();
    for block in &doc.blocks {
        let kind = match block.kind {
            BlockKind::Paragraph => "P".to_owned(),
            BlockKind::Heading(n) => format!("H{n}"),
            BlockKind::Code => "Code".to_owned(),
            BlockKind::Rule => "Rule".to_owned(),
            BlockKind::AlertTitle(a) => format!("Alert({a:?})"),
            BlockKind::TableRow { table, header, .. } => {
                format!("Row(t{table}{})", if header { ",head" } else { "" })
            }
        };
        out.push_str(&kind);
        if block.list > 0 {
            out.push_str(&format!(" list={}", block.list));
        }
        if block.quote > 0 {
            out.push_str(&format!(" quote={}", block.quote));
        }
        if block.quote_start > 0 {
            out.push_str(&format!(" qstart={}", block.quote_start));
        }
        let qlists = doc.block_quote_lists(block);
        if qlists.iter().any(|&depth| depth > 0) {
            out.push_str(&format!(" qlists={qlists:?}"));
        }
        if block.marker != Marker::None {
            out.push_str(&format!(" {:?}", block.marker));
        }
        if block.joined {
            out.push_str(" joined");
        }
        out.push_str(" |");
        let spans_of = |spans: &[Span], out: &mut String| {
            for span in spans {
                out.push(' ');
                out.push_str(&span_dump(doc, span));
            }
        };
        if let BlockKind::TableRow { .. } = block.kind {
            let mut next = 0;
            for (col, cell) in doc.row_cells(block) {
                // Fragments of a split row skip the columns that ran out of text.
                if col != next {
                    out.push_str(&format!(" c{col}"));
                }
                next = col + 1;
                out.push_str(" [");
                spans_of(cell, &mut out);
                out.push_str(" ]");
            }
        } else {
            spans_of(doc.block_spans(block), &mut out);
        }
        out.push('\n');
    }
    for (i, l) in doc.links.iter().enumerate() {
        out.push_str(&format!("link {}: {l}\n", i + 1));
    }
    for (i, img) in doc.images.iter().enumerate() {
        out.push_str(&format!(
            "image {}: {} remote={}\n",
            i + 1,
            img.src,
            img.remote
        ));
    }
    for (slug, block) in &doc.anchors {
        out.push_str(&format!("anchor #{slug} -> {block}\n"));
    }
    out
}

fn span_dump(doc: &Doc, span: &Span) -> String {
    let mut tags = String::new();
    for (bit, c) in [
        (style::BOLD, 'B'),
        (style::ITALIC, 'I'),
        (style::CODE, 'C'),
        (style::STRIKE, 'S'),
        (style::LINK, 'L'),
        (style::IMAGE, 'M'),
        (style::FOOTREF, 'F'),
    ] {
        if span.style & bit != 0 {
            tags.push(c);
        }
    }
    if span.link != 0 {
        tags.push_str(&span.link.to_string());
    }
    if span.image != 0 {
        tags.push_str(&format!("m{}", span.image));
    }
    let text = format!("{:?}", doc.span_text(span));
    if tags.is_empty() {
        text
    } else {
        format!("[{tags}]{text}")
    }
}

/// Structural invariants every parsed document must satisfy.
fn check_invariants(doc: &Doc) {
    for span in &doc.spans {
        assert!(span.start <= span.end && span.end as usize <= doc.text.len());
        assert!(doc.text.is_char_boundary(span.start as usize));
        assert!(doc.text.is_char_boundary(span.end as usize));
        assert!(span.link as usize <= doc.links.len());
        assert!(span.image as usize <= doc.images.len());
        assert_eq!(span.image != 0, span.style & style::IMAGE != 0);
    }
    for block in &doc.blocks {
        assert!(block.spans.start <= block.spans.end);
        assert!(block.spans.end as usize <= doc.spans.len());
        if let BlockKind::TableRow {
            first_cell, cells, ..
        } = block.kind
        {
            assert!((first_cell + cells) as usize <= doc.cells.len());
            let mut col = 0;
            for cell in doc.cells[first_cell as usize..][..cells as usize].iter() {
                assert!(cell.spans.start >= block.spans.start && cell.spans.end <= block.spans.end);
                assert!(cell.col >= col, "columns ascend");
                col = cell.col + 1;
            }
        }
    }
    for (_, block) in &doc.anchors {
        assert!((*block as usize) < doc.blocks.len());
    }
    for h in &doc.headings {
        assert!(matches!(
            doc.blocks[h.block as usize].kind,
            BlockKind::Heading(_)
        ));
    }
}

fn parse(src: &str) -> String {
    let doc = Doc::parse(src);
    check_invariants(&doc);
    dump(&doc)
}

#[test]
fn inline_styles() {
    insta::assert_snapshot!(parse(
        "Plain *italic* **bold** ***both*** ~~strike~~ `code` and [a **bold** link](https://x.y).\n\
         Soft\nbreak and hard  \nbreak."
    ));
}

#[test]
fn inline_nesting_depth_is_not_capped() {
    // 65536 nested levels used to overflow a u16 counter (a debug panic, or a wrap to zero that
    // dropped the style).
    let strong = format!("{}x{}", "**".repeat(65_536), "**".repeat(65_536));
    // Alternating `*` and `_` emphasis nests 70000 deep.
    let italic = format!("{}x {}", "*a _a ".repeat(35_000), "a_ a* ".repeat(35_000));
    for (src, flag) in [(strong, style::BOLD), (italic, style::ITALIC)] {
        let doc = Doc::parse(&src);
        check_invariants(&doc);
        let x = doc.spans.iter().find(|s| doc.span_text(s).contains('x'));
        let x = x.unwrap_or_else(|| panic!("{:.100}", dump(&doc)));
        assert!(x.style & flag != 0, "style {:#b}", x.style);
    }
}

#[test]
fn headings_and_slugs() {
    insta::assert_snapshot!(parse(
        "# Hello, World!\n## Hello, World!\n### Ünïcode & `code` — dash\n#### foo-1\n# Hello, World!\n# \n"
    ));
}

#[test]
fn lists() {
    insta::assert_snapshot!(parse(
        "- one\n- two\n  - nested *a*\n  - nested b\n- three\n\n\
         3. third\n4. fourth\n\n\
         - [x] done\n- [ ] todo\n\n\
         1. loose\n\n   second para\n\n2. next\n"
    ));
}

#[test]
fn quotes_and_alerts() {
    insta::assert_snapshot!(parse(
        "> quoted\n> > nested\n\n> [!WARNING]\n> Be careful.\n\n> [!TIP]\n> - item\n"
    ));
}

#[test]
fn code_blocks() {
    insta::assert_snapshot!(parse(
        "```rust\nfn main() {\n    *not emphasis*\n}\n```\n\n    indented\n    code\n\n```\n```\n"
    ));
}

#[test]
fn tables() {
    insta::assert_snapshot!(parse(
        "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | **b** c | `d` |\n| | x | [l](#y) |\n"
    ));
    let doc = Doc::parse("| a | b |\n|---|:-:|\n| 1 | 2 |\n");
    assert_eq!(doc.tables[0].aligns, vec![Align::None, Align::Center]);
}

#[test]
fn footnotes() {
    insta::assert_snapshot!(parse(
        "First[^b] and second[^a] and again[^b].\n\n[^a]: Note A.\n[^b]: Note B with *style*.\n\n# After\n"
    ));
}

#[test]
fn raw_html() {
    insta::assert_snapshot!(parse(
        "<p align=\"center\">\n  <img src=\"logo.png\" alt=\"Logo &amp; mark\" width=200>\n</p>\n\n\
         <!-- hidden comment -->\n\n\
         Inline <b>bold?</b> stays text, line<br>break, <img src='https://x.y/a.png'>.\n\n\
         <details>\n<summary>More &lt;info&gt;</summary>\n\nMarkdown *inside*.\n\n</details>\n"
    ));
}

#[test]
fn images_and_links() {
    let doc = Doc::parse(
        "![alt *text*](local.png) [![badge](https://img.shields.io/x.svg)](https://ci) ![](http://a/b.jpg)",
    );
    check_invariants(&doc);
    assert_eq!(doc.images.len(), 3);
    assert_eq!(doc.remote_image_count(), 2);
    insta::assert_snapshot!(dump(&doc));
}

#[test]
fn front_matter_hidden() {
    insta::assert_snapshot!(parse("---\ntitle: Secret\ntags: [a]\n---\n\n# Title\n"));
}

#[test]
fn empty_and_degenerate() {
    assert_eq!(parse(""), "");
    assert_eq!(parse("   \n\n\t\n"), "");
    parse("*");
    parse("[unclosed](");
    parse("<");
    parse("<img src=");
    parse("&#xFFFFFFFF; &#0; &; &amp");
    parse(&"> ".repeat(300));
    parse(&"- ".repeat(300));
    parse("| a |\n|---|\n| b | extra | cells |\n");
}

#[test]
fn entities() {
    assert_eq!(
        decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &bogus; &"),
        "a & b <c> AB &bogus; &"
    );
    assert_eq!(
        attr("img SRC='x.png' alt=\"A B\"", "src").as_deref(),
        Some("x.png")
    );
    assert_eq!(
        attr("img data-src=\"no\" src=yes", "src").as_deref(),
        Some("yes")
    );
    assert_eq!(attr("img alt", "alt"), None);
}

#[test]
fn entity_scan_is_linear() {
    // Every `&` used to scan the whole remainder for a `;`: quadratic on a run of them.
    let amps = "&".repeat(400_000);
    let t = std::time::Instant::now();
    assert_eq!(decode_entities(&amps), amps);
    let doc = Doc::parse(&format!("<div>{amps}</div>\n"));
    check_invariants(&doc);
    assert_eq!(doc.text, amps);
    // A `;` just past the 10-byte reach is not an entity.
    assert_eq!(decode_entities("&aaaaaaaaaa;&lt;"), "&aaaaaaaaaa;<");
    assert!(t.elapsed().as_secs_f64() < 1.0, "{:?}", t.elapsed());
}

/// Every example in the CommonMark spec parses without panicking and satisfies invariants.
#[test]
fn commonmark_spec_examples() {
    let spec = include_str!("../../fixtures/spec.md");
    let fence = "````````````````````````````````";
    let mut count = 0;
    let mut rest = spec;
    while let Some(i) = rest.find(&format!("{fence} example")) {
        rest = &rest[i + fence.len() + 8..];
        let end = rest.find(&format!("\n{fence}")).unwrap();
        let example = &rest[..end];
        let md = example.split("\n.\n").next().unwrap().replace('→', "\t");
        check_invariants(&Doc::parse(&md));
        rest = &rest[end..];
        count += 1;
    }
    assert!(count > 600, "found only {count} examples");
}

#[test]
fn fixtures_parse() {
    for name in ["tiny", "readme", "spec", "cjk", "rtl", "emoji", "tables"] {
        let path = format!("{}/fixtures/{name}.md", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        let doc = Doc::parse(&src);
        check_invariants(&doc);
        assert!(!doc.blocks.is_empty(), "{name}");
    }
}

#[test]
fn fixture_tables_snapshot() {
    let src = include_str!("../../fixtures/tables.md");
    insta::assert_snapshot!(parse(src));
}

#[test]
fn empty_items_keep_their_marker() {
    insta::assert_snapshot!(parse("- a\n-\n\nNext para\n"));
    insta::assert_snapshot!(parse("1. a\n2.\n\n# Heading\n"));
    // An empty item whose only content is a nested list keeps its own marker.
    insta::assert_snapshot!(parse("-\n  - b\n"));
}

#[test]
fn quote_boundaries() {
    insta::assert_snapshot!(parse(
        "> [!NOTE]\n> n\n\n> plain quote\n\n> a\n>\n> > inner\n\n>\n\nafter empty quote\n"
    ));
}

#[test]
fn footnote_edge_cases() {
    insta::assert_snapshot!(parse("x[^a] y[^b]\n\n[^a]: - item\n[^b]:\n"));
    insta::assert_snapshot!(parse("x[^a] y[^b]\n\n[^a]:\n[^b]: text\n"));
}

#[test]
fn html_tag_with_gt_in_attribute() {
    insta::assert_snapshot!(parse(
        "Text <img alt=\"a>b\" src='x>y.png'> after <span title=\"1 > 0\">span</span>.\n"
    ));
}

#[test]
fn markers_survive_non_paragraph_item_starts() {
    insta::assert_snapshot!(parse("1. ```\n   code\n   ```\n2. next\n"));
    insta::assert_snapshot!(parse("- | x | y |\n  |---|---|\n  | 1 | 2 |\n- next\n"));
    insta::assert_snapshot!(parse("- > [!NOTE]\n  > n\n- next\n\n- * * *\n"));
}

#[test]
fn footnote_definition_keeps_quote_start() {
    insta::assert_snapshot!(parse("> a\n\n> [^x]: d\n>\n> b\n\ny[^x]\n"));
}

#[test]
fn quote_list_nesting_order_is_recorded() {
    insta::assert_snapshot!(parse("- item\n\n  > quoted\n\n> - item\n>   > deep\n"));
}

/// Chunks of `text` as (text, style, link) triples, for `split_spans` expectations.
fn cut(text: &str, spans: &[Span], max: usize) -> Vec<Vec<(String, u8, u32)>> {
    let (spans, chunks) = split_spans(text, spans, max);
    chunks
        .into_iter()
        .map(|r| {
            spans[r.start as usize..r.end as usize]
                .iter()
                .map(|s| {
                    assert!(text.is_char_boundary(s.start as usize));
                    assert!(text.is_char_boundary(s.end as usize));
                    (
                        text[s.start as usize..s.end as usize].to_owned(),
                        s.style,
                        s.link,
                    )
                })
                .collect()
        })
        .collect()
}

fn plain(text: &str) -> Span {
    Span {
        start: 0,
        end: text.len() as u32,
        style: 0,
        link: 0,
        image: 0,
    }
}

#[test]
fn split_prefers_newline_then_whitespace_then_char_boundary() {
    let t = |s: &str, max| -> Vec<String> {
        cut(s, &[plain(s)], max)
            .into_iter()
            .map(|c| c.into_iter().map(|(t, ..)| t).collect())
            .collect()
    };
    // The last newline in reach wins over a later space; the newline ends its chunk.
    assert_eq!(t("ab\ncd ef gh", 8), ["ab\n", "cd ef gh"]);
    assert_eq!(t("aa\nbb\ncc\ndd", 8), ["aa\nbb\n", "cc\ndd"]);
    // No newline: after the last whitespace.
    assert_eq!(t("abc def ghi jkl", 9), ["abc def ", "ghi jkl"]);
    // No whitespace: at the limit.
    assert_eq!(t("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
    // Multi-byte chars are never cut: "é" is 2 bytes, so the 5-byte limit lands inside the 3rd.
    assert_eq!(t("ééééé", 5), ["éé", "éé", "é"]);
    // A 4-byte char at the limit, and a limit smaller than the char.
    assert_eq!(t("ab😀cd", 5), ["ab", "😀c", "d"]);
    assert_eq!(t("😀😀", 2), ["😀", "😀"]);
    // Multi-byte whitespace counts too (U+3000 is 3 bytes).
    assert_eq!(t("ab\u{3000}cdef", 7), ["ab\u{3000}", "cdef"]);
    // A leading newline is a blank line of its own.
    assert_eq!(t("\nabcdefgh", 4), ["\n", "abcd", "efgh"]);
    // Blank lines survive a cut: a cut never lands right before a newline (that would make
    // the end of the line a blank line), so the newline moves in or the last char moves out.
    assert_eq!(t("ab\n\n\ncd", 5), ["ab\n\n\n", "cd"]);
    assert_eq!(t("aaa bbb \nccc", 8), ["aaa ", "bbb \nccc"]);
    assert_eq!(t("abcdef\nghi", 6), ["abcde", "f\nghi"]);
    // Short text is a single chunk.
    assert_eq!(t("short", 8), ["short"]);
}

#[test]
fn split_cuts_spans_at_the_boundary() {
    // "aaa bbb ccc" with the middle word bold and linked: the cut after "bbb " splits nothing,
    // but a cut inside "bbb" splits the span into two with the same style and link.
    let text = "aaa bbbbbbbb ccc";
    let spans = [
        Span {
            start: 0,
            end: 4,
            ..plain(text)
        },
        Span {
            start: 4,
            end: 12,
            style: style::BOLD | style::LINK,
            link: 7,
            image: 0,
        },
        Span {
            start: 12,
            end: 16,
            ..plain(text)
        },
    ];
    let chunks = cut(text, &spans, 6);
    let b = style::BOLD | style::LINK;
    assert_eq!(
        chunks,
        vec![
            vec![("aaa ".to_owned(), 0, 0)],
            vec![("bbbbbb".to_owned(), b, 7)],
            vec![("bb".to_owned(), b, 7), (" ccc".to_owned(), 0, 0)],
        ]
    );
    // Rejoining the chunks gives back the text.
    let joined: String = chunks.iter().flatten().map(|(t, ..)| t.as_str()).collect();
    assert_eq!(joined, text);
}

/// `(kind, text)` of every block, with chunk runs rejoined (chunks keep their newlines, so
/// they concatenate back to the original text).
fn runs(doc: &Doc) -> Vec<(BlockKind, String)> {
    let mut out: Vec<(BlockKind, String)> = Vec::new();
    for b in &doc.blocks {
        let text: String = doc
            .block_spans(b)
            .iter()
            .map(|s| doc.span_text(s))
            .collect();
        match out.last_mut() {
            Some((_, t)) if b.joined => t.push_str(&text),
            _ => out.push((b.kind, text)),
        }
    }
    out
}

#[test]
fn oversized_code_block_is_chunked() {
    let code: String = (0..6000).map(|i| format!("line {i}\n")).collect();
    let doc = Doc::parse(&format!("```\n{code}```\n"));
    check_invariants(&doc);
    assert!(doc.blocks.len() > 2);
    for (i, b) in doc.blocks.iter().enumerate() {
        assert_eq!(b.kind, BlockKind::Code);
        assert_eq!(b.joined, i > 0);
        let bytes: usize = doc
            .block_spans(b)
            .iter()
            .map(|s| s.end as usize - s.start as usize)
            .sum();
        assert!(bytes <= MAX_BLOCK_BYTES, "chunk {i}: {bytes} bytes");
        // Cut after a newline: the chunks hold whole lines.
        let first = doc.span_text(&doc.block_spans(b)[0]);
        assert!(first.starts_with("line "), "{first:?}");
    }
    // Rejoining the chunks restores the code (minus its trailing newline).
    let runs = runs(&doc);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1, code.trim_end());
    // The dump flags continuation chunks.
    let dump = dump(&doc);
    let mut lines = dump.lines();
    assert!(lines.next().unwrap().starts_with("Code |"));
    assert!(lines.next().unwrap().starts_with("Code joined |"));
}

#[test]
fn chunks_keep_nesting_and_put_markers_on_the_first() {
    let words = "lorem ipsum ".repeat(4000);
    // A list item and a quote with a huge paragraph: the marker and quote start stay on the
    // first chunk; the others inherit list/quote depth.
    let doc = Doc::parse(&format!("1. {words}\n\n> > {words}\n\nafter\n"));
    check_invariants(&doc);
    let item: Vec<_> = doc.blocks.iter().filter(|b| b.list == 1).collect();
    assert!(item.len() > 2);
    assert_eq!(item[0].marker, Marker::Ordered(1));
    assert!(
        item[1..]
            .iter()
            .all(|b| b.marker == Marker::None && b.joined)
    );
    let quote: Vec<_> = doc.blocks.iter().filter(|b| b.quote == 2).collect();
    assert!(quote.len() > 2);
    assert_eq!(quote[0].quote_start, 1);
    assert!(quote[1..].iter().all(|b| b.quote_start == 0 && b.joined));
    // The paragraph after is not joined to anything.
    let last = doc.blocks.last().unwrap();
    assert!(!last.joined && last.quote == 0 && last.list == 0);
    // Paragraph chunks split at whitespace: no word is cut.
    for r in runs(&doc).iter().filter(|r| r.1.len() > MAX_BLOCK_BYTES) {
        for word in r.1.split_whitespace() {
            assert!(word == "lorem" || word == "ipsum", "{word:?}");
        }
    }
}

#[test]
fn chunked_headings_and_footnotes_anchor_at_the_first_chunk() {
    let words = "alpha beta ".repeat(3000);
    let doc = Doc::parse(&format!("# {words}\n\nSee[^n].\n\n[^n]: {words}\n"));
    check_invariants(&doc);
    let first = doc
        .blocks
        .iter()
        .position(|b| matches!(b.kind, BlockKind::Heading(_)));
    assert_eq!(first, Some(0));
    assert_eq!(doc.headings.len(), 1);
    assert_eq!(doc.headings[0].block, 0);
    assert!(doc.blocks[1].joined, "heading continues");
    let anchor = |name: &str| doc.anchors.iter().find(|(s, _)| s == name).map(|(_, b)| *b);
    assert_eq!(anchor(&doc.headings[0].slug), Some(0));
    let def = anchor("fn-n").unwrap() as usize;
    assert_eq!(doc.blocks[def].marker, Marker::Footnote(1));
    assert!(!doc.blocks[def].joined && doc.blocks[def + 1].joined);
    assert!(anchor("fnref-n").is_some());
    // The back-link rides on the definition's last chunk.
    let end = doc.blocks.last().unwrap();
    assert!(
        doc.block_spans(end)
            .last()
            .is_some_and(|s| doc.span_text(s) == " ↩")
    );
}

/// Text of each column of a table row block (columns a fragment doesn't store are empty).
fn cell_texts(doc: &Doc, block: &Block) -> Vec<String> {
    let BlockKind::TableRow { table, .. } = block.kind else {
        panic!("not a table row");
    };
    let mut out = vec![String::new(); doc.tables[table as usize].aligns.len()];
    for (col, spans) in doc.row_cells(block) {
        out[col] = spans.iter().map(|s| doc.span_text(s)).collect();
    }
    out
}

/// Bytes of text in a block's spans.
fn block_bytes(doc: &Doc, block: &Block) -> usize {
    doc.block_spans(block)
        .iter()
        .map(|s| (s.end - s.start) as usize)
        .sum()
}

#[test]
fn oversized_table_rows_split_into_fragments() {
    let big = "alpha beta ".repeat(5000);
    let big = big.trim_end();
    let doc = Doc::parse(&format!(
        "| h1 | h2 |\n|:--|--:|\n| small | {big} |\n| tail | x |\n"
    ));
    check_invariants(&doc);
    // Header, the row's fragments, the next row.
    let n = doc.blocks.len();
    assert!(n > 5, "{n} blocks");
    let row = &doc.blocks[1..n - 1];
    assert!(!doc.blocks[0].joined && !row[0].joined && !doc.blocks[n - 1].joined);
    assert!(row[1..].iter().all(|b| b.joined));
    let mut cells: [String; 2] = Default::default();
    for (k, b) in row.iter().enumerate() {
        let BlockKind::TableRow {
            table,
            header,
            cells: count,
            ..
        } = b.kind
        else {
            panic!("fragment {k} is a table row");
        };
        // The first fragment stores both cells; later ones only the cell with text left.
        assert_eq!(
            (table, header, count),
            (0, false, if k == 0 { 2 } else { 1 }),
            "fragment {k}"
        );
        assert!(block_bytes(&doc, b) <= MAX_BLOCK_BYTES, "fragment {k}");
        let texts = cell_texts(&doc, b);
        // The short cell is in the first fragment; later fragments hold an empty cell.
        assert_eq!(texts[0], if k == 0 { "small" } else { "" }, "fragment {k}");
        for (all, t) in cells.iter_mut().zip(&texts) {
            all.push_str(t);
        }
    }
    // Rejoined, the pieces are the original cells; pieces break at whitespace, never in words.
    assert_eq!(cells, ["small", big]);
    for b in row {
        let t = &cell_texts(&doc, b)[1];
        assert!(t.split_whitespace().all(|w| w == "alpha" || w == "beta"));
    }
    // The next row is intact, and the table has its one alignment list.
    assert_eq!(cell_texts(&doc, &doc.blocks[n - 1]), ["tail", "x"]);
    assert_eq!(doc.tables[0].aligns, [Align::Left, Align::Right]);
    // Small rows are never split.
    let doc = Doc::parse("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
    assert_eq!(doc.blocks.len(), 3);
    assert!(doc.blocks.iter().all(|b| !b.joined));
}

#[test]
fn row_fragments_only_visit_the_cells_with_text_left() {
    // One cell with a lot of text among many one-character cells: after the first fragment the
    // short cells are done, and visiting them again for each fragment would be quadratic.
    let visits = |n: usize| {
        let long = "word ".repeat(16 * n);
        let long = long.trim_end();
        let head = "| h ".repeat(n) + "|\n" + &"|-".repeat(n) + "|\n";
        let src = format!("{head}| {long} {}|\n", "| x ".repeat(n - 1));
        super::ROW_CELL_VISITS.with(|v| v.set(0));
        let doc = Doc::parse(&src);
        let visits = super::ROW_CELL_VISITS.with(|v| v.get());
        check_invariants(&doc);
        // Header row, then the body row's fragments: the long cell comes back whole.
        let rows: Vec<_> = doc.blocks.iter().filter(|b| !b.joined).collect();
        assert_eq!(rows.len(), 2);
        let body: Vec<_> = doc.blocks[1..].iter().collect();
        let first = cell_texts(&doc, body[0]);
        assert_eq!(first.len(), n);
        let mut text = first[0].clone();
        for b in &body[1..] {
            text += &cell_texts(&doc, b)[0];
        }
        assert_eq!(text, long);
        (visits, doc.blocks.len())
    };
    let mut prev = None;
    for n in [4000, 8000, 16000] {
        let (visits, blocks) = visits(n);
        assert!(blocks > 20, "{n} columns: {blocks} blocks");
        // The cells once, and one cell per fragment after the first (a small constant each).
        assert!(visits <= 4 * (n + blocks), "{n} columns: {visits} visits");
        if let Some(prev) = prev {
            assert!(
                visits < prev * 5 / 2,
                "{n} columns: {prev} -> {visits} visits"
            );
        }
        prev = Some(visits);
    }
}

#[test]
fn table_row_fragments_cut_on_char_boundaries_and_keep_headers() {
    // 3-byte chars with no whitespace: the cell budget lands inside characters.
    let wide = "語".repeat(12_000);
    let doc = Doc::parse(&format!("| {wide} | b |\n|---|---|\n| 1 | {wide} |\n"));
    check_invariants(&doc);
    let (head, body): (Vec<_>, Vec<_>) = doc
        .blocks
        .iter()
        .partition(|b| matches!(b.kind, BlockKind::TableRow { header: true, .. }));
    assert!(head.len() > 2 && body.len() > 2);
    // A header split into fragments stays a header; the body row after it starts fresh.
    assert!(!head[0].joined && head[1..].iter().all(|b| b.joined));
    assert!(!body[0].joined && body[1..].iter().all(|b| b.joined));
    assert!(!std::ptr::eq(head[0], body[0]));
    for (group, cell) in [(&head, 0), (&body, 1)] {
        let text: String = group
            .iter()
            .map(|b| cell_texts(&doc, b)[cell].clone())
            .collect();
        assert_eq!(text, wide);
    }
    // Every fragment reads as a full row: columns that ran out are empty.
    for b in &doc.blocks {
        assert_eq!(cell_texts(&doc, b).len(), 2);
    }
}

#[test]
fn footnote_back_links_target_the_chunk_with_the_reference() {
    let words = "word ".repeat(10_000);
    let doc = Doc::parse(&format!("{words} ref[^n]\n\n[^n]: def\n"));
    check_invariants(&doc);
    let target = doc
        .anchors
        .iter()
        .find(|(s, _)| s == "fnref-n")
        .map(|(_, b)| *b as usize)
        .unwrap();
    assert!(target > 1, "the reference is in a later chunk: {target}");
    assert!(
        doc.block_spans(&doc.blocks[target])
            .iter()
            .any(|s| s.style & style::FOOTREF != 0)
    );
    // Only that chunk holds the reference.
    let refs = doc.blocks.iter().filter(|b| {
        doc.block_spans(b)
            .iter()
            .any(|s| s.style & style::FOOTREF != 0)
    });
    assert_eq!(refs.count(), 1);
    // A reference inside a split table row resolves to its fragment too.
    let big = "alpha ".repeat(6000);
    let doc = Doc::parse(&format!(
        "| a | b |\n|---|---|\n| {big} | {big} end[^m] |\n\n[^m]: def\n"
    ));
    check_invariants(&doc);
    let target = doc
        .anchors
        .iter()
        .find(|(s, _)| s == "fnref-m")
        .map(|(_, b)| *b as usize)
        .unwrap();
    assert!(doc.blocks[target].joined);
    assert!(
        doc.block_spans(&doc.blocks[target])
            .iter()
            .any(|s| s.style & style::FOOTREF != 0)
    );
}

#[test]
fn id_spaces_are_not_capped_at_16_bits() {
    const N: usize = 70_000;
    let links: String = (0..N).map(|i| format!("[a](#{i}) ")).collect();
    let doc = Doc::parse(&links);
    check_invariants(&doc);
    assert_eq!(doc.links.len(), N);
    let last = doc.spans.iter().rev().find(|s| s.link != 0).unwrap();
    assert_eq!(last.link as usize, N);
    assert_eq!(doc.links[last.link as usize - 1], format!("#{}", N - 1));

    let images: String = (0..N).map(|i| format!("![](i{i}.png) ")).collect();
    let doc = Doc::parse(&images);
    check_invariants(&doc);
    assert_eq!(doc.images.len(), N);
    let last = doc.spans.iter().rev().find(|s| s.image != 0).unwrap();
    assert_eq!(
        doc.images[last.image as usize - 1].src,
        format!("i{}.png", N - 1)
    );
}

#[test]
fn many_footnotes_are_numbered_and_ordered() {
    const N: usize = 65_537;
    let mut src = String::new();
    for i in 0..N {
        src.push_str(&format!("[^n{i}]: x\n\n"));
    }
    // Mentioning a defined label again must not allocate a number (or overflow).
    src.push_str("ref[^n0] and[^n7].\n");
    let doc = Doc::parse(&src);
    check_invariants(&doc);
    let nums: Vec<u32> = doc
        .blocks
        .iter()
        .filter_map(|b| match b.marker {
            Marker::Footnote(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(nums, (1..=N as u32).collect::<Vec<_>>());
    // References are numbered in order of mention, and notes nothing refers to come after them.
    let refs: Vec<&str> = doc
        .spans
        .iter()
        .filter(|s| s.style & style::FOOTREF != 0)
        .map(|s| doc.span_text(s))
        .collect();
    assert_eq!(refs, ["1", "2"]);
}

#[test]
fn table_ids_are_not_capped_at_16_bits() {
    let mut src: String = (0..65_536).map(|_| "|a|\n|-|\n|b|\n\n").collect();
    src.push_str("|x|y|\n|-|-|\n|1|2|\n");
    let doc = Doc::parse(&src);
    check_invariants(&doc);
    assert_eq!(doc.tables.len(), 65_537);
    let Some(BlockKind::TableRow { table, cells, .. }) = doc.blocks.last().map(|b| b.kind) else {
        panic!("last block is a table row");
    };
    assert_eq!((table, cells), (65_536, 2));
    assert_eq!(doc.tables[table as usize].aligns.len(), 2);
}

#[test]
fn html_attributes_are_parsed_by_boundaries() {
    let src = |tag: &str| {
        let doc = Doc::parse(&format!("<p>\n{tag}\n</p>\n"));
        let img = doc.images.first().expect("an image").clone();
        let alt = doc
            .spans
            .iter()
            .find(|s| s.image != 0)
            .map(|s| doc.span_text(s).to_owned());
        (img.src, img.remote, alt.unwrap())
    };
    assert_eq!(
        src("<img src=https://example.com/a.png alt=pic>"),
        (
            "https://example.com/a.png".to_owned(),
            true,
            "pic".to_owned()
        )
    );
    // Another attribute's value that merely contains `src='...'` is not the src.
    assert_eq!(
        src("<img title=\"fake src='wrong.png'\" src='right.png'>").0,
        "right.png"
    );
    // Entities are decoded in src and alt.
    assert_eq!(
        src("<img src=\"a.png?x=1&amp;y=2\" alt=\"a &lt; b\">"),
        ("a.png?x=1&y=2".to_owned(), false, "a < b".to_owned())
    );
    // `/>` closes the tag; it isn't part of an unquoted value.
    assert_eq!(src("<img src=a.png/>").0, "a.png");
    assert_eq!(src("<img src=a.png />").0, "a.png");
    assert_eq!(src("<img src=dir/a.png/>").0, "dir/a.png");
    assert_eq!(
        attr("img src = 'x y.png'", "src").as_deref(),
        Some("x y.png")
    );
    assert_eq!(
        attr("img alt=\"unterminated", "alt").as_deref(),
        Some("unterminated")
    );
}

#[test]
fn heading_slugs_never_collide() {
    let anchors = |src: &str| -> Vec<String> {
        Doc::parse(src)
            .headings
            .into_iter()
            .map(|h| h.slug)
            .collect()
    };
    assert_eq!(
        anchors("# foo\n# foo-1\n# foo\n"),
        ["foo", "foo-1", "foo-2"]
    );
    assert_eq!(anchors("# a-1\n# a\n# a\n"), ["a-1", "a", "a-2"]);
    assert_eq!(
        anchors("# a\n# a\n# a-1\n# a\n"),
        ["a", "a-1", "a-1-1", "a-2"]
    );
    let doc = Doc::parse("# foo\n# foo-1\n# foo\n# a-1\n# a\n# a\n");
    let mut seen: Vec<&String> = doc.anchors.iter().map(|(s, _)| s).collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), doc.anchors.len());
}

#[test]
fn quote_list_depths_are_exact_at_any_depth() {
    insta::assert_snapshot!(parse("> > > > - > note\n"));
    // Deeper than any fixed table.
    let doc = Doc::parse(&format!("{}- x\n", "> ".repeat(9)));
    let b = doc.blocks.last().unwrap();
    assert_eq!(b.quote, 9);
    assert_eq!(doc.block_quote_lists(b), [0; 9]);
    let doc = Doc::parse("- > - > - > - > - > x\n");
    let b = doc.blocks.last().unwrap();
    assert_eq!((b.list, b.quote), (5, 5));
    assert_eq!(doc.block_quote_lists(b), [1, 2, 3, 4, 5]);
}

/// How many segments `src` is cut into at `budget`.
fn segment_count(src: &str, budget: usize) -> usize {
    segment::plan(src, budget).map_or(1, |p| p.segments.len())
}

/// How many of the cuts at `budget` are between two items of a list.
fn list_joins(src: &str, budget: usize) -> usize {
    segment::plan(src, budget).map_or(0, |p| p.segments.iter().filter(|s| s.list).count())
}

/// Segmented parsing at `budget` bytes gives exactly the document one parser gives. Returns the
/// number of segments, so callers can check that the text really was cut.
fn assert_same(name: &str, src: &str, budget: usize) -> usize {
    let whole = Doc::parse_whole(src);
    let Some(parts) = Doc::parse_segments(src, budget) else {
        return 1;
    };
    check_invariants(&parts);
    if whole != parts {
        std::fs::write("/tmp/vomd_fail.md", src).ok();
        let (a, b) = (dump(&whole), dump(&parts));
        let at = a.lines().zip(b.lines()).position(|(x, y)| x != y);
        let (la, lb): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
        let at = at.unwrap_or(la.len().min(lb.len()));
        panic!(
            "{name} @ {budget}: segmented parse differs at dump line {at}\n  whole:     {:.300}\n  segmented: {:.300}\n  ({} vs {} lines)",
            la.get(at).unwrap_or(&"<end>"),
            lb.get(at).unwrap_or(&"<end>"),
            la.len(),
            lb.len()
        );
    }
    segment_count(src, budget)
}

#[test]
fn segmented_parse_matches_fixtures() {
    for name in ["tiny", "readme", "spec", "cjk", "rtl", "emoji", "tables"] {
        let path = format!("{}/fixtures/{name}.md", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        for budget in [1, 200, 1024, 4096] {
            let n = assert_same(name, &src, budget);
            // The larger fixtures really are cut (the one-line ones can't be).
            if src.len() > 20_000 && budget <= 1024 {
                assert!(n >= 5, "{name} @ {budget}: {n} segments");
            }
        }
    }
}

#[test]
fn segmented_parse_handles_crlf_and_lone_cr_line_endings() {
    for name in ["readme", "tables", "spec"] {
        let path = format!("{}/fixtures/{name}.md", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path)
            .unwrap()
            .replace('\n', "\r\n");
        for budget in [1, 300, 2048] {
            let n = assert_same(&format!("{name} (CRLF)"), &src, budget);
            assert!(
                name == "tables" || n > 3,
                "{name} (CRLF) @ {budget}: {n} segments"
            );
        }
    }
    let table = big_table(500).replace('\n', "\r\n");
    assert!(assert_same("CRLF table", &table, 500) > 3);
    // A lone CR ends a line for pulldown-cmark but not for the planner: no cuts at all.
    let src = "one\rtwo\r\rthree\n\nfour\n";
    assert!(segment::plan(src, 1).is_none());
    assert_same("lone CR", src, 1);
}

#[test]
fn large_documents_parse_in_segments_with_the_same_result() {
    // Over `SEGMENT_THRESHOLD`, `Doc::parse` takes the segmented path by itself.
    let seed = include_str!("../../fixtures/readme.md").to_owned()
        + include_str!("../../fixtures/tables.md")
        + &big_table(200)
        + "\nNote[^n].\n\n[^n]: A note.\n";
    let src: String = (0..40)
        .map(|i| format!("\n# Section {i}\n\n{seed}"))
        .collect();
    assert!(src.len() > SEGMENT_THRESHOLD);
    let n = segment_count(&src, SEGMENT_BYTES);
    assert!(n > src.len() / SEGMENT_BYTES / 2, "{n} segments");
    let doc = Doc::parse(&src);
    check_invariants(&doc);
    assert!(doc == Doc::parse_whole(&src));
    // A giant table is one table, however many segments carry it.
    let table = big_table(40_000);
    assert!(table.len() > SEGMENT_THRESHOLD);
    let doc = Doc::parse(&table);
    assert_eq!(doc.tables.len(), 1);
    assert_eq!(doc.blocks.len(), 40_001);
    assert!(doc == Doc::parse_whole(&table));
}

/// Every example of the CommonMark spec, in one document and in groups, each example followed
/// by a paragraph. The examples leave fences, HTML blocks and lists open, so they interact; the
/// planner gives up on some of them (then the parse is whole and trivially equal).
#[test]
fn segmented_parse_matches_the_spec_examples_concatenated() {
    let spec = include_str!("../../fixtures/spec.md");
    let fence = "````````````````````````````````";
    let (mut examples, mut rest) = (Vec::new(), spec);
    while let Some(i) = rest.find(&format!("{fence} example")) {
        rest = &rest[i + fence.len() + 8..];
        let end = rest.find(&format!("\n{fence}")).unwrap();
        let md = rest[..end]
            .split("\n.\n")
            .next()
            .unwrap()
            .replace('→', "\t");
        examples.push(md);
        rest = &rest[end..];
    }
    let join = |group: &[String], sep: &str| -> String {
        group.iter().map(|e| format!("{e}{sep}")).collect()
    };
    let mut cuts = 0;
    for (sep, tag) in [
        ("\nA paragraph between examples.\n\n", "spaced"),
        ("\n", "tight"),
    ] {
        for budget in [1, 300] {
            assert_same(tag, &join(&examples, sep), budget);
            for (g, group) in examples.chunks(3).enumerate() {
                let src = join(group, sep);
                cuts += assert_same(&format!("{tag} group {g}"), &src, budget) - 1;
            }
        }
    }
    assert!(cuts > 100, "only {cuts} cuts");
}

/// A header, alignments, and rows with markup, so a cut between rows repeats a real header.
fn big_table(rows: usize) -> String {
    let mut src = String::from("| Name | Qty | Notes |\n|:-----|----:|:-----:|\n");
    for i in 0..rows {
        src.push_str(&format!("| item {i} | {i} | **b**{i} `c` [l](#{i}) |\n"));
    }
    src
}

#[test]
fn segmented_parse_cuts_giant_tables_between_rows() {
    let src = format!("# Title\n\n{}\nafter\n\n{}", big_table(3000), big_table(40));
    for budget in [1, 700, 4096] {
        let n = assert_same("tables", &src, budget);
        assert!(n > 5, "{n} segments at {budget}");
    }
    let plan = segment::plan(&src, 4096).unwrap();
    // Cuts inside the first table repeat its header, which the builder drops.
    assert!(plan.segments.iter().filter(|s| s.head.is_some()).count() > 3);
    let doc = Doc::parse_segments(&src, 4096).unwrap();
    assert_eq!(doc.tables.len(), 2);
    assert_eq!(
        doc.tables[0].aligns,
        [Align::Left, Align::Right, Align::Center]
    );
    let heads = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::TableRow { header: true, .. }))
        .count();
    assert_eq!(heads, 2);
    // The title, the first table (header and rows), a paragraph, the second table.
    assert_eq!(doc.blocks.len(), 1 + (1 + 3000) + 1 + (1 + 40));
}

/// Bytes the parsers read for `src` cut at `budget`: every segment with the header it repeats.
fn parser_input_bytes(src: &str, budget: usize) -> usize {
    segment::plan(src, budget).map_or(src.len(), |p| {
        let head = |s: &segment::Segment| s.head.as_ref().map_or(0, |h| h.len());
        p.segments.iter().map(|s| head(s) + s.range.len()).sum()
    })
}

#[test]
fn repeated_table_headers_cost_a_bounded_share_of_the_text() {
    // The header and the body grow together; repeating the header at every cut would cost
    // header × segments, quadratic in the size.
    let budget = 1024;
    let mut last = 0;
    for k in 0..4 {
        let cell = "h".repeat(512 << k);
        let mut src = format!("| {cell} | {cell} | {cell} | {cell} |\n|---|:-:|--:|---|\n");
        for i in 0..(1600 << k) {
            src.push_str(&format!("| row {i} | a | b | c |\n"));
        }
        let read = parser_input_bytes(&src, budget);
        assert!(
            read <= 2 * src.len(),
            "k={k}: {read} bytes read for {}",
            src.len()
        );
        // Still cut between rows, so the table is segmented at all.
        let n = assert_same("table header", &src, budget);
        assert!(n >= 4, "k={k}: {n} segments");
        assert!(read > last);
        last = read;
    }
}

#[test]
fn segmented_parse_resolves_definitions_from_other_segments() {
    let filler = "Some filler text to make the document longer.\n\n".repeat(30);
    // Reference definitions at the end, used at the start (with titles, labels in another case,
    // and a destination on the next line), plus a definition shadowed by a later duplicate.
    let src = format!(
        "[Foo], [bar][b], ![img][pic] and [foo].\n\n{filler}Again [FOO] and [missing].\n\n\
         [foo]: /first \"Title\"\n[b]:\n  /second\n[pic]: i.png\n\n[foo]: /ignored\n"
    );
    assert!(assert_same("definitions", &src, 400) > 3);
    let doc = Doc::parse_segments(&src, 400).unwrap();
    assert_eq!(&doc.links[..3], ["/first", "/second", "/first"]);
    assert_eq!(doc.images[0].src, "i.png");
    // Definitions before their use, and after a heading.
    let src = format!("# Links\n[x]: /x\n\n{filler}Use [x].\n");
    assert!(assert_same("definitions after heading", &src, 400) > 3);
    assert_eq!(Doc::parse_segments(&src, 400).unwrap().links, ["/x"]);
}

#[test]
fn segmented_parse_resolves_footnotes_from_other_segments() {
    let filler = "Some filler text to make the document longer.\n\n".repeat(30);
    let src = format!(
        "Early[^a] and [^B].\n\n{filler}Middle[^a], [^c] and [^missing].\n\n{filler}\
         [^a]: First note.\n\n    Second paragraph.\n\n[^b]: Second note with ref[^c].\n\n\
         [^c]: Third.\n\nLast[^a].\n"
    );
    assert!(assert_same("footnotes at the end", &src, 400) > 5);
    let doc = Doc::parse_segments(&src, 400).unwrap();
    let refs = doc
        .spans
        .iter()
        .filter(|s| s.style & style::FOOTREF != 0)
        .count();
    assert_eq!(refs, 6);
    // Definitions before their references work too, and so do ones inside a quote.
    let src = format!("[^a]: Note.\n\n> [^q]: Quoted.\n\n{filler}Late[^a] and[^q].\n");
    assert!(assert_same("footnotes at the start", &src, 400) > 3);
    // Front matter stays first.
    let src = format!("---\ntitle: x\n\nk: v\n---\nRef[^a].\n\n{filler}[^a]: Note.\n");
    assert!(assert_same("front matter and footnotes", &src, 400) > 3);
}

#[test]
fn segmented_parse_never_cuts_inside_blocks_that_span_blank_lines() {
    let filler = "Some filler text to make the document longer.\n\n".repeat(20);
    let blocks = [
        // Fenced code with blank lines, column-0 text and headings inside.
        "```\ncode\n\ntext at column 0\n\n# heading-like\n\n[x]: /not-a-definition\n```",
        "~~~\ncode\n\n~~~~\n\nstill code\n~~~~~",
        "  ```\n  indented fence\n\nstays\n  ```",
        // HTML blocks that only end at their closing tag.
        "<!--\ncomment\n\ntext in a comment\n\n-->",
        "<pre>\npre\n\ntext\n</pre>",
        "<script>\nvar a;\n\ntext\n</script>",
        "<style>\na {}\n\ntext\n</style>",
        "<![CDATA[\ndata\n\ntext\n]]>",
        "<?php\necho 1;\n\ntext\n?>",
        "<!DOCTYPE html\n\ntext\n>",
        // Lines that look like a metadata block in the middle of the text.
        "---\ntitle: x\n\ntext\n---",
        // Lists and quotes with blank lines between their parts.
        "- a\n\n- b\n\n  nested para\n\n- c",
        "1. one\n\n2. two\n\n   - nested\n\n     more\n\n3. three",
        "> quote\n>\n> more\n\n> another",
        "- item\n\n  > quote in item\n  >\n  > more\n\n  ```\n  code\n\n  more\n  ```\n\n- next",
        "> - a\n>\n>   b\n>\n> - c",
        "    indented\n\n    code\n\ntext",
    ];
    for (i, block) in blocks.iter().enumerate() {
        let src = format!("{filler}{block}\n\n{filler}Text at column 0.\n\n# After\n\n{filler}");
        for budget in [1, 90, 700] {
            assert_same(&format!("block {i}: {block}"), &src, budget);
        }
        let n = segment_count(&src, 90);
        assert!(n >= 3, "block {i}: {n} segments");
    }
    // All of them in one document, one after the other.
    let src: String = blocks.iter().map(|b| format!("{b}\n\n{filler}")).collect();
    for budget in [1, 90, 700] {
        assert_same("all blocks", &src, budget);
    }
}

/// Deterministic pseudo-random numbers (xorshift), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Lines that exercise every construct the segment scanner has to follow.
const FUZZ_LINES: &[&str] = &[
    "",
    "",
    "",
    "text",
    "more text",
    "Text with [foo] and [bar][baz] and [^a] and [^b].",
    "# Heading",
    "## Heading two",
    "Setext",
    "===",
    "---",
    "...",
    "* * *",
    "- item",
    "- [ ] task",
    "  - nested",
    "    - deeper",
    "1. one",
    "2) two",
    "10. ten",
    "+ plus",
    "> quote",
    "> > nested",
    ">",
    "> [!NOTE]",
    "    indented code",
    "\tcode",
    "```",
    "```rust",
    "~~~",
    "~~~~",
    "````",
    "  ```",
    "   ```",
    "    ```",
    "`` ` ``",
    "<!--",
    "-->",
    "<!-- one line -->",
    "<div>",
    "</div>",
    "<pre>",
    "</pre>",
    "<PRE>",
    "</PRE>",
    "<script>",
    "</script>",
    "<style>",
    "</style>",
    "<?php",
    "?>",
    "<!DOCTYPE html>",
    "<![CDATA[",
    "]]>",
    "<br>",
    "<img src=\"a.png\">",
    "| a | b |",
    "|---|:-:|",
    "| 1 | 2 |",
    "|",
    "|\t",
    "| \u{b}",
    "||",
    "| x |",
    "| a | b | c |",
    "a | b",
    "--|--",
    "[foo]: /url",
    "[foo]: /url \"title\"",
    "[bar]:",
    "/bar",
    "\"title\"",
    "[Baz]: <b z>",
    "[^a]: note a",
    "[^b]: note b",
    "    more note",
    "[^a]:",
    "  > [^b]: quoted note",
    "- [^a]: listed",
    "\u{a0}",
    "  ",
    "\t",
    "text  ",
    "&amp; &lt;",
    "\\",
    "\\*",
    "1.",
    "-",
    "*",
    "\u{4e2d}\u{6587}",
    "  - ",
    "---  ",
    "--- x",
    "----",
    "+++",
    // Definitions in containers, over several lines, with labels of every shape.
    "[x]: /x \"multi",
    "line title\"",
    "[a b]: /u",
    "[A  B]: /v 'tit'",
    "[a",
    "b]: /ml",
    "> > [x]: /nested",
    "-\t[x]: /tabbed",
    "- [a b]: /listed",
    "[^q r]: spaced note",
    "-\t[^q r]: tabbed note",
    "[^\u{e9}]: note \u{e9}",
    "[^a\\]b]: odd label",
    "Use [x], [a b], [A  B], [^q r], [^Q   R], [^\u{c9}] and [^a\\]b].",
    "| [^q r] | [x] |",
    // Blank lines of every kind, fences and metadata in odd places.
    "\u{c}",
    "\u{b}",
    "\t\t",
    "---\u{c}",
    "---\u{b}",
    "  ```js",
    "   ~~~",
    "     ```",
    "   1. item",
    "  - [ ] task",
    // List items of every shape, at the top level and below it.
    "* star",
    "2. two",
    "3) three",
    "- [x] done",
    " - indented marker",
    "  continuation",
    "* * *",
    "- - -",
    "- ---",
    "10) ten",
    "-\titem after a tab",
    "1234567890. too long",
    "-",
    "- ",
];

fn fuzz_doc(rng: &mut Rng) -> String {
    let mut src = String::new();
    for _ in 0..rng.below(120) + 20 {
        src.push_str(FUZZ_LINES[rng.below(FUZZ_LINES.len())]);
        src.push('\n');
    }
    src
}

/// Multi-line constructs, several with blank lines or column-0 text inside.
const FUZZ_BLOCKS: &[&str] = &[
    "A paragraph with [foo], [bar][baz] and a note[^a] and another[^b].\nSecond line.",
    "# Heading one",
    "## Heading *two*",
    "Setext heading\n==============",
    "- a\n- b\n\n- c",
    "1. one\n2. two\n\n   para in item\n3. three",
    "- item\n\n  ```\n  code\n\n  more\n  ```\n- next",
    "- list\n  - nested\n\n    deeper para\n\n- again",
    "> quote\n>\n> more\n\n> second quote",
    "> [!NOTE]\n> alert text\n>\n> - item",
    "```\ncode\n\nstill code\n\n# not a heading\n\ntext\n```",
    "~~~rust\nfn main() {}\n\n~~~",
    "````\n```\nnested\n```\n\n````",
    " ```\nindented fence\n\nstays code\n ```",
    "<!--\ncomment\n\nwith a blank line\n-->",
    "<!-- one line -->",
    "<pre>\npre text\n\nafter blank\n</pre>",
    "<script>\nvar x = 1;\n\nvar y = 2;\n</script>",
    "<div>\nhtml *block*\n</div>",
    "<details>\n<summary>More</summary>\n\nInner *markdown*\n\n</details>",
    "<p align=\"center\">\n  <img src=\"a.png\" alt=\"A\">\n</p>",
    "| a | b |\n|---|:-:|\n| 1 | 2 |\n| 3 | 4 |",
    "| a |\n|---|\n| only |",
    "[foo]: /url \"title\"\n[bar]: <b z>",
    "[baz]: /baz",
    "[^a]: note a\n\n    second paragraph of the note",
    "[^b]: note *b*",
    "    indented code\n\n    more code",
    "***",
    "---",
    "---\ntitle: x\n\nkey: y\n---",
    "A line\nwith a hard break  \nand more",
    "Text with `code` and <b>inline</b> html &amp; entities.",
    "1) paren list\n2) second",
    "* star\n\n* loose",
    // Fences in list items (closed or not) and other places the scan can't be sure about.
    "- item\n\n  ```\n  code\n  ```",
    "- item\n\n  ```\n  code\n\npara after an unclosed item fence",
    "- item\n\n  ```\n  code\n\nfree text\n\n  ```\n  top-level code\n\n  more\n  ```",
    "1. a\n   - b\n\n     ```\n     x\n     ```\n\n   c",
    "1. one\n\n   ~~~\n   tilde\n\n   ~~~~\n\n2. two",
    "<div>\n```\nhtml content\n```\n</div>",
    "<div>\n  ```\n</div>",
    // Metadata with form feed and vertical tab lines.
    "---\n\u{c}\n<pre>\n---\n\n# Heading\n**bold**\n</pre>",
    "---\u{c}\ntitle: x\n\nk: v\n---",
    "---\u{b}\nnot: metadata\n\n---",
    "text\n---\nmore: text\n\n---",
    // Definitions: in containers, over several lines, odd labels.
    "> [x]: /q\n> \"title\"",
    "- [x]: /l\n\n  - [y]: /m",
    "[a\nb]: /ml \"t\n2\"",
    "[x]:\n/dest\n\"title\"",
    "-\t[x]: /tab",
    "Use [x], [y], [a b], [A  B] and ![i][x].",
    "[A B]: /case\n[long label with several words in it]: /long",
    // Footnotes with labels of every shape, in containers, tables and at several cases.
    "Use[^q r], [^Q   R], [^\u{c9}], [^a\\]b] and [^long label here].",
    "[^q r]: spaced\n\n    para",
    "[^\u{e9}]: accented\n[^a\\]b]: odd\n[^LONG  label here]: long",
    "-\t[^q r]: tab",
    "> [^q r]: quoted\n>\n> more",
    "| h |\n|---|\n| [^q r] |",
    "| a | b |\n|---|---|\n| 1 | [^Q R] |\n\t\n| 2 | 3 |",
    "[^a]: outer\n\n    > [^b]: inner\n    >\n    > more",
    "[^a]: x\n    - [^b]: y\n    - [^c]: z",
    "[^q r]: first\n\n[^Q  R]: second duplicate",
    // Whitespace the scan has to see as pulldown-cmark does.
    "| a |\n|---|\n| 1 |\n\u{c}\n| 2 |\n| 3 |",
    "| a |\n|---|\n| 1 |\n\u{b}\n| 2 |\n| 3 |",
    // Lists that other lists, text and blocks follow without a gap.
    "- a\n- b\ntext right after\n- c",
    "1. a\n2. b\n> quote after\n3. c",
    "- a\n  - b\n- c\n\n  - d\n- e",
    "* a\n* b\n\n+ c\n+ d\n\n- e\n- f",
    "1. a\n2. b\n\n1) c\n2) d",
];

/// A document of blocks, usually separated by a blank line (sometimes by none), maybe with front
/// matter first.
fn block_doc(rng: &mut Rng) -> String {
    let mut src = String::new();
    if rng.below(4) == 0 {
        src.push_str("---\ntitle: Doc\n\ntags: [a]\n---\n\n");
    }
    for _ in 0..rng.below(60) + 10 {
        src.push_str(FUZZ_BLOCKS[rng.below(FUZZ_BLOCKS.len())]);
        src.push_str(if rng.below(10) == 0 { "\n" } else { "\n\n" });
    }
    src
}

/// A document of blocks with a few lines inserted, dropped or doubled at random places, which
/// leaves constructs half open and cuts in the middle of them.
fn mutated_doc(rng: &mut Rng) -> String {
    let src = block_doc(rng);
    let mut lines: Vec<&str> = src.split_inclusive('\n').collect();
    for _ in 0..rng.below(6) {
        let at = rng.below(lines.len() + 1);
        match rng.below(3) {
            0 => lines.insert(at, FUZZ_LINES[rng.below(FUZZ_LINES.len())]),
            1 if at < lines.len() => {
                lines.remove(at);
            }
            _ if at < lines.len() => lines.insert(at, lines[at]),
            _ => {}
        }
    }
    lines
        .iter()
        .map(|l| {
            if l.ends_with('\n') {
                l.to_string()
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

/// Items of the lists of [`list_doc`], after their marker; the lines after the first are indented
/// to the item's content.
const LIST_ITEMS: &[&str] = &[
    "item",
    "item with `code`, *emphasis* and [a link](/u)",
    "[ ] task",
    "[x] done",
    "[foo] and [baz][foo]",
    "text\ncontinued",
    "item\n- nested\n  - deeper",
    "item\n1. nested ordered\n2. second",
    "item\n\npara in item",
    "item\n\n> quote\n>\n> more",
    "> quote in item",
    "item\n```\ncode\n```",
    "item\n\n```\ncode\n\nwith a blank\n```",
    "~~~\ntilde fence\n\n~~~",
    "    code after the marker",
    "# heading in item",
    "item\n---",
    "<div>\nhtml\n</div>",
    "<!-- comment\n\nacross a gap -->",
    "| a |\n|---|\n| 1 |",
    "[^a] a note reference",
    "[^n]: a note in an item\n\nnote text",
    "[d]: /dest \"title\"",
    "[e]: /dest2\n\"title on the next line\"",
    "[q]: /q \"title\nover a blank\n\nline\"",
    "![img](a.png)",
    "",
];

/// Lines that end or interrupt a list, or only look like they continue it.
const LIST_BREAKS: &[&str] = &[
    "text",
    "lazy",
    " one space",
    "> quote",
    "# Heading",
    "---",
    "***",
    "<div>",
    "<!-- c -->",
    "```",
    "~~~",
    "    indented code",
    "| a |\n|---|\n| 1 |",
    "[foo]: /url",
    "[^a]: note",
    "1234567890. not a marker",
];

/// Documents with long lists: bullets of each kind and ordered lists (starting anywhere, numbers
/// that repeat, count up or are arbitrary, either delimiter) of items of every shape, tight and
/// loose, with other lists, text and blocks in between.
fn list_doc(rng: &mut Rng) -> String {
    let mut src = String::new();
    for _ in 0..rng.below(5) + 1 {
        if rng.below(4) == 0 {
            src.push_str(FUZZ_BLOCKS[rng.below(FUZZ_BLOCKS.len())]);
            src.push_str("\n\n");
            continue;
        }
        let kind = rng.below(5);
        let bullet = ["-", "*", "+"][kind.min(2)];
        let (ordered, delim) = (kind >= 3, if kind == 3 { '.' } else { ')' });
        let (mut number, step) = (rng.below(12), rng.below(3));
        // How often a blank line separates two items (0 = never: a tight list).
        let gap = [0, 0, 1, 2, 3, 5][rng.below(6)];
        for item in 0..rng.below(40) + 3 {
            // Now and then an item of another list: another bullet, another delimiter.
            let other = rng.below(25) == 0;
            let mark = if ordered {
                let delim = if other == (delim == ')') { '.' } else { ')' };
                format!("{number}{delim}")
            } else if other {
                ["-", "*", "+"][rng.below(3)].to_owned()
            } else {
                bullet.to_owned()
            };
            number = [number + step, rng.below(100)][usize::from(rng.below(8) == 0)];
            // Spaces after the marker; five or more make the content an indented code block.
            let pad = [1, 1, 1, 1, 2, 3, 4, 5][rng.below(8)];
            let indent = " ".repeat(mark.len() + if pad > 4 { 1 } else { pad });
            let text = LIST_ITEMS[rng.below(LIST_ITEMS.len())];
            let body = text.replace('\n', &format!("\n{indent}"));
            let tab = if rng.below(30) == 0 { "\t" } else { "" };
            src.push_str(&format!("{mark}{}{tab}{body}\n", " ".repeat(pad)));
            if gap > 0 && item % gap == gap - 1 {
                src.push('\n');
            }
            if rng.below(40) == 0 {
                src.push_str(LIST_BREAKS[rng.below(LIST_BREAKS.len())]);
                src.push('\n');
            }
        }
        src.push_str(["\n", "\n\nAfter the list.\n\n", "\n"][rng.below(3)]);
    }
    src
}

/// `VOMD_FUZZ=<n>` runs `n` random documents of each kind instead of the default.
fn fuzz_runs() -> usize {
    std::env::var("VOMD_FUZZ")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(600)
}

#[test]
fn segmented_parse_matches_on_random_documents() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ fuzz_runs() as u64);
    let mut cuts = 0;
    for i in 0..fuzz_runs() {
        let src = block_doc(&mut rng);
        for budget in [1, 150] {
            cuts += assert_same(&format!("block doc {i}\n{src}"), &src, budget) - 1;
        }
    }
    assert!(cuts > fuzz_runs() * 2, "only {cuts} cuts were exercised");
    // Random lines are mostly unsplittable, but a cut must never change the result.
    for i in 0..fuzz_runs() {
        let src = fuzz_doc(&mut rng);
        for budget in [1, 40] {
            assert_same(&format!("line doc {i}\n{src}"), &src, budget);
        }
    }
    for i in 0..fuzz_runs() {
        let src = mutated_doc(&mut rng);
        for budget in [1, 150] {
            assert_same(&format!("mutated doc {i}\n{src}"), &src, budget);
        }
    }
    // Long lists are cut between items, and every such cut joins up again.
    LIST_MISFITS.with(|n| n.set(0));
    let mut joins = 0;
    for i in 0..fuzz_runs() {
        let src = list_doc(&mut rng);
        for budget in [1, 60, 400] {
            assert_same(&format!("list doc {i}\n{src}"), &src, budget);
            assert_eq!(
                LIST_MISFITS.with(|n| n.get()),
                0,
                "list doc {i}\n{src}\n@ {budget}: the builder couldn't join the cuts in a list"
            );
            joins += list_joins(&src, budget);
        }
    }
    assert!(joins > fuzz_runs() * 3, "only {joins} list cuts");
}

#[test]
fn nested_images_keep_the_whole_outer_alt_text() {
    let doc = Doc::parse("![outer ![inner](inner.png) end](outer.png)");
    check_invariants(&doc);
    // Only the outer image is an image; the inner one contributes its alt text.
    assert_eq!(doc.images.len(), 1);
    assert_eq!(doc.images[0].src, "outer.png");
    let texts: Vec<_> = doc.spans.iter().map(|s| doc.span_text(s)).collect();
    assert_eq!(texts, ["outer inner end"]);
    assert_eq!(doc.spans[0].style & style::IMAGE, style::IMAGE);
    // Deeper nesting, and text after the image stays outside it.
    let doc = Doc::parse("![a ![b ![c](c.png) d](b.png) e](a.png) after");
    check_invariants(&doc);
    let texts: Vec<_> = doc.spans.iter().map(|s| doc.span_text(s)).collect();
    assert_eq!(texts, ["a b c d e", " after"]);
    assert_eq!(doc.images.len(), 1);
}

/// Every `#fragment` link points at an anchor, and anchor ids are unique.
fn check_anchors(doc: &Doc) {
    let mut seen = HashSet::new();
    for (id, _) in &doc.anchors {
        assert!(seen.insert(id.as_str()), "duplicate anchor {id:?}");
    }
    for link in doc.links.iter().filter(|l| l.starts_with('#')) {
        assert!(seen.contains(&link[1..]), "dangling link {link:?}");
    }
}

#[test]
fn footnote_anchors_never_collide_with_heading_slugs() {
    let doc = Doc::parse("# fnref-n\n\nref[^n]\n\n[^n]: def\n");
    check_invariants(&doc);
    check_anchors(&doc);
    // The heading keeps its slug; the footnote's back-link anchor takes the next free id, and
    // the link back to it follows.
    assert_eq!(doc.headings[0].slug, "fnref-n");
    let anchor = |id: &str| doc.anchors.iter().find(|(s, _)| s == id).map(|(_, b)| *b);
    assert_eq!(anchor("fnref-n"), Some(0));
    assert_eq!(anchor("fnref-n-1"), Some(1));
    assert!(doc.links.iter().any(|l| l == "#fnref-n-1"));
    assert!(doc.links.iter().any(|l| l == "#fn-n"));
    // The same for the definition's own anchor, and when the footnote comes first.
    let doc = Doc::parse("# fn-n\n\nref[^n]\n\n[^n]: def\n");
    check_anchors(&doc);
    assert_eq!(doc.headings[0].slug, "fn-n");
    assert!(doc.anchors.iter().any(|(s, _)| s == "fn-n-1"));
    assert!(doc.links.iter().any(|l| l == "#fn-n-1"));
    let doc = Doc::parse("ref[^n]\n\n[^n]: def\n\n# fnref-n\n\n# fn-n\n");
    check_invariants(&doc);
    check_anchors(&doc);
    let slugs: Vec<_> = doc.headings.iter().map(|h| h.slug.as_str()).collect();
    assert_eq!(slugs, ["fnref-n", "fn-n"]);
    // Several colliding labels and headings still give unique ids.
    let doc = Doc::parse(
        "# fnref-a\n# fnref-a-1\n# fn-b\n\nx[^a][^b]\n\n[^a]: A\n[^b]: B\n\n# fnref-b\n# fn-a\n",
    );
    check_invariants(&doc);
    check_anchors(&doc);
}

#[test]
fn references_inside_footnote_definitions_have_return_anchors() {
    let doc = Doc::parse("a[^a]\n\n[^a]: ref[^b]\n\n[^b]: def\n");
    check_invariants(&doc);
    check_anchors(&doc);
    let anchor = |id: &str| doc.anchors.iter().find(|(s, _)| s == id).map(|(_, b)| *b);
    // The reference to `b` is in note a's block, in the footnote section.
    let at = anchor("fnref-b").expect("an anchor for the reference inside note a") as usize;
    assert_eq!(anchor("fn-a").map(|b| b as usize), Some(at));
    assert!(doc.blocks[at].marker == Marker::Footnote(1));
    assert!(
        doc.block_spans(&doc.blocks[at])
            .iter()
            .any(|s| s.style & style::FOOTREF != 0)
    );
    // The first reference wins: one in the text before beats one in a definition.
    let doc = Doc::parse("x[^b] a[^a]\n\n[^a]: ref[^b]\n\n[^b]: def\n");
    check_anchors(&doc);
    let b = doc.anchors.iter().find(|(s, _)| s == "fnref-b").unwrap().1;
    assert_eq!(b, 0);
    // And in the other order, the definition's reference wins.
    let doc = Doc::parse("a[^a]\n\n[^a]: ref[^b]\n\n[^b]: def\n\nlate[^b]\n");
    check_anchors(&doc);
    let b = doc.anchors.iter().find(|(s, _)| s == "fnref-b").unwrap().1 as usize;
    assert!(doc.blocks[b].marker == Marker::Footnote(1));
    // Many notes in a chain, listed in reference order, keep their anchors on the right blocks.
    let doc = Doc::parse("r[^c]\n\n[^a]: end\n[^b]: to a[^a]\n[^c]: to b[^b] and **bold**[^a]\n");
    check_invariants(&doc);
    check_anchors(&doc);
    for label in ["a", "b"] {
        let at = doc
            .anchors
            .iter()
            .find(|(s, _)| s == &format!("fnref-{label}"))
            .unwrap()
            .1 as usize;
        assert!(
            doc.block_spans(&doc.blocks[at])
                .iter()
                .any(|s| s.style & style::FOOTREF != 0),
            "fnref-{label}"
        );
    }
}

#[test]
fn footnotes_ending_in_a_table_or_code_get_their_own_backlink_paragraph() {
    for end in [
        "    | h |\n    |---|\n    | c |\n",
        "    ```\n    code\n    ```\n",
        "    ***\n",
    ] {
        let src = format!("a[^t]\n\n[^t]:\n{end}");
        let doc = Doc::parse(&src);
        check_invariants(&doc);
        check_anchors(&doc);
        let last = doc.blocks.last().unwrap();
        assert_eq!(last.kind, BlockKind::Paragraph, "{src:?}");
        let spans = doc.block_spans(last);
        assert_eq!(spans.len(), 1, "{src:?}");
        assert_eq!(doc.span_text(&spans[0]), "↩");
        assert_eq!(doc.links[spans[0].link as usize - 1], "#fnref-t");
        // The definition's last block was left alone: no span was glued onto it.
        let before = &doc.blocks[doc.blocks.len() - 2];
        assert!(
            doc.block_spans(before)
                .iter()
                .all(|s| doc.span_text(s) != " ↩")
        );
    }
    // Paragraphs still take the back-link in their own text.
    let doc = Doc::parse("a[^t]\n\n[^t]: text\n");
    let last = doc.blocks.last().unwrap();
    assert!(
        doc.block_spans(last)
            .last()
            .is_some_and(|s| doc.span_text(s) == " ↩")
    );
}

#[test]
fn wide_rows_are_split_within_an_aggregate_byte_budget() {
    // 512 columns of 1 KB each: every cell used to get at least 256 bytes of every fragment, so a
    // fragment held 128 KB.
    const COLS: usize = 512;
    let word = "word ".repeat(200);
    let head: String = (0..COLS).map(|c| format!("| h{c} ")).collect::<String>() + "|\n";
    let delim = "|---".repeat(COLS) + "|\n";
    let row: String = (0..COLS).map(|_| format!("| {word} ")).collect::<String>() + "|\n";
    let doc = Doc::parse(&format!("{head}{delim}{row}"));
    check_invariants(&doc);
    let widest = doc
        .blocks
        .iter()
        .map(|b| block_bytes(&doc, b))
        .max()
        .unwrap();
    assert!(widest <= MAX_BLOCK_BYTES, "a fragment holds {widest} bytes");
    // The row still holds all of its text, piece by piece.
    assert!(doc.blocks.len() > 30, "{} blocks", doc.blocks.len());
    let total: usize = doc.blocks.iter().map(|b| block_bytes(&doc, b)).sum();
    let header: usize = (0..COLS).map(|c| format!("h{c}").len()).sum();
    assert_eq!(total, header + COLS * word.trim_end().len());
    let first = &doc.blocks[1];
    assert_eq!(cell_texts(&doc, first).len(), COLS);
    // With more columns than the budget has bytes, the floor keeps every cell moving.
    let doc = Doc::parse(&format!(
        "{}{}| {} |\n",
        "|a".repeat(2000) + "|\n",
        "|-".repeat(2000) + "|\n",
        "x ".repeat(100)
            .repeat(2000)
            .replace("x x ", "|")
            .trim_end_matches('|')
    ));
    check_invariants(&doc);
}

#[test]
fn exhausted_cells_cost_nothing_in_later_fragments() {
    // One long cell and 511 one-character ones: later fragments used to store 511 empty cells
    // each, about 10 million ranges for a 5 MB table.
    const COLS: usize = 512;
    let long = "word ".repeat(120_000);
    let head: String = (0..COLS).map(|c| format!("| h{c} ")).collect::<String>() + "|\n";
    let delim = "|---".repeat(COLS) + "|\n";
    let row = format!("| {long} {}|\n", "| x ".repeat(COLS - 1));
    let doc = Doc::parse(&format!("{head}{delim}{row}"));
    check_invariants(&doc);
    let fragments = doc.blocks.len();
    assert!(fragments > 30, "{fragments} blocks");
    // The header, plus the row's first fragment with every cell, plus one cell per later
    // fragment.
    assert!(
        doc.cells.len() <= COLS + COLS + fragments,
        "{} cells for {fragments} blocks",
        doc.cells.len()
    );
    // Block 0 is the header, block 1 the row's first fragment.
    let later = &doc.blocks[2];
    assert!(later.joined);
    let stored: Vec<_> = doc.row_cells(later).map(|(col, _)| col).collect();
    assert_eq!(stored, [0]);
    // They still read as full rows.
    assert_eq!(cell_texts(&doc, later).len(), COLS);
    // The long cell is complete.
    let text: String = doc.blocks[1..]
        .iter()
        .map(|b| cell_texts(&doc, b)[0].clone())
        .collect();
    assert_eq!(text, long.trim_end());
    // Short cells sit in the first fragment only.
    assert_eq!(cell_texts(&doc, &doc.blocks[1])[1], "x");
    assert_eq!(cell_texts(&doc, &doc.blocks[2])[1], "");
}

/// Some paragraphs, to put distance between two parts of a document.
fn filler(paragraphs: usize) -> String {
    "Some filler text to make the document longer.\n\n".repeat(paragraphs)
}

fn footnote_refs(doc: &Doc) -> usize {
    doc.spans
        .iter()
        .filter(|s| s.style & style::FOOTREF != 0)
        .count()
}

#[test]
fn definitions_anywhere_resolve_across_segments() {
    let filler = filler(30);
    let long = "long label ".repeat(90);
    let cases = [
        ("[x]".to_owned(), "> [x]: /x\n".to_owned()),
        ("[x]".to_owned(), "[x]: /x \"a\nb\"\n".to_owned()),
        ("[x]".to_owned(), "- [x]: /x\n".to_owned()),
        ("[x]".to_owned(), "- - [x]: /x\n".to_owned()),
        ("[x]".to_owned(), "> > [x]: /x\n".to_owned()),
        ("[x]".to_owned(), "1. > [x]: /x\n".to_owned()),
        ("[x]".to_owned(), "-\t[x]: /x\n".to_owned()),
        ("[x]".to_owned(), ">\t[x]: /x\n".to_owned()),
        ("[x]".to_owned(), "[x]:\n/x\n".to_owned()),
        ("[x]".to_owned(), "  [x]: </x> 'T'\n".to_owned()),
        ("[a b]".to_owned(), "[a\nb]: /x\n".to_owned()),
        ("[a  b]".to_owned(), "[A B]: /x\n".to_owned()),
        ("[a, b.]".to_owned(), "[A, B.]: /x\n".to_owned()),
        ("[\u{e9}]".to_owned(), "[\u{c9}]: /x\n".to_owned()),
        (
            format!("[{}]", long.trim_end()),
            format!("[{}]: /x\n", long.to_uppercase().trim_end()),
        ),
        ("![x]".to_owned(), "[x]: /x\n".to_owned()),
    ];
    for (i, (usage, def)) in cases.iter().enumerate() {
        for src in [
            format!("{usage}\n\n{filler}{def}"),
            format!("{def}\n{filler}{usage}\n"),
        ] {
            let name = format!("definition {i}: {def:?}");
            assert!(assert_same(&name, &src, 400) > 3, "{name}");
            let doc = Doc::parse_segments(&src, 400).unwrap();
            let whole = Doc::parse_whole(&src);
            let found = whole.links.len() + whole.images.len();
            assert_eq!(found, 1, "{name}: the single parser resolves the use");
            assert_eq!(doc.links.len() + doc.images.len(), 1, "{name}");
        }
    }
    // A big document, past the threshold for segmenting by itself (the definition in a quote).
    let src = format!(
        "[x]\n\n{}> [x]: /x\n",
        "filler paragraph\n\n".repeat(65_000)
    );
    assert!(src.len() > SEGMENT_THRESHOLD);
    assert_eq!(Doc::parse(&src).links, ["/x"]);
}

#[test]
fn conflicting_definitions_in_two_segments_fall_back_to_one_parser() {
    let filler = filler(30);
    // The first definition of a label counts; a later segment's own would shadow it.
    let src = format!("[x]\n\n{filler}[x]: /first\n\n{filler}[X]: /second\n");
    assert!(segment::plan(&src, 400).is_some());
    assert!(Doc::parse_segments(&src, 400).is_none());
    let doc = Doc::parse_whole(&src);
    assert_eq!(doc.links, ["/first"]);
    // The same definition twice is no conflict.
    let src = format!("[x]\n\n{filler}[x]: /same\n\n{filler}[X]: /same\n");
    assert!(assert_same("same definition twice", &src, 400) > 5);
}

#[test]
fn footnote_labels_of_every_shape_resolve_across_segments() {
    let filler = filler(30);
    let long = "long label ".repeat(15);
    let cases = [
        ("Ref[^a b]".to_owned(), "[^a b]: note\n".to_owned()),
        ("Ref[^A]".to_owned(), "[^a]: note\n".to_owned()),
        ("Ref[^a  b]".to_owned(), "[^A B]: note\n".to_owned()),
        (
            format!("Ref[^{}]", long.trim_end()),
            format!("[^{}]: note\n", long.to_uppercase().trim_end()),
        ),
        ("Ref[^a]".to_owned(), "-\t[^a]: note\n".to_owned()),
        ("Ref[^a]".to_owned(), "> [^a]: note\n".to_owned()),
        ("Ref[^a]".to_owned(), "1. [^a]: note\n".to_owned()),
        ("Ref[^a:b]".to_owned(), "[^a:b]: note\n".to_owned()),
        ("Ref[^a\\]b]".to_owned(), "[^a\\]b]: note\n".to_owned()),
        ("Ref[^\u{e9}]".to_owned(), "[^\u{c9}]: note\n".to_owned()),
        ("Ref[^a\tb]".to_owned(), "[^a b]: note\n".to_owned()),
        // In a table cell `\|` stands for `|`.
        (
            "| h |\n|---|\n| x[^a\\|b] |\n".to_owned(),
            "[^a|b]: note\n".to_owned(),
        ),
    ];
    for (i, (usage, def)) in cases.iter().enumerate() {
        for src in [
            format!("{usage}\n{filler}{def}"),
            format!("{def}\n{filler}{usage}\n"),
        ] {
            let name = format!("footnote {i}: {usage:?} {def:?}");
            assert!(assert_same(&name, &src, 400) > 3, "{name}");
            let whole = Doc::parse_whole(&src);
            assert_eq!(footnote_refs(&whole), 1, "{name}: a real reference");
            let parts = Doc::parse_segments(&src, 400).unwrap();
            assert_eq!(footnote_refs(&parts), 1, "{name}");
        }
    }
}

#[test]
fn footnote_reference_scan_is_linear() {
    // Inputs where a scan that looks for the closing bracket of every `[^` from scratch would take
    // quadratic time: the work is counted, not timed.
    let units = ["[^", "[^a ", "[^a\\[^", "[^\\[^\\[^", "[^a b\n", "[^a][^b"];
    for unit in units {
        let text = unit.repeat(1_000_000 / unit.len());
        let scanned = segment::footnote_references(&text, |_| ());
        assert!(
            scanned <= text.len() + 8,
            "{unit:?}: {scanned} bytes looked at in {}",
            text.len()
        );
    }
    // One label that runs on for the whole text, with escaped brackets that look like new labels.
    let text = format!("[^{}", "\\[^".repeat(300_000));
    let scanned = segment::footnote_references(&text, |_| panic!("not a label"));
    assert!(scanned <= text.len() + 8, "{scanned} of {}", text.len());
    // A label is found where there is one, normalized as pulldown-cmark does.
    let mut found = Vec::new();
    segment::footnote_references("a [^ Ab \t c ] b [^x\\|y] [^] [^z", |l| found.push(l));
    assert_eq!(found, ["Ab c", "x\\|y", "x|y"]);
    // And through the whole parse, a big text of unmatched `[^` goes the segmented way.
    let src = format!(
        "{}Note[^n].\n\n[^n]: A note.\n",
        "text [^ and [^a more [^\n\n".repeat(50_000)
    );
    assert!(src.len() > SEGMENT_THRESHOLD);
    assert!(segment_count(&src, SEGMENT_BYTES) > 5);
    let doc = Doc::parse(&src);
    check_invariants(&doc);
    assert_eq!(footnote_refs(&doc), 1);
}

#[test]
fn a_list_fence_does_not_stop_segmentation() {
    let table = big_table(3000);
    let prefixes = [
        "- item\n\n  ```\n  code\n  ```\n\n",
        "1. item\n\n   ~~~\n   code\n\n   more\n   ~~~\n\n",
        "- a\n  - b\n\n    ```\n    c\n    ```\n\n",
        "> quote\n\n- item\n\n  ```rust\n  fn main() {}\n  ```\n\n",
        "- item\n\n  ```\n  code\n  ```\n\n- again\n\n  ```\n  more\n  ```\n\ntext\n\n",
    ];
    for prefix in prefixes {
        let src = format!("{prefix}{table}\nafter\n");
        let n = assert_same(prefix, &src, 4096);
        assert!(n > 5, "{prefix:?}: {n} segments");
        // The rest of the text is cut as before: no segment grows beyond a budget's worth.
        let plan = segment::plan(&src, 4096).unwrap();
        let widest = plan.segments.iter().map(|s| s.range.len()).max().unwrap();
        assert!(widest < 2 * 4096, "{prefix:?}: a segment of {widest} bytes");
        // And in the middle of a text.
        let filler = filler(100);
        let src = format!("{filler}{prefix}{filler}{table}");
        assert!(assert_same(prefix, &src, 2000) > 8, "{prefix:?}");
    }
    // A fence in a list item that is never closed ends with the item: the text goes on.
    let src = format!("- item\n\n  ```\n  code\n\nfree text\n\n{table}");
    assert_same("unclosed item fence", &src, 4096);
    // A closing line that really opens a fence, after the item ended.
    let src = format!(
        "- item\n\n  ```\n  code\n\nfree text\n\n  ```\n  real top-level code\n\n  more\n  ```\n\n{table}"
    );
    // (Both readings of the first fence line stay possible, so there are no cuts to count.)
    assert_same("fence after the item", &src, 4096);
}

/// Kinds of the blocks, one letter each: P(aragraph), H(eading), R(ule).
fn kinds(doc: &Doc) -> String {
    doc.blocks
        .iter()
        .map(|b| match b.kind {
            BlockKind::Paragraph => 'P',
            BlockKind::Heading(_) => 'H',
            BlockKind::Rule => 'R',
            _ => '?',
        })
        .collect()
}

#[test]
fn unclosed_metadata_candidates_parse_in_linear_time() {
    // pulldown-cmark's metadata option reads forward for a closing line at every `---` it sees
    // (quadratic when there is none), so front matter is only looked for at the start of the text
    // and the option is off. Every `---\t` after the intro is a rule, and each `x` a paragraph.
    for kb in [64, 128, 256] {
        let n = kb * 1024 / 8;
        let src = format!("intro\n\n{}", "---\t\nx\n\n".repeat(n));
        let t = std::time::Instant::now();
        let doc = Doc::parse(&src);
        assert_eq!(doc.blocks.len(), 1 + 2 * n, "{kb} KB");
        assert_eq!(kinds(&doc), format!("P{}", "RP".repeat(n)), "{kb} KB");
        assert!(
            t.elapsed().as_secs_f64() < 3.0,
            "{kb} KB: {:?}",
            t.elapsed()
        );
    }
    // At the very start the first `---\t` opens front matter that the second closes.
    let doc = Doc::parse(&"---\t\nx\n\n".repeat(1000));
    assert_eq!(kinds(&doc), format!("P{}", "RP".repeat(998)));
    // With a closing line at the end the metadata option would swallow all of it as one block.
    let src = format!("intro\n\n{}...\n", "---\t\nx\n\n".repeat(1000));
    let doc = Doc::parse(&src);
    assert_eq!(doc.blocks.len(), 1 + 2000 + 1);
    assert_eq!(doc.text.matches('x').count(), 1000);
}

#[test]
fn front_matter_is_found_only_at_the_start() {
    let hidden = [
        "---\ntitle: x\n---\n# T\n",
        "---  \t\ntitle: x\n---\t \n# T\n",
        "---\ntitle: x\n...\n# T\n",
        "---\r\ntitle: x\r\n\r\nk: v\r\n---\r\n# T\r\n",
        "\u{feff}---\ntitle: x\n---\n# T\n",
        "---\ntitle: x\n---\n\n# T\n",
    ];
    for src in hidden {
        let doc = Doc::parse(src);
        check_invariants(&doc);
        assert_eq!(kinds(&doc), "H", "{src:?}");
        assert_eq!(doc.text, "T", "{src:?}");
        assert!(front_matter_len(src) > 0, "{src:?}");
        assert!(
            src[front_matter_len(src)..].trim_start().starts_with('#'),
            "{src:?}"
        );
    }
    // Front matter that runs to the end of the text, with and without a final newline.
    for src in ["---\na: b\n---", "---\na: b\n---\n", "---\na: b\n...\n"] {
        assert_eq!(front_matter_len(src), src.len(), "{src:?}");
        assert!(Doc::parse(src).blocks.is_empty());
    }
    // Not front matter: no closing line, a blank or closing line right after the opener, text on
    // the opener's line, indentation, a BOM-less lookalike after other text, or a longer rule.
    let shown = [
        ("---\ntitle: x\n", "RP"),
        ("---\n\ntitle: x\n---\n", "RH"),
        ("---\n---\ntext\n", "RRP"),
        ("--- x\ntitle: x\n---\n", "H"),
        (" ---\ntitle: x\n---\n", "RH"),
        ("----\ntitle: x\n---\n", "RH"),
        ("\n---\ntitle: x\n---\n", "RH"),
        ("text\n\n---\ntitle: x\n\nmore\n---\n", "PRPH"),
        ("text\n\n---\ntitle: x\n...\n", "PRP"),
    ];
    for (src, expected) in shown {
        let doc = Doc::parse(src);
        check_invariants(&doc);
        assert_eq!(front_matter_len(src), 0, "{src:?}");
        assert_eq!(kinds(&doc), expected, "{src:?}");
        assert!(
            doc.text.contains("title") || expected == "RRP" || expected == "H",
            "{src:?}"
        );
    }
}

#[test]
fn segmented_parse_keeps_metadata_lookalikes_visible() {
    // `---` and `...` lines in quotes and lists, with distant lines that could close them: none
    // of it is metadata, so one parser and segments both show it all.
    let filler = filler(20);
    let regions = [
        "> ---\n> secret: value\n>\n> ---",
        "> ---\n> secret: value\n> ...",
        "- ---\n  secret: value\n\n  ---",
        "1. ---\n   secret: value\n   ...",
        "text\n---\nsecret: value\n\n---",
    ];
    for (i, region) in regions.iter().enumerate() {
        let src = format!("{filler}{region}\n\n{filler}---\nclose: me\n...\n\n{filler}---\n");
        for budget in [1, 90, 700] {
            assert_same(&format!("region {i} @ {budget}"), &src, budget);
        }
        let doc = Doc::parse_whole(&src);
        assert!(doc.text.contains("secret"), "region {i}");
        assert!(doc.text.contains("close"), "region {i}");
    }
    // As a text over the production threshold, where `Doc::parse` really segments it.
    let src = format!(
        "{}> ---\n> secret: value\n>\n> ---\n\n{}---\nclose: me\n...\n",
        "filler paragraph\n\n".repeat(60_000),
        "filler paragraph\n\n".repeat(60_000)
    );
    assert!(src.len() > SEGMENT_THRESHOLD);
    let doc = Doc::parse(&src);
    assert!(doc == Doc::parse_whole(&src));
    assert!(doc.text.contains("secret"));
    assert!(segment_count(&src, SEGMENT_BYTES) > 3);
}

#[test]
fn footnote_labels_match_case_insensitively() {
    insta::assert_snapshot!(parse("Ref[^A]\n\n[^a]: note\n"));
    for src in [
        "Ref[^A]\n\n[^a]: note\n",
        "Ref[^a] and [^A]\n\n[^A]: note\n",
        "[^a]: note\n\nRef[^A]\n",
        "x[^\u{c9}]\n\n[^\u{e9}]: n\n",
        "x[^a  b]\n\n[^A B]: n\n",
        "x[^STRASSE]\n\n[^strasse]: n\n",
    ] {
        let doc = Doc::parse(src);
        check_invariants(&doc);
        check_anchors(&doc);
        let notes = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.marker, Marker::Footnote(_)))
            .count();
        assert_eq!(notes, 1, "{src:?}");
        assert!(
            doc.spans
                .iter()
                .filter(|s| s.style & style::FOOTREF != 0)
                .all(|s| doc.span_text(s) == "1"),
            "{src:?}"
        );
        // Every anchor a link names exists, and the note is numbered 1.
        assert!(
            doc.blocks.iter().any(|b| b.marker == Marker::Footnote(1)),
            "{src:?}"
        );
    }
}

#[test]
fn duplicate_footnote_definitions_keep_the_first() {
    let doc = Doc::parse("a[^a]\n\n[^a]: first\n\n[^a]: second\n");
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(!doc.text.contains("second"));
    let notes = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.marker, Marker::Footnote(_)))
        .count();
    assert_eq!(notes, 1);
    // Case-insensitively, and from another segment.
    let doc = Doc::parse("a[^a]\n\n[^A]: first\n\n[^a]: second\n\nb[^A]\n");
    check_anchors(&doc);
    assert!(doc.text.contains("first") && !doc.text.contains("second"));
    let filler = filler(30);
    let src = format!("a[^a]\n\n[^a]: first\n\n{filler}[^A]: second\n\n{filler}z[^a]\n");
    assert!(assert_same("duplicate definitions", &src, 400) > 5);
    let doc = Doc::parse_segments(&src, 400).unwrap();
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(doc.text.contains("first") && !doc.text.contains("second"));
}

#[test]
fn definitions_inside_a_duplicate_definition_still_count() {
    // The second `[^a]` is dropped, but the `[^b]` in it is a definition of a label that is free.
    let src = "r[^a][^b]\n\n[^a]: first\n\n[^a]: second\n\n    > [^b]: inner\n";
    let doc = Doc::parse(src);
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(doc.text.contains("first") && doc.text.contains("inner"));
    assert!(!doc.text.contains("second"));
    for id in ["fn-a", "fn-b", "fnref-a", "fnref-b"] {
        assert!(doc.anchors.iter().any(|(s, _)| s == id), "{id}");
    }
    let notes = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.marker, Marker::Footnote(_)))
        .count();
    assert_eq!(notes, 2);
    // A label that is taken is dropped there too, and the text after the dropped definition and
    // inside it still reads as it did: a dropped nested duplicate, then a definition after it.
    let src = "r[^a][^b][^c]\n\n[^a]: first\n\n[^b]: first b\n\n[^a]: dup\n\n    [^b]: dup b\n\n    [^c]: inner c\n\n[^d]: after\n";
    let doc = Doc::parse(src);
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(doc.text.contains("inner c") && doc.text.contains("after"));
    assert!(!doc.text.contains("dup") && !doc.text.contains("dup b"));
    // Inside another note, and across segments.
    let doc =
        Doc::parse("r[^a][^c]\n\n[^a]: top\n\n    > [^a]: y\n    >\n    >     > [^c]: deep\n");
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(doc.text.contains("deep") && !doc.text.contains('y'));
    let filler = filler(30);
    let src = format!(
        "{filler}r[^a][^b]\n\n[^a]: first\n\n{filler}[^a]: second\n\n    > [^b]: inner\n\n{filler}"
    );
    assert!(assert_same("nested in a duplicate", &src, 400) > 5);
    let doc = Doc::parse_segments(&src, 400).unwrap();
    check_anchors(&doc);
    assert!(doc.text.contains("inner"));
}

/// The text of the block's spans, back-link arrows included.
fn block_text(doc: &Doc, block: &Block) -> String {
    doc.block_spans(block)
        .iter()
        .map(|s| doc.span_text(s))
        .collect()
}

#[test]
fn unreferenced_footnotes_have_no_back_link() {
    let doc = Doc::parse("[^a]: unused\n");
    check_invariants(&doc);
    check_anchors(&doc);
    assert!(
        doc.blocks
            .iter()
            .all(|b| !block_text(&doc, b).contains('↩'))
    );
    assert!(doc.links.iter().all(|l| !l.starts_with("#fnref")));
    assert!(doc.anchors.iter().any(|(id, _)| id == "fn-a"));
    assert!(doc.anchors.iter().all(|(id, _)| id != "fnref-a"));
    // Referenced notes keep theirs; unreferenced ones follow them, numbered after.
    let doc = Doc::parse("x[^b]\n\n[^a]: unused\n\n[^b]: used\n");
    check_invariants(&doc);
    check_anchors(&doc);
    let notes: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.marker, Marker::Footnote(_)))
        .map(|b| (b.marker, block_text(&doc, b)))
        .collect();
    assert_eq!(
        notes,
        [
            (Marker::Footnote(1), "used ↩".to_owned()),
            (Marker::Footnote(2), "unused".to_owned()),
        ]
    );
    // An unreferenced note that ends in a table, code or a rule has no paragraph of its own for
    // the arrow either.
    for end in [
        "    | h |\n    |---|\n    | c |\n",
        "    ```\n    code\n    ```\n",
        "    ***\n",
    ] {
        let doc = Doc::parse(&format!("[^t]:\n{end}"));
        check_invariants(&doc);
        check_anchors(&doc);
        assert!(
            doc.blocks
                .iter()
                .all(|b| !block_text(&doc, b).contains('↩')),
            "{end:?}"
        );
        let with = Doc::parse(&format!("a[^t]\n\n[^t]:\n{end}"));
        assert_eq!(with.blocks.len(), doc.blocks.len() + 2, "{end:?}");
    }
    // A reference only inside an unreferenced note's text counts as a reference.
    let doc = Doc::parse("[^a]: see[^b]\n\n[^b]: other\n");
    check_anchors(&doc);
}

#[test]
fn footnotes_are_numbered_by_reference_not_by_definition() {
    let doc = Doc::parse("[^b]: B\n\n[^a]: A\n\nref[^a] then[^b]\n");
    check_invariants(&doc);
    check_anchors(&doc);
    let refs: Vec<_> = doc
        .spans
        .iter()
        .filter(|s| s.style & style::FOOTREF != 0)
        .map(|s| doc.span_text(s))
        .collect();
    assert_eq!(refs, ["1", "2"]);
    let notes: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.marker, Marker::Footnote(_)))
        .map(|b| (b.marker, block_text(&doc, b)))
        .collect();
    assert_eq!(
        notes,
        [
            (Marker::Footnote(1), "A ↩".to_owned()),
            (Marker::Footnote(2), "B ↩".to_owned()),
        ]
    );
    // Also across segments, with the definitions in front.
    let filler = filler(30);
    let src = format!("[^b]: B\n\n[^a]: A\n\n{filler}ref[^a] then[^b]\n");
    assert!(assert_same("numbering", &src, 400) > 3);
}

#[test]
fn form_feed_and_vertical_tab_lines_are_not_cut_inside_html_blocks() {
    // A form feed line after `---` can't be told from text: the `<pre>` block that follows holds
    // a heading and bold text as plain content.
    let region = "---\n\u{c}\n<pre>\n---\n\n# Heading\n**bold**\n</pre>\n\n";
    for blank in ["\u{c}", "\u{b}", "\t", " \u{c} ", "\u{c}\u{c}"] {
        let region = region.replace('\u{c}', blank);
        let filler = filler(6);
        let src = format!("{filler}{region}{filler}Text.\n");
        for budget in [1, 100, 700] {
            assert_same(&format!("{blank:?} @ {budget}"), &src, budget);
        }
    }
    // As a 1 MB text, with the region where the first segment is full.
    let filler = "filler paragraph\n\n".repeat(7283);
    let src = format!("{filler}{region}{}", "filler paragraph\n\n".repeat(58_000));
    assert!(src.len() > SEGMENT_THRESHOLD);
    assert_same("1 MB", &src, SEGMENT_BYTES);
    let doc = Doc::parse(&src);
    assert!(doc == Doc::parse_whole(&src));
}

#[test]
fn lines_of_whitespace_are_read_like_pulldown_cmark_reads_them() {
    let table = big_table(100);
    let rows = "| a | b | c |\n".repeat(100);
    for blank in [
        "\t",
        "\u{b}",
        "\u{c}",
        "  ",
        "      ",
        "\t\t",
        " \t ",
        "\u{b}\u{c}",
    ] {
        // A blank line ends a table: the rows after it are text.
        let src = format!("{table}{blank}\n{rows}");
        for budget in [1, 300] {
            assert_same(&format!("table, {blank:?} @ {budget}"), &src, budget);
        }
        // Lines of whitespace after link reference definitions, where pulldown-cmark can lose the
        // rest of the text.
        for def in [
            "[a b]: /l\n",
            "- [a b]: /l\n",
            "> [a]: /l\n",
            "- [x]: /y\n  [z]:\n",
        ] {
            let src = format!(
                "{def}{blank}\n  - nested\n\n{}[a b]\n\n{}",
                filler(5),
                filler(5)
            );
            for budget in [1, 100] {
                assert_same(&format!("{def:?} {blank:?} @ {budget}"), &src, budget);
            }
        }
    }
}

#[test]
fn reference_links_that_would_hit_the_expansion_limit_fall_back_to_one_parser() {
    // pulldown-cmark stops copying definitions into links once that has cost as many bytes as its
    // text is long (at least 100 KB). A segment's parser has less to spend than the one for the
    // whole text, so a segment that spends its share parses on one parser.
    let url = format!("/{}", "u".repeat(1000));
    // 300 uses (300 KB) in a segment of a 690 KB text.
    let uses = "[x] ".repeat(300);
    let src = format!("[x]: {url}\n\n{uses}\n\n{}", filler(15_000));
    assert!(Doc::parse_segments(&src, 400).is_none());
    assert_eq!(
        Doc::parse_whole(&src).links.len(),
        300,
        "one parser resolves them all"
    );
    assert_same("one segment spends too much", &src, 400);
    // Many segments that spend little each but a lot together: the whole text is only 120 KB.
    let mut src = format!("[x]: {url}\n\n");
    for _ in 0..150 {
        src.push_str(&format!("[x] {}\n\n", "t".repeat(450)));
    }
    assert!(Doc::parse_segments(&src, 400).is_none());
    assert_same("segments together spend too much", &src, 400);
    // Modest use is no problem, and the text is cut.
    let src = format!("[x]: {url}\n\n{}", "[x] and a paragraph.\n\n".repeat(50));
    assert!(assert_same("modest use", &src, 400) > 3);
}

#[test]
fn footnotes_inside_footnotes_stay_part_of_the_outer_note() {
    for src in [
        "[^a]: x\n\n    > [^b]: y\n\nr[^a][^b]\n",
        "[^a]: x\n    - [^b]: y\n\nr[^a][^b]\n",
        "[^a]: x\n    - [^b]: y\n\nr[^a]\n",
        "[^a]: x\n    - [^b]: y\n",
        "r[^b]\n\n[^a]: x\n    - [^b]: y\n",
        "r[^a]\n\n[^a]: x\n    - [^A]: y\n    - [^c]: z\n",
    ] {
        let doc = Doc::parse(src);
        check_invariants(&doc);
        check_anchors(&doc);
        // One section entry and one arrow for the outer note, and the inner label names its text.
        let notes = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.marker, Marker::Footnote(_)))
            .count();
        assert_eq!(notes, 1, "{src:?}");
        let arrows: usize = doc
            .blocks
            .iter()
            .map(|b| block_text(&doc, b).matches('↩').count())
            .sum();
        assert!(arrows <= 1, "{src:?}");
        let at = |id: &str| doc.anchors.iter().find(|(a, _)| a == id).map(|(_, b)| *b);
        let inner = at("fn-b")
            .or(at("fn-c"))
            .expect("an anchor for the inner note");
        assert!(
            block_text(&doc, &doc.blocks[inner as usize]).contains(['y', 'z']),
            "{src:?}"
        );
    }
}

#[test]
fn heading_slugs_keep_marks_and_connectors_like_github() {
    let slug = |src: &str| Doc::parse(src).headings[0].slug.clone();
    // A virama (Mn) and a decomposed accent are part of their word.
    assert_eq!(slug("# हिन्दी\n"), "हिन्दी");
    assert_eq!(slug("# cafe\u{0301}\n"), "cafe\u{0301}");
    assert_eq!(slug("# Café Ünï\n"), "café-ünï");
    // Connector punctuation (`_`, `‿`) and `-` stay; other punctuation, symbols and emoji go.
    assert_eq!(slug("# snake_case and a‿b\n"), "snake_case-and-a‿b");
    assert_eq!(
        slug("# Hello, World! 🎉 — 100% (ok) #1\n"),
        "hello-world---100-ok-1"
    );
    assert_eq!(slug("# foo-bar\n"), "foo-bar");
    // Links to such headings resolve.
    let doc = Doc::parse("[go](#हिन्दी) [a](#cafe\u{0301})\n\n# हिन्दी\n\n# cafe\u{0301}\n");
    let targets: Vec<&str> = doc
        .links
        .iter()
        .filter_map(|l| l.strip_prefix('#'))
        .collect();
    assert_eq!(targets, ["हिन्दी", "cafe\u{0301}"]);
    for t in targets {
        assert!(doc.anchors.iter().any(|(s, _)| s == t), "dangling #{t}");
    }
}

/// Heading slugs with their block (into the final block list), anchors that way too.
fn heading_blocks(doc: &Doc) -> Vec<(String, u32)> {
    doc.headings
        .iter()
        .map(|h| (h.slug.clone(), h.block))
        .collect()
}

#[test]
fn headings_inside_footnotes_have_anchors() {
    let src = "[go](#note-title) [^n]\n\n[^n]:\n    # Note title\n\n    text\n";
    let doc = Doc::parse(src);
    check_invariants(&doc);
    check_anchors(&doc);
    let at = doc
        .anchors
        .iter()
        .find(|(id, _)| id == "note-title")
        .expect("the link resolves")
        .1;
    assert!(matches!(
        doc.blocks[at as usize].kind,
        BlockKind::Heading(1)
    ));
    assert_eq!(block_text(&doc, &doc.blocks[at as usize]), "Note title");
    // It is a heading of the document too, after the ones of the main flow.
    assert_eq!(heading_blocks(&doc), [("note-title".to_owned(), at)]);
    assert_eq!(Doc::parse_whole(src), doc);

    // Definitions are listed by reference number: the headings and anchors follow them there, not
    // where the definitions were written. A heading in a nested definition counts too.
    let src = "# Top\n\nsee [x](#second) [y](#first) [z](#inner)\n\nb[^b] a[^a]\n\n\
               [^a]:\n    # First\n\n    one\n\n\
               [^b]:\n    ## Second\n\n    two\n    - [^c]:\n      ### Inner\n\n      deep\n";
    let doc = Doc::parse(src);
    check_invariants(&doc);
    check_anchors(&doc);
    let slugs: Vec<_> = doc.headings.iter().map(|h| h.slug.as_str()).collect();
    assert_eq!(slugs, ["top", "second", "inner", "first"]);
    for h in &doc.headings {
        let anchor = doc.anchors.iter().find(|(id, _)| *id == h.slug).unwrap();
        assert_eq!(anchor.1, h.block);
        let want = match h.slug.as_str() {
            "top" => "Top",
            "second" => "Second",
            "inner" => "Inner",
            _ => "First",
        };
        assert!(
            block_text(&doc, &doc.blocks[h.block as usize]).starts_with(want),
            "{h:?}"
        );
    }
    assert!(doc.headings.windows(2).all(|w| w[0].block < w[1].block));
    assert_eq!(Doc::parse_whole(src), doc);
}

#[test]
fn heading_slugs_stay_unique_across_the_text_and_its_footnotes() {
    let src = "# Same\n\nx[^a][^b]\n\n# Same\n\n[^a]:\n    # Same\n\n    text\n\n\
               [^b]:\n    # Same\n    # Same-1\n\n    text\n\n# Same\n\n[^b]: dropped\n    # Same\n";
    let doc = Doc::parse(src);
    check_invariants(&doc);
    check_anchors(&doc);
    // Slugs are handed out in text order, the unused suffixes first; a repeated definition (its
    // text is dropped) takes none.
    let slugs: Vec<_> = doc.headings.iter().map(|h| h.slug.as_str()).collect();
    assert_eq!(
        slugs,
        ["same", "same-1", "same-4", "same-2", "same-3", "same-1-1"]
    );
    let links: Vec<_> = (0..slugs.len())
        .map(|i| format!("[{i}](#{})", slugs[i]))
        .collect();
    let linked = Doc::parse(&format!("{}\n\n{src}", links.join(" ")));
    check_anchors(&linked);
    assert_eq!(linked.headings.len(), slugs.len());
    assert_eq!(Doc::parse_whole(src), doc);
}

#[test]
fn headings_inside_footnotes_segment_like_one_parser() {
    let note = |i: usize| {
        format!(
            "Ref[^n{i}] and [x](#note-{i}).\n\n[^n{i}]:\n    # Note {i}\n\n    text\n\n# Note {i}\n\n"
        )
    };
    let src: String = (0..12).map(note).collect();
    for budget in [1, 100, 300] {
        assert!(assert_same("footnote headings", &src, budget) > 1);
    }
    let doc = Doc::parse_whole(&src);
    check_anchors(&doc);
    assert_eq!(doc.headings.len(), 24);
}

/// `n` lines made by `line`, each with its line break.
fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
    (0..n).map(|i| line(i) + "\n").collect()
}

/// The ordered markers of the blocks, in order (the number each item that has one is shown with).
fn numbers(doc: &Doc) -> Vec<u32> {
    doc.blocks
        .iter()
        .filter_map(|b| match b.marker {
            Marker::Ordered(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// Segmented parsing at `budget` equals one parser, cuts lists between items (more than `joins`
/// of them), and the builder joins them up without falling back to one parser.
fn assert_list_joins(name: &str, src: &str, budget: usize, joins: usize) {
    LIST_MISFITS.with(|n| n.set(0));
    assert_same(name, src, budget);
    let at = list_joins(src, budget);
    assert!(at > joins, "{name} @ {budget}: {at} list cuts");
    assert!(
        Doc::parse_segments(src, budget).is_some(),
        "{name} @ {budget}: fell back to one parser"
    );
    assert_eq!(LIST_MISFITS.with(|n| n.get()), 0, "{name} @ {budget}");
}

#[test]
fn segmented_parse_cuts_lists_between_items() {
    let mut cases: Vec<(String, String)> = Vec::new();
    for marker in ["-", "*", "+", "1.", "1)", "7.", "0.", "5)", "999999999."] {
        cases.push((
            format!("tight {marker}"),
            lines(40, |i| {
                format!("{marker} item {i} with `code` and [a link](/u{i})")
            }),
        ));
        // Blank lines make a list loose wherever they are: between every item, only after the
        // first, only before the last, or only in some segment far from the cut.
        for (gap, name) in [(1, "every"), (7, "7th"), (40, "last")] {
            let src: String = (0..40)
                .map(|i| {
                    let blank = if i % gap == gap - 1 && i < 39 {
                        "\n"
                    } else {
                        ""
                    };
                    format!("{marker} item {i}\n{blank}")
                })
                .collect();
            cases.push((format!("loose {marker} ({name})"), src));
        }
    }
    // Ordered numbers: repeated, written any way at all.
    let any = [5, 9, 2, 100, 0, 3, 3, 3, 14, 1];
    for delim in ['.', ')'] {
        cases.push((
            format!("any numbers {delim}"),
            lines(40, |i| format!("{}{delim} item {i}", any[i % any.len()])),
        ));
        cases.push((
            format!("counting {delim}"),
            lines(40, |i| format!("{}{delim} item {i}", i + 3)),
        ));
    }
    // Items that hold other blocks, tight and loose; the text of an item after a code block
    // or a quote, tasks.
    let rich = [
        "- [ ] task {i}",
        "- [x] done {i}",
        "- item {i}\n  - nested\n    - deeper",
        "- item {i}\n\n  ```rust\n  fn f() {{}}\n\n  ```\n",
        "- item {i}\n  > quote\n  > more",
        "- > quote {i}\n  >\n  > - in it",
        "- item {i}\n\n  | a |\n  |---|\n  | 1 |",
        "- # heading {i}\n  text",
        "- item {i}\n\n      indented code\n\n  after",
        "- <div>{i}</div>",
        "-   spaced {i}",
        "-\ttabbed {i}",
    ];
    cases.push((
        "rich bullets".into(),
        lines(48, |i| rich[i % rich.len()].replace("{i}", &i.to_string())),
    ));
    cases.push((
        "rich ordered".into(),
        lines(48, |i| {
            let item = rich[i % rich.len()].replace("{i}", &i.to_string());
            let indented = item.replace("\n  ", "\n   ");
            format!("{}. {}", i % 7, &indented[2..])
        }),
    ));
    // A list followed by text, with more list after it.
    cases.push((
        "list, text, list".into(),
        lines(60, |i| format!("1. a{i}")) + "\nText.\n\n" + &lines(60, |i| format!("1. b{i}")),
    ));
    for (name, src) in &cases {
        for budget in [1, 40, 200] {
            let joins = if budget == 200 { 0 } else { 2 };
            assert_list_joins(name, src, budget, joins);
        }
    }
}

#[test]
fn joined_lists_keep_their_numbers_and_depth() {
    // One list, numbered from its first number whatever later items say.
    let src = lines(30, |i| format!("{}. item {i}", [7, 1, 9][i % 3]));
    assert_list_joins("numbers", &src, 30, 5);
    let doc = Doc::parse_segments(&src, 30).unwrap();
    assert_eq!(numbers(&doc), (7..37).collect::<Vec<_>>());
    assert!(doc.blocks.iter().all(|b| b.list == 1));
    // `1.` repeated.
    let src = lines(30, |i| format!("1. item {i}"));
    assert_list_joins("ones", &src, 30, 5);
    assert_eq!(
        numbers(&Doc::parse_segments(&src, 30).unwrap()),
        (1..=30).collect::<Vec<_>>()
    );
    // A list that ends is not continued: the next one starts again at its own number. The
    // delimiter makes a list of its own, so does the text between.
    for src in [
        lines(12, |i| format!("1. a{i}")) + &lines(12, |i| format!("1) b{i}")),
        lines(12, |i| format!("1. a{i}\n")) + &lines(12, |i| format!("5) b{i}\n")),
        lines(12, |i| format!("1. a{i}")) + "\ntext\n\n" + &lines(12, |i| format!("1. b{i}")),
        lines(12, |i| format!("1. a{i}")) + "\n---\n\n" + &lines(12, |i| format!("3. b{i}")),
        lines(12, |i| format!("3. a{i}")) + "\n<!-- x -->\n\n" + &lines(12, |i| format!("9. b{i}")),
        "> 3. quoted\n> 4. items\n\n".to_owned() + &lines(12, |i| format!("1. b{i}")),
        "- a\n  - b\n\n".to_owned() + &lines(12, |i| format!("1. b{i}")),
    ] {
        for budget in [1, 20, 60] {
            assert_same("two lists", &src, budget);
            let n = |doc: &Doc| numbers(doc);
            let (whole, parts) = (Doc::parse_whole(&src), Doc::parse(&src));
            assert_eq!(n(&whole), n(&parts));
            if let Some(parts) = Doc::parse_segments(&src, budget) {
                // Neither list's numbers go on in the other.
                assert_eq!(n(&whole), n(&parts), "{src:?}");
            }
        }
    }
    let src = lines(12, |i| format!("1. a{i}")) + &lines(12, |i| format!("1) b{i}"));
    let doc = Doc::parse_segments(&src, 20).unwrap();
    let want: Vec<u32> = (1..=12).chain(1..=12).collect();
    assert_eq!(numbers(&doc), want);
}

#[test]
fn lists_are_not_cut_where_they_may_end_or_be_another_list() {
    // After ten items a line that ends the list, or may (lazy text, a quote, a rule, HTML, ...),
    // with or without blank lines, then ten more items of the same kind (a new list), which are
    // not joined to the first.
    let breaks = [
        "text",
        "lazy continuation",
        " one space",
        "> quote",
        " > quote",
        "# Heading",
        "---",
        "***",
        "- - -",
        "* * *",
        "<div>",
        "<!-- c -->",
        "```\ncode\n```",
        "~~~",
        "[foo]: /url",
        "[^a]: note",
        "| a |\n|---|\n| 1 |",
        "1234567890. not a marker",
        "-",
        "- ",
        "-\t",
        "10.",
        "* other bullet",
        "+ another bullet",
    ];
    for brk in breaks {
        for gaps in [("", ""), ("\n", "\n"), ("", "\n"), ("\n", "")] {
            for marker in ["-", "1."] {
                let first = lines(10, |i| format!("{marker} a{i}"));
                let second = lines(10, |i| format!("{marker} b{i}"));
                let src = format!("{first}{}{brk}\n{}{second}", gaps.0, gaps.1);
                let at = first.len() + gaps.0.len() + brk.len() + 1 + gaps.1.len();
                for budget in [1, 25] {
                    assert_same(brk, &src, budget);
                    let plan = segment::plan(&src, budget).unwrap();
                    // The first item after the break may start a segment only if it isn't one
                    // that goes on with the list before it.
                    assert!(
                        plan.segments
                            .iter()
                            .all(|s| !(s.range.start == at && s.list)),
                        "{brk:?} {gaps:?} {marker}: joined to the list before it"
                    );
                }
            }
        }
    }
}

#[test]
fn a_list_start_is_certain_before_it_is_cut() {
    // A list inside a quote or a footnote, or one that only looks like one, is not top-level.
    for head in [
        "> - a\n> - b\n",
        "> 1. a\n> 2. b\n",
        "[^n]: note\n    - a\n    - b\n",
        "    - a\n    - b\n",
        "text\n2. a\n3. b\n",
        "<div>\n- a\n- b\n",
        "```\n- a\n- b\n```\n",
        "1234567890. a\n1234567890. b\n",
        "-a\n-b\n",
        "- - -\n- - -\n",
        "-\n-\n",
    ] {
        let src = head.repeat(30) + "- x\n- y\n";
        for budget in [1, 20, 100] {
            assert_same(head, &src, budget);
            let src = format!("{head}\n{}", lines(12, |i| format!("1. n{i}")));
            assert_same(head, &src, budget);
        }
    }
}

#[test]
fn a_giant_list_is_cut_between_its_items() {
    // A generated file index: one list of 116,280 items, 5.0 MB.
    let src = lines(116_280, |i| {
        format!("- [module_{i:06}.rs](src/module_{i:06}.rs)")
    });
    assert!(src.len() > 5_000_000);
    let plan = segment::plan(&src, SEGMENT_BYTES).expect("a list is cut");
    let longest = plan.segments.iter().map(|s| s.range.len()).max().unwrap();
    assert!(
        plan.segments.len() >= src.len() / SEGMENT_BYTES,
        "{} segments",
        plan.segments.len()
    );
    assert!(longest < SEGMENT_BYTES + 100, "{longest} bytes");
    assert!(plan.segments[1..].iter().all(|s| s.list));
    assert!(!plan.segments[0].list);
    // It parses to the same document either way (a smaller one, which keeps the test quick, with
    // the real thresholds).
    let src = lines(26_000, |i| {
        format!("{}. [module_{i:06}.rs](src/module_{i:06}.rs)", i % 3 + 1)
    });
    assert!(src.len() > SEGMENT_THRESHOLD);
    let doc = Doc::parse(&src);
    check_invariants(&doc);
    assert!(doc == Doc::parse_whole(&src));
    assert_eq!(doc.blocks.len(), 26_000);
    assert_eq!(numbers(&doc).last(), Some(&26_000));
}

#[test]
fn segments_that_do_not_fit_a_list_are_refused() {
    // The planner said the next segment goes on with the list: if it doesn't start with one (or
    // the list that ends isn't a top-level one), the builder says so instead of guessing.
    let mut b = Builder::new(0);
    b.begin_segment(0, false, true);
    for event in Parser::new_ext("text\n", options()) {
        b.event(event);
    }
    assert!(b.broken && b.misfit);
    let mut b = Builder::new(0);
    b.begin_segment(0, false, true);
    for event in Parser::new_ext("1. a\n", options()) {
        b.event(event);
    }
    assert!(
        b.broken && b.misfit,
        "an ordered list can't continue nothing"
    );
    let mut b = Builder::new(0);
    let mut events = Parser::new_ext("> - a\n", options()).collect::<Vec<_>>();
    let end = events.pop().unwrap();
    assert!(matches!(end, Event::End(TagEnd::BlockQuote(_))));
    assert!(!b.hold_list(&end));
    assert!(b.broken && b.misfit);
}

/// Pipelines to parse with: all on one thread, and threads of every number against windows
/// tighter and wider than they are.
fn pipelines() -> Vec<pipeline::Config> {
    [(0, 1), (1, 1), (1, 3), (2, 1), (3, 2), (3, 5), (7, 8)]
        .into_iter()
        .map(|(workers, window)| pipeline::Config::new(workers, window))
        .collect()
}

/// Segmented parsing at `budget` on every pipeline gives the document one parser gives. Returns
/// how many segments the text is cut into.
fn assert_same_in_parallel(name: &str, src: &str, budget: usize) -> usize {
    let whole = Doc::parse_whole(src);
    for (i, config) in pipelines().iter().enumerate() {
        let Some(parts) = Doc::parse_segments_with(src, budget, config) else {
            continue;
        };
        assert!(
            whole == parts,
            "{name} @ {budget}: the pipeline ({} workers, window {}) differs",
            config.workers,
            config.window
        );
        assert!(config.peak() <= config.window, "{name}: window overrun {i}");
    }
    segment_count(src, budget)
}

#[test]
fn parallel_parse_matches_the_single_parser() {
    for name in ["tiny", "readme", "spec", "cjk", "rtl", "emoji", "tables"] {
        let path = format!("{}/fixtures/{name}.md", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        for budget in [1, 200, 1024] {
            let n = assert_same_in_parallel(name, &src, budget);
            if src.len() > 20_000 && budget <= 1024 {
                assert!(n >= 5, "{name} @ {budget}: {n} segments");
            }
        }
    }
    // The shapes that make a giant document: a generated file index, a table, prose with
    // definitions and footnotes far from their uses, and the same with CRLF line ends.
    let index: String = (0..3000)
        .map(|i| format!("- [module_{i:06}.rs](src/module_{i:06}.rs)\n"))
        .collect();
    let numbered: String = (0..1500)
        .map(|i| format!("{}. item [x]\n", i + 1))
        .collect();
    let numbered = numbered + "\n[x]: /x\n";
    let notes = format!(
        "Ref[^a] and [y].\n\n{}[^a]: A note.\n\n[y]: /y \"Y\"\n\n{}",
        filler(40),
        filler(40)
    );
    let mixed = include_str!("../../fixtures/readme.md").to_owned()
        + &big_table(300)
        + &index[..2000]
        + "\n\n"
        + include_str!("../../fixtures/tables.md");
    for (name, src) in [
        ("file index", index),
        ("numbered list", numbered),
        ("table", big_table(1500)),
        ("notes", notes),
        ("mixed", mixed),
    ] {
        for text in [src.clone(), src.replace('\n', "\r\n")] {
            for budget in [100, 400, 3000] {
                let n = assert_same_in_parallel(name, &text, budget);
                assert!(
                    n > 3 || text.len() / budget < 4,
                    "{name} @ {budget}: {n} segments"
                );
            }
        }
    }
}

#[test]
fn parallel_parse_keeps_few_segments_in_flight() {
    let table = big_table(4000);
    let index: String = (0..6000)
        .map(|i| format!("- [module_{i:06}.rs](src/module_{i:06}.rs)\n"))
        .collect();
    for src in [&table, &index] {
        for (workers, window) in [(3, 1), (3, 2), (3, 4), (2, 5)] {
            let config = pipeline::Config::new(workers, window);
            let doc = Doc::parse_segments_with(src, 800, &config).unwrap();
            assert!(doc == Doc::parse_whole(src));
            assert!(segment_count(src, 800) > 50);
            assert!(
                config.peak() <= window,
                "{} segments in flight, window {window}",
                config.peak()
            );
            // The threads do get ahead of the builder when there is room.
            assert!(config.peak() >= window.min(2), "{workers} × {window}");
        }
    }
}

#[test]
fn a_late_fallback_with_threads_ahead_gives_the_single_parser_result() {
    let url = format!("/{}", "u".repeat(1000));
    // The segments that spend too much on references come last, after hundreds that don't:
    // the threads are well ahead of the builder when it finds out. (One segment spends its own
    // share, or many spend it together.)
    let uses = "[x] ".repeat(300);
    let single = format!("[x]: {url}\n\n{}{uses}\n\n{}", filler(4000), filler(100));
    let mut many = format!("[x]: {url}\n\n{}", filler(3000));
    for _ in 0..400 {
        many.push_str(&format!("[x] {}\n\n", "t".repeat(450)));
    }
    for (name, src) in [("one segment", single), ("together", many)] {
        assert!(segment_count(&src, 400) > 300, "{name}");
        let whole = Doc::parse_whole(&src);
        assert!(whole.links.len() >= 100, "{name}: one parser resolves them");
        for config in pipelines() {
            let (workers, window) = (config.workers, config.window);
            assert!(
                Doc::parse_segments_with(&src, 400, &config).is_none(),
                "{name}: {workers} workers, window {window}"
            );
        }
    }
    // The same through `Doc::parse` for a document big enough to be segmented by itself.
    let src = format!("[x]: {url}\n\n{}{uses}\n\n{}", filler(25_000), filler(100));
    assert!(src.len() > SEGMENT_THRESHOLD);
    assert!(Doc::parse_segments(&src, SEGMENT_BYTES).is_none());
    assert!(Doc::parse(&src) == Doc::parse_whole(&src));
}
