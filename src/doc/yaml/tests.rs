use super::*;

fn s(text: &str) -> Node {
    Node::Scalar {
        text: text.into(),
        plain: true,
        tag: None,
    }
}

fn q(text: &str) -> Node {
    Node::Scalar {
        text: text.into(),
        plain: false,
        tag: None,
    }
}

fn tagged(text: &str, tag: &str) -> Node {
    Node::Scalar {
        text: text.into(),
        plain: true,
        tag: Some(tag.into()),
    }
}

fn map(entries: Vec<(Node, Node)>) -> Node {
    Node::Map(entries)
}

fn node(src: &str) -> Node {
    match read(src) {
        Ok(Some((n, false))) => n,
        other => panic!("{src:?}: {other:?}"),
    }
}

/// The value of `k:` in `src`.
fn value_of(src: &str) -> Node {
    match node(src) {
        Node::Map(mut e) if e.len() == 1 => e.remove(0).1,
        other => panic!("{src:?}: {other:?}"),
    }
}

#[test]
fn documents() {
    assert_eq!(read(""), Ok(None));
    assert_eq!(read("\n  \n# c\n  # d\n"), Ok(None));
    assert_eq!(node("text"), s("text"));
    assert_eq!(node("- a\n- b\n"), Node::Seq(vec![s("a"), s("b")]));
    assert_eq!(
        node("a: 1\nb:\n  c: [x, {y: z}]\n"),
        map(vec![
            (s("a"), s("1")),
            (
                s("b"),
                map(vec![(
                    s("c"),
                    Node::Seq(vec![s("x"), map(vec![(s("y"), s("z"))])])
                )])
            ),
        ])
    );
    // Duplicates are kept here; the Hash made of them keeps the last.
    assert_eq!(
        node("a: 1\na: 2\n"),
        map(vec![(s("a"), s("1")), (s("a"), s("2"))])
    );
    // Aliases are noted, not followed.
    assert_eq!(
        read("a: &x 1\nb: *x\n"),
        Ok(Some((
            map(vec![(s("a"), s("1")), (s("b"), Node::Alias)]),
            true
        )))
    );
}

#[test]
fn empty_values() {
    assert_eq!(
        node("a:\nb: ~\nc: ''\nd: []\ne: {}\n"),
        map(vec![
            (s("a"), s("")),
            (s("b"), s("~")),
            (s("c"), q("")),
            (s("d"), Node::Seq(vec![])),
            (s("e"), map(vec![])),
        ])
    );
    assert!(read("k: [a, , b]").is_err());
    assert_eq!(
        value_of("k: {a, b: }"),
        map(vec![(s("a"), s("")), (s("b"), s(""))])
    );
    assert_eq!(value_of("k:\n- \n-\n"), Node::Seq(vec![s(""), s("")]));
}

#[test]
fn plain_scalars_fold() {
    assert_eq!(value_of("k: a\n  b\n\n  c\n\n\n  d"), s("a b\nc\n\nd"));
    assert_eq!(value_of("k: a   b  # c\n"), s("a   b"));
    assert_eq!(value_of("k: a#b:c :d"), s("a#b:c :d"));
    assert_eq!(
        value_of("k: [a\n  b, c d]"),
        Node::Seq(vec![s("a b"), s("c d")])
    );
    assert_eq!(
        value_of("k: {a:b, c: d}"),
        map(vec![(s("a:b"), s("")), (s("c"), s("d"))])
    );
    assert_eq!(value_of("k: -a"), s("-a"));
    assert_eq!(value_of("k: ?a"), s("?a"));
    assert_eq!(value_of("k: :a"), s(":a"));
    // A continuation line must be indented more than the mapping.
    assert_eq!(
        node("a: x\nb: y\n"),
        map(vec![(s("a"), s("x")), (s("b"), s("y"))])
    );
    // In a sequence item: more than the dash.
    assert_eq!(node("- a\n  b\n- c\n"), Node::Seq(vec![s("a b"), s("c")]));
}

#[test]
fn quoted_scalars() {
    assert_eq!(value_of("k: 'it''s'"), q("it's"));
    assert_eq!(value_of("k: 'a\n  b\n\n  c  '"), q("a b\nc  "));
    assert_eq!(
        value_of(r#"k: "\0\a\b\t\	\n\v\f\r\e\ \"\/\\\N\_\L\P""#),
        q("\0\x07\x08\t\t\n\x0b\x0c\r\x1b \"/\\\u{85}\u{a0}\u{2028}\u{2029}")
    );
    assert_eq!(value_of(r#"k: "\x41\u00e9\U0001F600""#), q("Aé😀"));
    assert_eq!(value_of("k: \"a \\\n   b\""), q("a b"));
    assert_eq!(value_of("k: \"a\\\n   b\""), q("ab"));
    assert_eq!(value_of("k: \"a\n\n  b\""), q("a\nb"));
    // An escaped break goes with its line's indentation; blank lines after it stay breaks.
    assert_eq!(value_of("k: \"a\\\n\n  b\""), q("a\nb"));
    assert_eq!(value_of("k: \"a\\\n  \n\n  b\""), q("a\n\nb"));
    assert_eq!(value_of("k: \"a\\\r\n\r\n  b\""), q("a\nb"));
    assert_eq!(value_of("k: \"#not a comment\""), q("#not a comment"));
    assert!(read(r#"k: "\'""#).is_err());
    assert!(read(r#"k: "\U00110000""#).is_err());
}

#[test]
fn block_scalars() {
    let block = |b: &str| match value_of(&format!("k: {b}")) {
        Node::Scalar { text, plain, .. } => {
            assert!(!plain);
            text
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(block("|\n  a\n   b\n\n  c\n\n\n"), "a\n b\n\nc\n");
    assert_eq!(block("|-\n  a\n\n"), "a");
    assert_eq!(block("|+\n  a\n\n"), "a\n\n");
    assert_eq!(block(">\n  a\n  b\n\n  c\n   d\n  e\n"), "a b\nc\n d\ne\n");
    assert_eq!(block("|2\n    a\n"), "  a\n");
    assert_eq!(block("|-2 # c\n   a\n"), " a");
    assert_eq!(block("|\n\n  \n  a\n"), "\n\na\n");
    assert_eq!(block("|\n"), "");
    assert_eq!(block("|"), "");
    // It ends at the first line indented less, which goes on as the mapping's next key.
    assert_eq!(
        node("a: |\n  x\nb: 1\n"),
        map(vec![(s("a"), q("x\n")), (s("b"), s("1"))])
    );
    assert_eq!(node("- |\n  x\n- y\n"), Node::Seq(vec![q("x\n"), s("y")]));
}

#[test]
fn tags_and_anchors() {
    assert_eq!(value_of("k: !!str 1"), tagged("1", "tag:yaml.org,2002:str"));
    assert_eq!(value_of("k: !x 1"), tagged("1", "!x"));
    assert_eq!(value_of("k: ! 1"), tagged("1", "!"));
    assert_eq!(
        value_of("k: !<tag:a.b,2000:c> 1"),
        tagged("1", "tag:a.b,2000:c")
    );
    assert_eq!(
        value_of("k: &a !!int 1"),
        tagged("1", "tag:yaml.org,2002:int")
    );
    assert_eq!(
        value_of("k: !!int &a 1"),
        tagged("1", "tag:yaml.org,2002:int")
    );
    assert_eq!(value_of("k: !!str"), tagged("", "tag:yaml.org,2002:str"));
    assert_eq!(
        value_of("k: [!!str , a]"),
        Node::Seq(vec![tagged("", "tag:yaml.org,2002:str"), s("a")])
    );
    assert_eq!(value_of("k: &a\n  x: 1"), map(vec![(s("x"), s("1"))]));
    assert_eq!(value_of("k: !!map\n  x: 1"), map(vec![(s("x"), s("1"))]));
    assert!(read("k: !<a").is_err());
    assert!(read("k: !a\"b\"").is_err());
}

#[test]
fn collections() {
    // A mapping's sequence may sit at the mapping's indentation.
    assert_eq!(
        node("a:\n- 1\n- 2\nb: 3\n"),
        map(vec![
            (s("a"), Node::Seq(vec![s("1"), s("2")])),
            (s("b"), s("3"))
        ])
    );
    assert_eq!(
        node("- - a\n  - b\n- c: 1\n  d: 2\n"),
        Node::Seq(vec![
            Node::Seq(vec![s("a"), s("b")]),
            map(vec![(s("c"), s("1")), (s("d"), s("2"))]),
        ])
    );
    assert_eq!(
        node("? a\n? b\n: 1\n"),
        map(vec![(s("a"), s("")), (s("b"), s("1"))])
    );
    assert_eq!(
        node("? - x\n  - y\n: v\n"),
        map(vec![(Node::Seq(vec![s("x"), s("y")]), s("v"))])
    );
    assert_eq!(
        node("[a, b]: c\n"),
        map(vec![(Node::Seq(vec![s("a"), s("b")]), s("c"))])
    );
    assert_eq!(
        value_of("k: [? a : b, c: d, \"e\":f]"),
        Node::Seq(vec![
            map(vec![(s("a"), s("b"))]),
            map(vec![(s("c"), s("d"))]),
            map(vec![(q("e"), s("f"))]),
        ])
    );
    assert_eq!(
        value_of("k: {? a, b: [c]}"),
        map(vec![(s("a"), s("")), (s("b"), Node::Seq(vec![s("c")]))])
    );
    assert_eq!(
        value_of("k: [\n  a,   # x\n  b\n]"),
        Node::Seq(vec![s("a"), s("b")])
    );
}

#[test]
fn implied_empty_nodes_count_against_the_limit() {
    let over = |src: String| {
        let e = read(&src).unwrap_err();
        assert!(e.contains("exceeded max node count of 100000"), "{e}");
    };
    // Empty sequence items and mapping values, written or not.
    over(format!("items:\n{}", "-\n".repeat(MAX_NODES + 1)));
    over(format!("items:\n{}", "  a:\n".repeat(MAX_NODES / 2 + 1)));
    // Explicit keys with nothing in them.
    over("? \n".repeat(MAX_NODES / 2 + 1));
    // Omitted values in flow mappings and sequences.
    over(format!("items: {{{}}}\n", "a:,".repeat(MAX_NODES / 2 + 1)));
    over(format!("items: {{{}}}\n", "a,".repeat(MAX_NODES / 2 + 1)));
    over(format!("items: [{}]\n", "a:,".repeat(MAX_NODES / 3 + 1)));
    over(format!("items: [{}]\n", "? ,".repeat(MAX_NODES / 3 + 1)));
    // Under the limit, the same forms read.
    assert!(read(&format!("items:\n{}", "-\n".repeat(MAX_NODES - 10))).is_ok());
}

#[test]
fn reading_is_linear() {
    crate::doc::assert_linear("flat map", 10_000, |n| {
        let src: String = (0..n).map(|i| format!("key{i}: value {i}\n")).collect();
        assert!(read(&src).is_ok());
    });
    crate::doc::assert_linear("long flow", 20_000, |n| {
        let src = format!("k: [{}]\n", "abc, ".repeat(n));
        assert!(read(&src).is_ok());
    });
    crate::doc::assert_linear("long plain", 100_000, |n| {
        let src = format!("k: {}\n", "word\n  ".repeat(n));
        assert!(read(&src).is_ok());
    });
    crate::doc::assert_linear("blank lines of tabs", 100_000, |n| {
        // (Past the mapping's column: a tab left of it is an error.)
        let src = format!("k: v\n {}\n", "\t".repeat(n));
        assert!(read(&src).is_ok());
    });
    crate::doc::assert_linear("deep and wide", 2_000, |n| {
        let unit = "- [[[[[[[[[[a]]]]]]]]]]\n";
        assert!(read(&unit.repeat(n)).is_ok());
    });
}
