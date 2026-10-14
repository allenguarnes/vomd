use super::*;

/// Human-readable IR dump for snapshots, one block per line:
/// `<kind> [list=N] [quote=N] [qstart=N] [marker] | [style]"text" ...`
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
        if block.marker != Marker::None {
            out.push_str(&format!(" {:?}", block.marker));
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
            assert!((first_cell + cells as u32) as usize <= doc.cells.len());
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
