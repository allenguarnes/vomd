use super::*;

/// The table of front matter `yaml` (between the fences) as `key | value` lines, a cell's line
/// breaks as `⏎`, bold as `**…**`; `markdown` when it isn't front matter; an error as
/// `error: <message>`.
fn table_of(yaml: &str) -> String {
    let src = format!("---\n{yaml}---\n");
    let Some(events) = events(&src) else {
        return "markdown".into();
    };
    let mut out = String::new();
    let mut cell = 0;
    let mut quote = false;
    for e in events {
        match e {
            Event::Start(Tag::BlockQuote(_)) => quote = true,
            Event::End(TagEnd::BlockQuote(_)) => quote = false,
            Event::Text(t) if quote => {
                out.push_str("error: ");
                out.push_str(t.strip_prefix("Error in user YAML: ").unwrap());
            }
            Event::Start(Tag::CodeBlock(_)) => {}
            Event::Text(t) if cell == 0 => {
                out.push_str("\ncode:\n");
                out.push_str(&t);
            }
            Event::Start(Tag::TableRow) => cell = 0,
            Event::End(TagEnd::TableRow) => out.push('\n'),
            Event::Start(Tag::TableCell) => {
                cell += 1;
                if cell == 2 {
                    out.push_str(" | ");
                }
            }
            Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) => out.push_str("**"),
            Event::Text(t) => out.push_str(&t),
            Event::HardBreak => out.push('⏎'),
            _ => {}
        }
    }
    out
}

/// The value cell of `key: <yaml>`.
fn cell(yaml: &str) -> String {
    let t = table_of(&format!("k: {yaml}\n"));
    match t.strip_prefix("**k** | ") {
        Some(v) => v.trim_end_matches('\n').to_owned(),
        None => t,
    }
}

/// How a plain scalar is typed and printed (a key's value, so the cell shows it).
fn typed(plain: &str) -> String {
    cell(plain)
}

#[test]
fn githubs_typing_probe() {
    // GitHub's rendering of this front matter (fetched 2026-11-21), row by row.
    let yaml = "octal: 0755\noctal2: 0o17\nhex: 0x1F\nsexa: 1:30\nsexa2: 190:20:30\n\
        commas: 1,000\nfloat: 4.50\nexp: 1e3\ninf: .inf\nnan: .NaN\n\
        bools: [yes, No, ON, off, y, n, True, FALSE]\nnulls: [~, null, Null]\nempty:\n\
        date: 2024-03-05\ntime_zoned: 2024-03-05 10:20:30 +02:00\n\
        time_utc: 2024-03-05T10:20:30Z\ntime_plain: 2024-03-05 10:20:30\nquoted: \"yes\"\n\
        single: 'it''s'\nstrtag: !!str 0755\nhtml: \"<b>bold</b> & <i>x</i>\"\n\
        url: https://example.com/a\nmd: \"**not bold** `code`\"\nunicode: \"café ☕\"\n";
    assert_eq!(
        table_of(yaml),
        "**octal** | 493\n**octal2** | 0o17\n**hex** | 31\n**sexa** | 5400\n\
         **sexa2** | 685230\n**commas** | 1000\n**float** | 4.5\n**exp** | 1e3\n\
         **inf** | Infinity\n**nan** | NaN\n\
         **bools** | true, false, true, false, y, n, true, false\n**nulls** | \n**empty** | \n\
         **date** | 2024-03-05\n**time_zoned** | 2024-03-05 10:20:30 +0200\n\
         **time_utc** | 2024-03-05 10:20:30 UTC\n**time_plain** | 2024-03-05 02:20:30 -0800\n\
         **quoted** | yes\n**single** | it's\n**strtag** | 0755\n\
         **html** | <b>bold</b> & <i>x</i>\n**url** | https://example.com/a\n\
         **md** | **not bold** `code`\n**unicode** | café ☕\n"
    );
}

#[test]
fn githubs_blocks_probe() {
    let yaml = "literal: |\n  line one\n  line two\n\n  after blank\nfolded: >\n  folded one\n  \
        folded two\nkeep: |+\n  kept\n\nstrip: |-\n  stripped\nlong: This is a fairly long value\n\
        nested:\n  a: 1\n  b:\n    c: deep\n    d: [1, 2]\nlistofmaps:\n  - name: one\n    v: 1\n  \
        - name: two\nlistoflists: [[1, 2], [3]]\nemptylist: []\nemptymap: {}\n";
    assert_eq!(
        table_of(yaml),
        "**literal** | line one line two after blank\n**folded** | folded one folded two\n\
         **keep** | kept\n**strip** | stripped\n**long** | This is a fairly long value\n\
         **nested** | **a**: 1⏎**b**: ⏎\u{a0}\u{a0}**c**: deep⏎\u{a0}\u{a0}**d**: 1, 2\n\
         **listofmaps** | • **name**: one⏎\u{a0}\u{a0}**v**: 1⏎• **name**: two\n\
         **listoflists** | • 1, 2⏎• 3\n**emptylist** | \n**emptymap** | \n"
    );
}

#[test]
fn githubs_keys_probe() {
    let yaml = "a: 1\na: 2\n1: int key\ntrue: bool key\n? [x]\n: complex\nwin: \"C:\\\\path\"\n\
        multi: \"a\\nb\"\n";
    assert_eq!(
        table_of(yaml),
        "**a** | 2\n**1** | int key\n**true** | bool key\n**[\"x\"]** | complex\n\
         **win** | C:\\path\n**multi** | a b\n"
    );
    // A key that is a string and one that is a number are two keys, as in a Ruby Hash.
    assert_eq!(table_of("1: a\n'1': b\n"), "**1** | a\n**1** | b\n");
}

#[test]
fn githubs_other_probes() {
    assert_eq!(cell(":foo"), "foo");
    assert_eq!(cell("!custom v"), "v");
    // Not maps: GitHub shows them as Markdown.
    for yaml in [
        "Some text\n",
        "- a\n- b\n",
        "# only a comment\n",
        "\n\n",
        "[a, b]\n",
        "~\n",
    ] {
        assert_eq!(table_of(yaml), "markdown", "{yaml:?}");
    }
    let aliases =
        "base: &b {x: 1}\nuse: *b\nmerged: {<<: *b, y: 2}\nsym: :foo\ntagged: !custom v\n";
    assert_eq!(
        table_of(aliases),
        format!("error: Alias parsing is not enabled.\ncode:\n---\n{aliases}---\n")
    );
    let invalid = "title: [unclosed\nkey: : :\n";
    assert_eq!(
        table_of(invalid),
        format!(
            "error: (<unknown>): did not find expected ',' or ']' while parsing a flow sequence \
             at line 1 column 8\ncode:\n---\n{invalid}---\n"
        )
    );
}

#[test]
fn psych_integers() {
    for (plain, shown) in [
        ("0", "0"),
        ("-0", "0"),
        ("+12", "12"),
        ("1_000", "1000"),
        ("1,000,000", "1000000"),
        ("1,2", "12"),
        ("1__0", "1__0"),
        ("1,", "1,"),
        ("0755", "493"),
        ("-0755", "-493"),
        ("0_7", "7"),
        ("08", "08"),
        ("00", "0"),
        ("0b1010", "10"),
        ("0b_1", "1"),
        ("0b2", "0b2"),
        ("0x1F", "31"),
        ("-0x1f", "-31"),
        ("0xg", "0xg"),
        ("0o17", "0o17"),
        (
            "123456789012345678901234567890",
            "123456789012345678901234567890",
        ),
        (
            "0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
            "340282366920938463463374607431768211455",
        ),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
    // Too long to convert: as written.
    let long = format!("0x{}", "f".repeat(MAX_RADIX_DIGITS + 1));
    assert_eq!(typed(&long), long);
}

#[test]
fn psych_sexagesimal() {
    for (plain, shown) in [
        ("1:30", "5400"),
        ("190:20:30", "685230"),
        ("-1:30", "-1800"),
        ("12:00", "43200"),
        ("1:5", "3900"),
        ("1:60", "1:60"),
        ("1:30:00:00", "1:30:00:00"),
        ("1:30.5", "5430.0"),
        ("1:00:30.5", "3630.5"),
        ("a:30", "a:30"),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
}

#[test]
fn psych_floats_print_as_ruby() {
    for (plain, shown) in [
        ("4.50", "4.5"),
        ("1.0", "1.0"),
        ("1.", "1.0"),
        (".5", "0.5"),
        ("-.5", "-0.5"),
        ("+1.5", "1.5"),
        ("1.0e+3", "1000.0"),
        ("1.e+3", "1000.0"),
        ("1.0e3", "1.0e3"),
        ("1e3", "1e3"),
        ("1,000.5", "1000.5"),
        ("0.0001", "0.0001"),
        ("0.00001", "1.0e-05"),
        ("1000000000000000.0", "1000000000000000.0"),
        ("10000000000000000.0", "1.0e+16"),
        ("1.5e-7", "1.5e-07"),
        ("-0.0", "-0.0"),
        ("3.10", "3.1"),
        ("1.0e+999", "Infinity"),
        (".", "."),
        ("-.", "-."),
        ("1.2.3", "1.2.3"),
        (".inf", "Infinity"),
        ("+.INF", "Infinity"),
        ("-.Inf", "-Infinity"),
        (".nan", "NaN"),
        ("+.nan", "+.nan"),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
    assert_eq!(ruby_float(0.1 + 0.2), "0.30000000000000004");
    assert_eq!(ruby_float(1e100), "1.0e+100");
    assert_eq!(ruby_float(123456789.125), "123456789.125");
    // `!!float` makes a float of a number.
    assert_eq!(cell("!!float 1"), "1.0");
    assert_eq!(cell("!!float abc"), "abc");
}

#[test]
fn psych_words() {
    for (plain, shown) in [
        ("yes", "true"),
        ("Yes", "true"),
        ("YES", "true"),
        ("on", "true"),
        ("TRUE", "true"),
        ("tRuE", "true"),
        ("no", "false"),
        ("Off", "false"),
        ("false", "false"),
        ("y", "y"),
        ("n", "n"),
        ("yess", "yess"),
        ("null", ""),
        ("NULL", ""),
        ("~", ""),
        ("nil", "nil"),
        ("none", "none"),
        ("hello world", "hello world"),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
    // Quoted, a block scalar, or `!!str`: a string, whatever it says.
    assert_eq!(cell("'yes'"), "yes");
    assert_eq!(cell("|\n  yes\n"), "yes");
    assert_eq!(cell("!!str yes"), "yes");
    // The non-specific tag `!` types as no tag (GitHub: `! 12` is 12).
    assert_eq!(cell("! 0755"), "493");
    assert_eq!(cell("! yes"), "true");
    assert_eq!(cell("! abc"), "abc");
    // A tag other than `!!str` on a quoted scalar: typed, as Psych does.
    assert_eq!(cell("!!int \"0755\""), "493");
}

#[test]
fn psych_dates_and_times() {
    for (plain, shown) in [
        ("2024-03-05", "2024-03-05"),
        ("2024-3-5", "2024-03-05"),
        ("2024-02-30", "2024-02-30"),
        ("2023-02-29", "2023-02-29"),
        ("2024-02-29", "2024-02-29"),
        ("2024-13-01", "2024-13-01"),
        ("2020-05-01T11:25:52+08:00", "2020-05-01 11:25:52 +0800"),
        ("2024-03-05 10:20:30 +02:00", "2024-03-05 10:20:30 +0200"),
        ("2024-03-05 10:20:30 -0530", "2024-03-05 10:20:30 -0530"),
        ("2024-03-05 10:20:30 +5", "2024-03-05 10:20:30 +0500"),
        ("2024-03-05 10:20:30 +130", "2024-03-05 10:20:30 +1300"),
        ("2024-03-05 10:20:30 -00:30", "2024-03-05 10:20:30 +0030"),
        ("2024-03-05 10:20:30 +24:00", "2024-03-05 10:20:30 +24:00"),
        ("2024-03-05T10:20:30Z", "2024-03-05 10:20:30 UTC"),
        ("2024-03-05t10:20:30.123456Z", "2024-03-05 10:20:30 UTC"),
        ("2024-03-05 10:20:30 Z", "2024-03-05 10:20:30 UTC"),
        // A day past the month's end runs into the next month, as with Ruby's Time.
        ("2023-02-31 00:00:00 Z", "2023-03-03 00:00:00 UTC"),
        ("2024-03-05 24:00:00 Z", "2024-03-06 00:00:00 UTC"),
        ("2024-03-05 23:59:60 Z", "2024-03-06 00:00:00 UTC"),
        ("2024-03-05 24:00:01 Z", "2024-03-05 24:00:01 Z"),
        ("2024-03-05 10:61:00 Z", "2024-03-05 10:61:00 Z"),
        ("2024-03-32 10:00:00 Z", "2024-03-32 10:00:00 Z"),
        // Without a zone: UTC, shown in Los Angeles' time (GitHub's servers).
        ("2018-08-02 00:00:00", "2018-08-01 17:00:00 -0700"),
        ("2024-03-05 10:20:30", "2024-03-05 02:20:30 -0800"),
        ("2024-01-01 00:00:00", "2023-12-31 16:00:00 -0800"),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
}

#[test]
fn los_angeles_daylight_time() {
    let at = |s: &str| typed(s);
    // 2024: from 10 March 10:00 UTC to 3 November 09:00 UTC.
    assert_eq!(at("2024-03-10 09:59:59"), "2024-03-10 01:59:59 -0800");
    assert_eq!(at("2024-03-10 10:00:00"), "2024-03-10 03:00:00 -0700");
    assert_eq!(at("2024-11-03 08:59:59"), "2024-11-03 01:59:59 -0700");
    assert_eq!(at("2024-11-03 09:00:00"), "2024-11-03 01:00:00 -0800");
    // 2007, the first year of the rule.
    assert_eq!(at("2007-03-11 10:00:00"), "2007-03-11 03:00:00 -0700");
    assert_eq!(at("2007-11-04 08:59:59"), "2007-11-04 01:59:59 -0700");
    // 2006: from the first Sunday of April (2 April) to the last of October (29 October).
    assert_eq!(at("2006-03-12 12:00:00"), "2006-03-12 04:00:00 -0800");
    assert_eq!(at("2006-04-02 10:00:00"), "2006-04-02 03:00:00 -0700");
    assert_eq!(at("2006-10-29 08:59:59"), "2006-10-29 01:59:59 -0700");
    assert_eq!(at("2006-10-29 09:00:00"), "2006-10-29 01:00:00 -0800");
    // 1987.
    assert_eq!(at("1987-04-05 10:00:00"), "1987-04-05 03:00:00 -0700");
    assert_eq!(at("1987-10-25 09:00:00"), "1987-10-25 01:00:00 -0800");
    // Before 1987 the 1987 rule is applied (Los Angeles' older history isn't kept).
    assert_eq!(at("1970-07-01 00:00:00"), "1970-06-30 17:00:00 -0700");
}

#[test]
fn symbols_and_strings() {
    for (plain, shown) in [
        (":foo", "foo"),
        (":\"a b\"", "a b"),
        (":'x'", "x"),
        (":\":y\"", "y"),
        ("::x", ":x"),
        ("+inf", "+inf"),
        ("a#b", "a#b"),
        ("-a", "-a"),
        ("C:\\x", "C:\\x"),
    ] {
        assert_eq!(typed(plain), shown, "{plain}");
    }
}

#[test]
fn binary_is_decoded() {
    // GitHub shows `!!binary` decoded (2026-11-22). Bytes that aren't UTF-8 show as U+FFFD.
    assert_eq!(cell("!!binary aGVsbG8="), "hello");
    assert_eq!(
        cell("!!binary |\n  aGVs\n  bG8gd29y\n  bGQ="),
        "hello world"
    );
    assert_eq!(cell("!!binary /w=="), "\u{fffd}");
    assert_eq!(cell("!!binary \"aGk\""), "hi");
    assert_eq!(cell("\"aGk=\""), "aGk=");
}

#[test]
fn whitespace_collapses_as_in_a_browser() {
    assert_eq!(cell("\"  a \\t\\n b  \""), "a b");
    assert_eq!(cell("|\n  one\n    two\n\n  three\n"), "one two three");
    assert_eq!(cell(">\n  folded\n  text\n"), "folded text");
    assert_eq!(cell("plain\n  over\n\n  lines"), "plain over lines");
    // A no-break space is not whitespace to collapse.
    assert_eq!(cell("\"a\\_ b\""), "a\u{a0} b");
    // NUL can't be shown.
    assert_eq!(cell("\"a\\0b\""), "a\u{fffd}b");
}

#[test]
fn merge_keys_without_aliases() {
    assert_eq!(
        table_of("<<: {a: 1, b: 2}\nb: 3\n"),
        "**a** | 1\n**b** | 3\n"
    );
    // A merged map replaces what is before it, keeping its place.
    assert_eq!(
        table_of("a: 1\n<<: {a: 2, c: 3}\n"),
        "**a** | 2\n**c** | 3\n"
    );
    // A list of maps: the first one wins.
    assert_eq!(
        table_of("<<: [{a: 1}, {a: 2, b: 2}]\n"),
        "**a** | 1\n**b** | 2\n"
    );
    // Not maps: an ordinary key.
    assert_eq!(table_of("<<: x\n"), "**<<** | x\n");
    assert_eq!(table_of("<<: [1, {a: 2}]\n"), "**<<** | • 1⏎• **a**: 2\n");
    assert_eq!(table_of("!!str <<: {a: 1}\n"), "**<<** | **a**: 1\n");
}

#[test]
fn complex_keys_show_as_ruby_inspects_them() {
    assert_eq!(table_of("? [x, 1, ~]\n: v\n"), "**[\"x\", 1, nil]** | v\n");
    assert_eq!(
        table_of("? \n  a: 1\n  :b: x\n: v\n"),
        "**{\"a\" => 1, b: \"x\"}** | v\n"
    );
    assert_eq!(
        table_of("[\"q\\\"\", 'a\\b']: v\n"),
        "**[\"q\\\"\", \"a\\\\b\"]** | v\n"
    );
    assert_eq!(
        table_of("? - :\"a b\"\n  - 1.5\n: v\n"),
        "**[:\"a b\", 1.5]** | v\n"
    );
    // Equal complex keys are one key.
    assert_eq!(table_of("[a]: 1\n[a]: 2\n"), "**[\"a\"]** | 2\n");
    // A nil key.
    assert_eq!(table_of("~: v\n"), " | v\n");
}

#[test]
fn outline_of_nested_values() {
    assert_eq!(
        cell(
            "\n  - label: Install\n    url: /docs/\n    tags: [a, b]\n  - plain\n  - [x]\n  - - deep\n    - {m: 1}"
        ),
        "• **label**: Install⏎\u{a0}\u{a0}**url**: /docs/⏎\u{a0}\u{a0}**tags**: a, b⏎• plain⏎• x⏎\
         • • deep⏎\u{a0}\u{a0}• **m**: 1"
    );
    assert_eq!(
        cell("\n  a:\n    b:\n      c: 1\n  d: [~, 2]"),
        "**a**: ⏎\u{a0}\u{a0}**b**: ⏎\u{a0}\u{a0}\u{a0}\u{a0}**c**: 1⏎**d**: 2"
    );
    // Null items draw nothing; an empty collection is an empty item (GitHub's empty table).
    assert_eq!(cell("[~, a, ~]"), "a");
    assert_eq!(cell("[~, {}, [], a]"), "• ⏎• ⏎• a");
    assert_eq!(cell("[{}, {x: 1}]"), "• ⏎• **x**: 1");
}

#[test]
fn errors_say_where() {
    let err = |yaml: &str| {
        let t = table_of(yaml);
        let e = t.strip_prefix("error: ").unwrap_or(&t);
        e.split('\n').next().unwrap().to_owned()
    };
    for (yaml, msg) in [
        // Tabs in a line's indentation: after a plain scalar, at the scalar (GitHub, 2026-11-22);
        // else at the tab.
        (
            "a: 1\n\tb: 2\n",
            "found a tab character that violates indentation while scanning a plain scalar \
             at line 1 column 4",
        ),
        (
            "title: Hello\n\t\nauthor: x\n",
            "found a tab character that violates indentation while scanning a plain scalar \
             at line 1 column 8",
        ),
        (
            "title: Hello\n\t# comment\nauthor: x\n",
            "found a tab character that violates indentation while scanning a plain scalar \
             at line 1 column 8",
        ),
        (
            "title: \"Hello\"\n\t\nauthor: x\n",
            "found character that cannot start any token while scanning for the next token \
             at line 2 column 1",
        ),
        (
            "title: \"Hello\"\n  \t\nauthor: x\n",
            "found character that cannot start any token while scanning for the next token \
             at line 2 column 3",
        ),
        (
            "a:\n  b: 1\n  \t\n  c: 2\n",
            "found a tab character that violates indentation while scanning a plain scalar \
             at line 2 column 6",
        ),
        // A line without its `:` (GitHub, 2026-11-22).
        (
            "title: x\nauthor\n",
            "could not find expected ':' while scanning a simple key at line 2 column 1",
        ),
        (
            "tags:\n  - a\n  b\n",
            "could not find expected ':' while scanning a simple key at line 3 column 3",
        ),
        (
            "b: !! 12\n",
            "did not find expected tag URI while parsing a tag at line 1 column 4",
        ),
        // After a block `-` or `?`, a tab before the content (libyaml eats only spaces there).
        (
            "tags:\n-\tone\n-\ttwo\n",
            "found character that cannot start any token while scanning for the next token \
             at line 2 column 2",
        ),
        (
            "tags:\n- \tx\n",
            "found character that cannot start any token while scanning for the next token \
             at line 2 column 3",
        ),
        (
            "? \tk\n: v\n",
            "found character that cannot start any token while scanning for the next token \
             at line 1 column 3",
        ),
        // At a sequence's column, what can't start a key is a missing `-`.
        (
            "k:\n  - a\n  - b\n  :\n",
            "did not find expected '-' indicator while parsing a block collection at line 2 \
             column 3",
        ),
        (
            "k:\n  - a\n  - b\n  ]\n",
            "did not find expected '-' indicator while parsing a block collection at line 2 \
             column 3",
        ),
        (
            "a: \"open\n",
            "found unexpected end of stream while scanning a quoted scalar at line 1 column 4",
        ),
        (
            "a: {b: 1\n",
            "did not find expected ',' or '}' while parsing a flow mapping at line 1 column 4",
        ),
        (
            "a: b: c\n",
            "mapping values are not allowed in this context at line 1 column 5",
        ),
        (
            "a: - b\n",
            "block sequence entries are not allowed in this context at line 1 column 4",
        ),
        (
            "a: 1\nb\nc: 2\n",
            "could not find expected ':' while scanning a simple key at line 2 column 1",
        ),
        (
            "a: 1\nb c\n",
            "could not find expected ':' while scanning a simple key at line 2 column 1",
        ),
        (
            "a: 1\n\"b\" c\n",
            "did not find expected key while parsing a block mapping at line 1 column 1",
        ),
        (
            "a: 1\n  b: 2\n",
            "mapping values are not allowed in this context at line 2 column 4",
        ),
        (
            "a:\n    b: 1\n  c: 2\n",
            "did not find expected key while parsing a block mapping at line 1 column 1",
        ),
        (
            "- a\nb: 1\n",
            "did not find expected '-' indicator while parsing a block collection at line 1 column 1",
        ),
        (
            "a: \"\\q\"\n",
            "found unknown escape character while parsing a quoted scalar at line 1 column 4",
        ),
        (
            "a: \"\\xZZ\"\n",
            "did not find expected hexdecimal number while parsing a quoted scalar at line 1 column 4",
        ),
        (
            "a: \"\\uD800\"\n",
            "found invalid Unicode character escape code while parsing a quoted scalar at line 1 column 4",
        ),
        (
            "a: | x\n",
            "did not find expected comment or line break while scanning a block scalar at line 1 column 4",
        ),
        (
            "a: |0\n",
            "found an indentation indicator equal to 0 while scanning a block scalar at line 1 column 4",
        ),
        (
            "a: [,]\n",
            "did not find expected node content while parsing a flow node at line 1 column 5",
        ),
        (
            "a: @x\n",
            "found character that cannot start any token while scanning for the next token at line 1 column 4",
        ),
        (
            "a: &\n",
            "did not find expected alphabetic or numeric character while scanning an anchor at line 1 column 4",
        ),
        (
            "a: !e!x y\n",
            "found undefined tag handle while parsing a node at line 1 column 4",
        ),
        (
            "a: [b\nc]: d\n",
            "mapping values are not allowed in this context at line 2 column 3",
        ),
        (
            "a: 1\n\"b\nc\": 2\n",
            "could not find expected ':' while scanning a simple key at line 2 column 1",
        ),
        (
            "a\nb: 1\n",
            "mapping values are not allowed in this context at line 2 column 2",
        ),
        (
            "a: 1\n\u{7}\n",
            "control characters are not allowed at line 1 column 1",
        ),
        (
            "é: [x\n",
            "did not find expected ',' or ']' while parsing a flow sequence at line 1 column 4",
        ),
        (
            "a: 1\n  b: 2\n@\n",
            "mapping values are not allowed in this context at line 2 column 4",
        ),
        (
            "  a: 1\n@\n",
            "found character that cannot start any token while scanning for the next \
             token at line 2 column 1",
        ),
        (
            "  a: 1\n\"open\n",
            "found unexpected end of stream while scanning a quoted scalar at \
             line 2 column 1",
        ),
    ] {
        assert_eq!(err(yaml), format!("(<unknown>): {msg}"), "{yaml:?}");
    }
    // An alias anywhere, even unused or undefined, is GitHub's alias error; a syntax error
    // anywhere comes first.
    assert_eq!(err("a: *x\n"), "Alias parsing is not enabled.");
    assert_eq!(err("- *x\n"), "Alias parsing is not enabled.");
    assert!(err("a: *x\nb: [\n").contains("flow sequence"));
    // Anchors alone are fine.
    assert_eq!(table_of("a: &x 1\n"), "**a** | 1\n");
}

#[test]
fn limits() {
    let err = |yaml: &str| table_of(yaml).lines().next().unwrap().to_owned();
    // Depth: 32 nested collections are read, 33 are not.
    let flow = |n: usize| format!("a: {}{}\n", "[".repeat(n - 1), "]".repeat(n - 1));
    assert!(!err(&flow(32)).starts_with("error"));
    assert_eq!(
        err(&flow(33)),
        "error: (<unknown>): exceeded max depth of 32 at line 1 column 35"
    );
    let mut block = String::new();
    for d in 0..40 {
        block.push_str(&format!("{}k{d}:\n", "  ".repeat(d)));
    }
    assert_eq!(
        err(&block),
        "error: (<unknown>): exceeded max depth of 32 at line 33 column 65"
    );
    let seq = format!("a: {}x\n", "- ".repeat(40));
    assert!(err(&seq).starts_with("error: (<unknown>): block sequence entries are not allowed"));
    let seq = format!("{}x\n", "- ".repeat(40));
    assert!(err(&format!("a:\n{seq}")).contains("exceeded max depth of 32"));
    // Nodes.
    let many = |n: usize| format!("a: [{}]\n", "1,".repeat(n));
    assert!(!err(&many(MAX_NODE_TEST - 3)).starts_with("error"));
    assert!(err(&many(MAX_NODE_TEST)).contains("exceeded max node count of 100000"));
    // Size.
    let big = format!("a: {}\n", "x".repeat(MAX_BYTES));
    assert_eq!(err(&big), "error: front matter over 1 MiB is not read");
}

const MAX_NODE_TEST: usize = yaml::MAX_NODES;

#[test]
fn yaml_details() {
    // Plain scalars: `#` needs a space before it to start a comment, `:` a space after it to end
    // a key.
    assert_eq!(
        table_of("a: b#c # comment\nurl: http://x:80/\n"),
        "**a** | b#c\n**url** | http://x:80/\n"
    );
    // Compact sequences, and maps in sequences.
    assert_eq!(
        table_of("tags:\n- a\n- b\nx: 1\n"),
        "**tags** | a, b\n**x** | 1\n"
    );
    assert_eq!(
        table_of("l:\n  - a: 1\n    b: 2\n  -\n    c: 3\n"),
        "**l** | • **a**: 1⏎\u{a0}\u{a0}**b**: 2⏎• **c**: 3\n"
    );
    // Flow collections over lines, with comments, trailing commas and single pairs.
    assert_eq!(
        table_of("f: [a, # one\n  b,\n  c: d, ]\ng: {x: 1, y}\n"),
        "**f** | • a⏎• b⏎• **c**: d\n**g** | **x**: 1⏎**y**: \n"
    );
    assert_eq!(cell("{\"a\":1, b:2}"), "**a**: 1⏎**b:2**: ");
    // Quoted scalars over lines fold.
    assert_eq!(cell("'one\n  two\n\n  three'"), "one two three");
    assert_eq!(cell("\"one \\\n  two\""), "one two");
    // A blank line after an escaped break is still a break: the words stay apart.
    assert_eq!(cell("\"hello\\\n\n  world\""), "hello world");
    assert_eq!(
        table_of("t: \"hello\\\r\n\r\n  world\"\r\n"),
        "**t** | hello world\n"
    );
    assert_eq!(cell("\"x\\u00e9\\U0001F600\\x41\""), "xé😀A");
    // Block scalars with indicators and comments.
    assert_eq!(cell("|2- # c\n    text\n"), "text");
    assert_eq!(table_of("a: >-\n\n  x\nb: 1\n"), "**a** | x\n**b** | 1\n");
    // Explicit keys, tags and anchors on keys.
    assert_eq!(
        table_of("? a\n: 1\n? b\nc: 2\n"),
        "**a** | 1\n**b** | \n**c** | 2\n"
    );
    assert_eq!(table_of("&k !!str 1: v\n"), "**1** | v\n");
    assert_eq!(table_of("!!int a: v\n"), "**a** | v\n");
    // Tabs after a token are blanks; a blank line of tabs is blank.
    assert_eq!(table_of("a:\tb\t\nc: d\n"), "**a** | b\n**c** | d\n");
    // After a key's `:`, tabs are blanks.
    assert_eq!(table_of("a: \tb\nc:\t\"d\"\n"), "**a** | b\n**c** | d\n");
    assert_eq!(table_of("a: b\n  \t\nc: d\n"), "**a** | b\n**c** | d\n");
    assert!(table_of("a: b\n\t\nc: d\n").starts_with("error"));
    // The root ends where its indentation does: what follows isn't read (GitHub, 2026-11-22).
    assert_eq!(table_of("  title: x\nauthor: y\n"), "**title** | x\n");
    assert_eq!(table_of("a: 1\n...\n"), "**a** | 1\n");
    assert_eq!(table_of("[a]\nb\n"), "markdown");
    // CRLF, and a `...` line ends the document.
    assert_eq!(table_of("a: 1\r\nb: 2\r\n"), "**a** | 1\n**b** | 2\n");
    assert_eq!(table_of("a: 1\rb: 2\n"), "**a** | 1\n**b** | 2\n");
    assert_eq!(table_of("a: 1\n...\nb: [\n"), "**a** | 1\n");
    // A map indented as a whole.
    assert_eq!(table_of("  a: 1\n  b: 2\n"), "**a** | 1\n**b** | 2\n");
    // A key that is a long plain scalar: still a key on one line.
    let long = "k".repeat(1500);
    assert!(table_of(&format!("{long}: v\n")).contains("mapping values are not allowed"));
}

/// The seeds of the `yaml` fuzz target, through what it runs.
#[test]
fn the_fuzz_seeds() {
    let dir = format!("{}/fuzz/seeds/yaml", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let yaml = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        if let Some(bytes) = crate::doc::Doc::fuzz_front_matter(&yaml) {
            assert!(bytes <= 256 * yaml.len() + 4096);
        }
        n += 1;
    }
    assert!(n >= 10);
    let laughs = include_str!("../../../fuzz/seeds/yaml/billion-laughs.yaml");
    assert!(table_of(laughs).starts_with("error: Alias parsing is not enabled."));
}

#[test]
fn the_source_is_shown_under_an_error() {
    let src = "\u{feff}---  \r\na: [\r\n---\t\r\n";
    let events = events(src).unwrap();
    let code = events.iter().find_map(|e| match e {
        Event::Text(t) if t.starts_with("---") => Some(t.to_string()),
        _ => None,
    });
    assert_eq!(code.unwrap(), "---  \na: [\n---\t\n");
    assert!(matches!(
        &events[5],
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(l))) if &**l == "yaml"
    ));
}
