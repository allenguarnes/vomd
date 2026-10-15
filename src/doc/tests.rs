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
    // The references show their definition's number.
    let refs: Vec<&str> = doc
        .spans
        .iter()
        .filter(|s| s.style & style::FOOTREF != 0)
        .map(|s| doc.span_text(s))
        .collect();
    assert_eq!(refs, ["1", "8"]);
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
        // A metadata block in the middle of the text.
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
        assert!(
            n >= 3 || block.starts_with("  ```") || block.contains("- item"),
            "block {i}: {n} segments"
        );
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
    "| x |",
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
