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
            for cell in doc.row_cells(block) {
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
            for cell in doc.cells[first_cell as usize..][..cells as usize].iter() {
                assert!(cell.start >= block.spans.start && cell.end <= block.spans.end);
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
    // The last newline in reach wins over a later space; the newline itself is dropped.
    assert_eq!(t("ab\ncd ef gh", 8), ["ab", "cd ef gh"]);
    assert_eq!(t("aa\nbb\ncc\ndd", 8), ["aa\nbb", "cc\ndd"]);
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
    assert_eq!(t("\nabcdefgh", 4), ["", "abcd", "efgh"]);
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

/// `(kind, text)` of every block, with chunk runs rejoined.
fn runs(doc: &Doc) -> Vec<(BlockKind, String)> {
    let mut out: Vec<(BlockKind, String)> = Vec::new();
    for b in &doc.blocks {
        let text: String = doc
            .block_spans(b)
            .iter()
            .map(|s| doc.span_text(s))
            .collect();
        match out.last_mut() {
            Some((_, t)) if b.joined => {
                t.push('\n');
                t.push_str(&text);
            }
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
    // Rejoining with the dropped newlines restores the code (minus its trailing newline).
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
