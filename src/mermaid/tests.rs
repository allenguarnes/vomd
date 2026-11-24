use super::*;

/// A flowchart of every feature the layout must handle: subgraphs with their own `direction`, an edge to a subgraph,
/// `a --> b & c & d`, dotted and two-way labelled edges, `<b>`, `<i>`, `<br/>` and an emoji.
const CAFE: &str = include_str!("../../fixtures/mermaid-cafe.mmd");

fn metrics() -> Metrics {
    Metrics::of(&crate::fonts::Fonts::new().unwrap())
}

fn overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn inside(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3]
}

fn geometry(src: &str) -> mermaid_svg::vomd::Geometry {
    let m = metrics();
    mermaid_svg::vomd::set_measure(Some(Box::new(move |s| m.measure(s))));
    let g = mermaid_svg::vomd::flowchart_geometry(src).expect("a flowchart");
    mermaid_svg::vomd::set_measure(None);
    g
}

/// No two nodes overlap; a subgraph's box holds its nodes and no other, overlaps no other
/// subgraph it isn't inside of (or around) and no node outside it; every edge is routed.
fn check_layout(g: &mermaid_svg::vomd::Geometry) {
    let n = &g.nodes;
    for i in 0..n.len() {
        for j in i + 1..n.len() {
            assert!(!overlap(n[i].1, n[j].1), "{} overlaps {}", n[i].0, n[j].0);
        }
    }
    for (id, b, members) in &g.groups {
        for (nid, nb) in n {
            if members.contains(nid) {
                assert!(inside(*nb, *b), "{nid} is outside its subgraph {id}");
            } else {
                assert!(!overlap(*nb, *b), "{nid} overlaps subgraph {id}");
            }
        }
        for (other, ob, om) in &g.groups {
            if other == id {
                continue;
            }
            let nested =
                members.iter().all(|m| om.contains(m)) || om.iter().all(|m| members.contains(m));
            if !nested {
                assert!(!overlap(*b, *ob), "subgraph {id} overlaps subgraph {other}");
            }
        }
    }
    for (from, to, route) in &g.edges {
        let route = route
            .as_ref()
            .unwrap_or_else(|| panic!("{from} --> {to} is missing"));
        assert!(route.len() >= 2, "{from} --> {to} has no route");
    }
}

#[test]
fn the_reference_flowchart_is_laid_out_cleanly() {
    let g = geometry(CAFE);
    assert_eq!(g.nodes.len(), 12);
    assert_eq!(g.groups.len(), 2);
    // `order --> brew --> kitchen` and `machine --> cups & milk & syrup & napkins` are 3 and 4 edges.
    assert_eq!(g.edges.len(), 12);
    check_layout(&g);
    let node = |id: &str| g.nodes.iter().find(|n| n.0 == id).unwrap().1;
    let group = |id: &str| g.groups.iter().find(|n| n.0 == id).unwrap().1;
    // Top to bottom: the order, its brewing, the kitchen, the counter.
    assert!(node("order")[3] < node("brew")[1]);
    assert!(node("brew")[3] < group("kitchen")[1]);
    assert!(group("kitchen")[3] < group("counter")[1]);
    // The edge to the subgraph ends on its box, from above.
    let (_, _, to_kitchen) = g.edges.iter().find(|e| e.1 == "kitchen").unwrap();
    let end = *to_kitchen.as_ref().unwrap().last().unwrap();
    assert!(
        (end.1 - group("kitchen")[1]).abs() < 1.0,
        "{end:?} isn't on the box's top"
    );
    // The counter's four nodes are side by side, on one rank.
    let ys: Vec<f64> = ["cups", "milk", "syrup", "napkins"]
        .iter()
        .map(|n| node(n)[1] + node(n)[3])
        .collect();
    assert!(ys.windows(2).all(|w| (w[0] - w[1]).abs() < 1.0), "{ys:?}");
}

#[test]
fn subgraphs_alone_take_their_direction() {
    let src = "flowchart LR\n  subgraph one[First]\n    direction TB\n    a1 --> a2\n  end\n  \
               subgraph two\n    b1 --> b2\n  end\n  subgraph three\n    subgraph inner\n      \
               c1 --> c2\n    end\n    c3\n  end\n  one --> two\n  two --> three\n  c2 --> c3\n  \
               x((circle)) --> y{Decide?}\n  y -->|yes| z[/para/]\n  y -->|no| one\n  y ==> z\n  \
               a1 --> a1\n";
    let g = geometry(src);
    check_layout(&g);
    let node = |id: &str| g.nodes.iter().find(|n| n.0 == id).unwrap().1;
    // `direction TB` in an LR chart: a2 under a1.
    assert!(node("a1")[3] < node("a2")[1]);
    assert!((node("a1")[0] - node("a2")[0]).abs() < 1.0);
}

#[test]
fn labels_are_measured_by_vomds_font() {
    // A wide and a narrow label of as many characters: the boxes differ (an estimate by
    // character count would make them the same).
    let g = geometry("flowchart TD\n  a[WWWWWWWWWW] --> b[iiiiiiiiii]\n");
    let w = |id: &str| {
        let b = g.nodes.iter().find(|n| n.0 == id).unwrap().1;
        b[2] - b[0]
    };
    assert!(w("a") > w("b") + 40.0, "{} vs {}", w("a"), w("b"));
}

/// Small diagrams of every type shown.
pub(crate) const SHOWN: &[(&str, &str)] = &[
    (
        "flowchart",
        "flowchart LR\n  a[Start] --> b{Ok?}\n  b -->|yes| c(Done)\n  b -.->|no| a\n",
    ),
    (
        "sequence",
        "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi\n  Note over Alice,Bob: done\n",
    ),
    (
        "class",
        "classDiagram\n  Animal <|-- Dog\n  class Dog {\n    +bark()\n  }\n",
    ),
    (
        "state",
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go\n  Busy --> [*]\n",
    ),
    ("er", "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n"),
    (
        "gantt",
        "gantt\n  dateFormat YYYY-MM-DD\n  section A\n  One :a1, 2026-01-01, 3d\n  Two :after a1, 2d\n",
    ),
    ("pie", "pie title Pets\n  \"Dogs\" : 3\n  \"Cats\" : 2\n"),
    (
        "mindmap",
        "mindmap\n  root((Root))\n    A\n    B\n      C\n",
    ),
    (
        "timeline",
        "timeline\n  title History\n  2002 : One\n  2004 : Two\n",
    ),
    (
        "gitGraph",
        "gitGraph\n  commit\n  branch dev\n  commit\n  checkout main\n  merge dev\n",
    ),
    (
        "journey",
        "journey\n  title Day\n  section Morning\n    Coffee: 5: Me\n",
    ),
    (
        "quadrant",
        "quadrantChart\n  x-axis Low --> High\n  y-axis Low --> High\n  A: [0.3, 0.6]\n",
    ),
    (
        "xychart",
        "xychart-beta\n  x-axis [a, b, c]\n  y-axis 0 --> 10\n  bar [1, 5, 3]\n",
    ),
];

#[test]
fn every_type_shown_makes_an_svg_in_both_themes() {
    let m = metrics();
    for (name, src) in SHOWN {
        for dark in [false, true] {
            let svg = make(src, dark, &m).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(svg.starts_with("<svg"), "{name}");
            // A fixed size, never the responsive envelope.
            assert!(!svg[..200.min(svg.len())].contains("100%"), "{name}");
            // Drawn by VOMD's SVG code.
            let (_, w, h) = crate::image::header_of(svg.as_bytes()).expect(name);
            assert!(w > 0 && h > 0, "{name}");
        }
    }
}

#[test]
fn types_not_shown_say_so() {
    let m = metrics();
    for (src, kind) in [
        ("sankey-beta\na,b,1\n", "sankey"),
        (
            "requirementDiagram\n  requirement r {\n    id: 1\n  }\n",
            "requirement",
        ),
        ("C4Context\n  Person(a, \"A\")\n", "C4"),
        ("block-beta\n  a b\n", "block"),
    ] {
        assert_eq!(
            make(src, false, &m),
            Err(format!("{kind} diagrams aren't supported"))
        );
    }
}

#[test]
fn a_broken_diagram_says_where() {
    let m = metrics();
    let e = make("flowchart TD\n  a --> b\n  c[unclosed --> d\n", false, &m).unwrap_err();
    assert!(e.starts_with("line 3: "), "{e}");
    let e = make("nonsense\n", false, &m).unwrap_err();
    assert!(e.starts_with("unknown diagram type"), "{e}");
    assert_eq!(make("", false, &m).unwrap_err(), "the diagram is empty");
}

#[test]
fn limits() {
    let m = metrics();
    // Over the source cap: one name, which says so; nothing is made.
    let big = format!(
        "flowchart TD\n{}",
        "  a --> b\n".repeat(MAX_SOURCE / 10 + 1)
    );
    assert!(big.len() > MAX_SOURCE);
    let [l, d] = srcs(&big);
    assert_eq!(l, d);
    assert!(note(&l).unwrap().contains("too large"));
    assert!(make(&big, false, &m).is_err());
    // Over the layout's node cap (within the source cap): too complex.
    let mut many = String::from("flowchart TD\n");
    for i in 0..2100 {
        many += &format!("n{i}\n");
    }
    assert!(many.len() <= MAX_SOURCE);
    assert_eq!(
        make(&many, false, &m).unwrap_err(),
        "the diagram is too complex"
    );
    // A dense graph (every pair of 80 nodes in order: 3,160 edges, under the caps) runs out
    // of the work budget, quickly, and says so.
    let mut dense = String::from("flowchart TD\n");
    for i in 0..80 {
        for j in i + 1..80 {
            dense += &format!("n{i}-->n{j}\n");
        }
    }
    assert!(dense.len() <= MAX_SOURCE, "{}", dense.len());
    let t = Instant::now();
    assert_eq!(
        make(&dense, false, &m).unwrap_err(),
        "the diagram is too complex"
    );
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

#[test]
fn the_theme_is_githubs_unless_the_diagram_names_one() {
    let m = metrics();
    let src = "flowchart TD\n  a --> b\n";
    let light = make(src, false, &m).unwrap();
    let dark = make(src, true, &m).unwrap();
    assert_ne!(light, dark);
    // Mermaid's `default` theme's node fill, and the `dark` theme's.
    assert!(light.contains("#ECECFF"), "{light}");
    assert!(!dark.contains("#ECECFF"));
    // `%%{init}%%` and front matter win over VOMD's theme.
    let init = format!("%%{{init: {{'theme': 'dark'}}}}%%\n{src}");
    assert_eq!(
        make(&init, false, &m).unwrap(),
        make(&init, true, &m).unwrap()
    );
    assert!(!make(&init, false, &m).unwrap().contains("#ECECFF"));
    let front = format!("---\nconfig:\n  theme: forest\n---\n{src}");
    assert_eq!(
        make(&front, false, &m).unwrap(),
        make(&front, true, &m).unwrap()
    );
}

#[test]
fn names_are_one_per_source_and_theme() {
    let [l, d] = srcs("flowchart TD\n  a --> b\n");
    assert_ne!(l, d);
    assert!(is_diagram(&l) && is_diagram(&d));
    assert_eq!(srcs("flowchart TD\n  a --> b\n"), [l.clone(), d]);
    assert_ne!(srcs("flowchart TD\n  a --> c\n")[0], l);
    // Not a file's path: no file system takes a NUL.
    let p = virtual_path(&l).unwrap();
    assert!(p.to_str().unwrap().starts_with('\0'));
    assert!(is_virtual(&p));
    assert!(virtual_path("images/a.png").is_none());
}

#[test]
fn the_job_and_result_round_trip() {
    let m = metrics();
    let job = put_job("pie\n  \"a\" : 1\n", true, &m, 0);
    let (src, dark, back, test) = get_job(&job).unwrap();
    assert_eq!((src.as_str(), dark, test), ("pie\n  \"a\" : 1\n", true, 0));
    assert_eq!(back, m);
    assert!(get_job(&job[..job.len() - 1]).is_none());
    for r in [Ok("<svg/>".to_owned()), Err("line 1: x".to_owned())] {
        assert_eq!(get_result(&put_result(&r)), Some(r));
    }
    assert!(get_result(&[7, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
}

#[test]
fn a_panic_here_is_a_failure() {
    let r = guarded(|| panic!("a test's panic"));
    assert_eq!(r, Err(WHY_BROKEN.to_owned()));
    assert!(make_here("pie\n  \"a\" : 1\n", false, &Metrics::default()).is_ok());
}

#[test]
fn the_maker_makes_each_diagram_once() {
    let src = "```mermaid\nflowchart TD\n  maker --> once\n```\n";
    let doc = crate::doc::Doc::parse(src);
    let key_light = key("flowchart TD\n  maker --> once", false);
    let mut maker = Maker::default();
    maker.request(&doc);
    let t = Instant::now();
    while pending(&doc.images[0].src) {
        assert!(t.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(note(&doc.images[0].src).is_none());
    let path = virtual_path(&doc.images[0].src).unwrap();
    let made = bytes(&path).unwrap();
    assert_eq!(stamp(&path), stamp(&path));
    // A reload of the same text: the same name, made already, not again.
    let again = crate::doc::Doc::parse(src);
    assert_eq!(again.images[0].src, doc.images[0].src);
    maker.request(&again);
    assert!(Arc::ptr_eq(&bytes(&path).unwrap(), &made));
    assert!(matches!(
        store().map.get(&key_light),
        Some((State::Done(..), _))
    ));
    // The dark theme's is another, made when it is shown.
    let mut dark = again;
    dark.set_dark(true);
    assert_ne!(dark.images[0].src, doc.images[0].src);
    assert!(!pending(&dark.images[0].src));
    assert!(bytes(&virtual_path(&dark.images[0].src).unwrap()).is_none());
}

#[test]
fn the_store_keeps_to_its_budget() {
    let mut s = Store::default();
    let chunk = Arc::new(vec![0u8; STORE_BYTES / 4]);
    for k in 0..10 {
        s.put(1_000 + k, State::Done(chunk.clone(), 0));
        assert!(s.bytes <= STORE_BYTES);
    }
    assert!(s.map.contains_key(&1_009));
    assert!(!s.map.contains_key(&1_000));
}

#[test]
fn metrics_are_the_bold_faces() {
    let m = metrics();
    assert!(m.measure("WWW") > m.measure("iii") * 2.0);
    // CJK and emoji are a full em; anything else outside the table, an estimate.
    assert_eq!(m.measure("中"), 1.0);
    assert!(m.measure("\u{10000}") > 0.3);
}

/// The fuzz target's seeds, findings included: made without a panic, within the budget.
#[test]
fn the_fuzz_seeds_are_made() {
    let dir = format!("{}/fuzz/seeds/mermaid", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let data = std::fs::read(entry.unwrap().path()).unwrap();
        let Some((&b, rest)) = data.split_first() else {
            continue;
        };
        let src = String::from_utf8_lossy(rest);
        let r = make(&src, b & 1 != 0, &Metrics::default());
        if let Ok(svg) = r {
            assert!(crate::image::header_of(svg.as_bytes()).is_some());
        }
        n += 1;
    }
    assert!(n >= 20);
}

#[test]
fn autonumber_digits_read_in_both_themes() {
    let m = metrics();
    let src = "sequenceDiagram\n  autonumber\n  A->>B: hi\n";
    // Light: white digits on the near-black badge; dark: black ones on the light grey one.
    let light = make(src, false, &m).unwrap();
    let dark = make(src, true, &m).unwrap();
    assert!(light.contains("fill=\"#fff\" font-size=\"11\""), "{light}");
    assert!(dark.contains("fill=\"#000\" font-size=\"11\""), "{dark}");
}

#[test]
fn a_state_note_is_level_with_its_state_on_its_side() {
    let src = "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go\n  Busy --> Idle : done\n  \
               Busy --> [*]\n  note right of Idle : waiting here\n  note left of Busy : working\n";
    let m = metrics();
    mermaid_svg::vomd::set_measure(Some(Box::new(move |s| m.measure(s))));
    let boxes = mermaid_svg::vomd::state_note_boxes(src);
    mermaid_svg::vomd::set_measure(None);
    assert_eq!(boxes.len(), 2);
    let mid = |b: [f64; 4]| (b[1] + b[3]) / 2.0;
    let (state, note) = boxes[0];
    assert!(
        note[0] >= state[2] && note[0] - state[2] < 30.0,
        "right: {state:?} {note:?}"
    );
    assert!(
        (mid(state) - mid(note)).abs() < 1.0,
        "level: {state:?} {note:?}"
    );
    let (state, note) = boxes[1];
    assert!(
        note[2] <= state[0] && state[0] - note[2] < 30.0,
        "left: {state:?} {note:?}"
    );
    assert!(
        (mid(state) - mid(note)).abs() < 1.0,
        "level: {state:?} {note:?}"
    );
}

#[test]
fn a_name_nobody_asked_for_isnt_loading() {
    // An image written by hand with a diagram's name: its alt text, not a box loading forever.
    assert!(!pending("vomd-mermaid:light/0123456789abcdef"));
    assert!(note("vomd-mermaid:light/0123456789abcdef").is_none());
}

#[test]
fn a_document_makes_at_most_its_first_diagrams() {
    let src: String = (0..MAX_DIAGRAMS + 2)
        .map(|i| format!("```mermaid\npie\n  \"a{i}\" : 1\n```\n\n"))
        .collect();
    let doc = crate::doc::Doc::parse(&src);
    assert_eq!(doc.images.len(), MAX_DIAGRAMS + 2);
    assert!(note(&doc.images[MAX_DIAGRAMS - 1].src).is_none());
    let past = note(&doc.images[MAX_DIAGRAMS].src).unwrap();
    assert!(past.contains("first 100"), "{past}");
}

#[test]
fn passing_failures_are_tried_again() {
    let mut s = store();
    let (slow, broken, wrong) = (u64::MAX - 1, u64::MAX - 2, u64::MAX - 3);
    s.put(slow, State::Failed(WHY_SLOW.into(), true));
    s.put(broken, State::Failed(WHY_BROKEN.into(), true));
    s.put(wrong, State::Failed("line 1: x".into(), false));
    drop(s);
    forget_transient();
    let s = store();
    assert!(!s.map.contains_key(&slow) && !s.map.contains_key(&broken));
    assert!(s.map.contains_key(&wrong));
}

#[test]
fn what_a_document_shows_is_never_evicted() {
    let mut s = Store::default();
    let chunk = Arc::new(vec![0u8; STORE_BYTES / 4]);
    // Ten shown, together past the budget: all kept.
    s.shown.insert(1, (0..10).collect());
    for k in 0..10 {
        s.put(k, State::Done(chunk.clone(), 0));
    }
    assert!((0..10).all(|k| s.map.contains_key(&k)));
    // More than 4,096 entries: the shown ones stay, the number of the rest is cut.
    s.shown.insert(2, (10_000..10_050).collect());
    for k in 10_000..15_000 {
        s.put(k, State::Failed("line 1: x".into(), false));
    }
    assert!((10_000..10_050).all(|k| s.map.contains_key(&k)));
    assert!((0..10).all(|k| s.map.contains_key(&k)));
    assert!(s.map.len() <= 4_097);
}

/// Poll `view` as the window does until its diagrams are made and nothing changes for a
/// while: the number of polls that changed something. Panics if it never settles.
fn settle(view: &mut crate::view::View) -> usize {
    let (w, h) = (600, 400);
    view.resize(w, h);
    let mut px = vec![0u32; (w * h) as usize];
    let (mut changes, mut quiet) = (0, 0);
    let t = Instant::now();
    while quiet < 100 {
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "never settled ({changes} changes)"
        );
        view.ensure();
        view.paint(&mut px, w as usize, Instant::now());
        let now = Instant::now();
        view.tick(now, Some(now));
        std::thread::sleep(Duration::from_millis(5));
        let pending = view.doc().images.iter().any(|i| pending(&i.src));
        if view.poll_images(Instant::now()) || pending {
            changes += 1;
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    changes
}

fn view_of(src: &str) -> crate::view::View {
    crate::view::View::new(
        crate::fonts::Fonts::new().unwrap(),
        crate::doc::Doc::parse(src),
        None,
        (600, 400),
        1.0,
    )
}

#[test]
fn diagrams_past_the_store_settle() {
    // Ten different journeys of about 45 KB, about 0.9 MB of SVG each: more than the store
    // keeps (the tests' store is 4 MB).
    let mut src = String::new();
    for n in 0..10 {
        src += &format!("```mermaid\njourney\n  title Day {n}\n");
        let mut body = String::new();
        let mut i = 0;
        while body.len() < 45_000 {
            if i % 20 == 0 {
                body += &format!("  section Part {i}\n");
            }
            body += &format!("    Task number {i} of the day: {}: Me, You\n", i % 5 + 1);
            i += 1;
        }
        src += &body;
        src += "```\n\n";
    }
    let mut view = view_of(&src);
    settle(&mut view);
    let made: usize = view
        .doc()
        .images
        .iter()
        .filter_map(|i| virtual_path(&i.src))
        .filter_map(|p| bytes(&p))
        .map(|b| b.len())
        .sum();
    assert!(made > STORE_BYTES, "{made} bytes made");
    // Once settled, nothing is made again.
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(500) {
        assert!(!view.poll_images(Instant::now()), "made again");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_theme_switch_makes_the_other_themes_diagram() {
    let src = "```mermaid\nflowchart TD\n  theme --> switch\n```\n";
    let mut view = view_of(src);
    view.set_theme_mode(crate::view::ThemeMode::Light);
    settle(&mut view);
    let light = view.doc().images[0].src.clone();
    assert!(bytes(&virtual_path(&light).unwrap()).is_some());
    view.set_theme_mode(crate::view::ThemeMode::Dark);
    let dark = view.doc().images[0].src.clone();
    assert_ne!(dark, light);
    settle(&mut view);
    assert!(bytes(&virtual_path(&dark).unwrap()).is_some());
}

#[test]
fn new_faces_make_the_diagrams_again() {
    let src = "```mermaid\nflowchart TD\n  faces --> again\n```\n";
    let doc = crate::doc::Doc::parse(src);
    let path = virtual_path(&doc.images[0].src).unwrap();
    let mut maker = Maker::default();
    let wait = || {
        let t = Instant::now();
        while pending(&doc.images[0].src) || bytes(&path).is_none() {
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    maker.request(&doc);
    wait();
    let before = bytes(&path).unwrap();
    // Twice as wide: the boxes grow.
    let mut wide = Metrics::default();
    wide.fallback *= 2.0;
    assert!(maker.set_metrics(wide));
    maker.forget(&doc);
    maker.request(&doc);
    wait();
    let after = bytes(&path).unwrap();
    assert_ne!(*before, *after);
}

#[test]
fn a_result_with_old_metrics_lets_the_diagram_be_made_again() {
    let mut s = Store::default();
    let key = 77;
    s.put(key, State::Pending);
    finish(&mut s, 1, key, (false, 0), Ok("<svg/>".into()));
    // Not left "being made": the next request makes it again.
    assert!(!s.map.contains_key(&key));
    // With the current ones it is kept, whatever the store said meanwhile.
    finish(&mut s, 1, key, (true, 0), Ok("<svg/>".into()));
    assert!(matches!(s.map.get(&key), Some((State::Done(..), _))));
}

#[test]
fn a_documents_diagrams_have_a_byte_limit() {
    let mut s = Store::default();
    s.shown.insert(1, [1, 2, 3].into_iter().collect());
    s.put(1, State::Pending);
    s.put(2, State::Pending);
    s.put(3, State::Pending);
    let svg = |n: usize| Ok("x".repeat(n));
    finish(&mut s, 1, 1, (true, 0), svg(PINNED_BYTES / 2));
    finish(&mut s, 1, 2, (true, 0), svg(PINNED_BYTES / 4));
    assert!(matches!(s.map.get(&2), Some((State::Done(..), _))));
    // Past the limit with the others: shown as code, and tried again on a reload.
    finish(&mut s, 1, 3, (true, 0), svg(PINNED_BYTES / 3));
    assert!(matches!(s.map.get(&3), Some((State::Failed(w, true), _)) if &**w == WHY_TOGETHER));
}

#[test]
fn a_diagram_refused_when_drawn_is_code() {
    let src = srcs("flowchart TD\n  refused --> drawn\n")[0].clone();
    let Some(Name::Diagram(key)) = name(&src) else {
        panic!()
    };
    assert!(!refused(&src, "the diagram is too large to draw"));
    store().put(key, State::Done(Arc::new(b"<svg/>".to_vec()), 0));
    assert!(!shown_as_code(&src));
    assert!(refused(&src, "the diagram is too large to draw"));
    assert!(shown_as_code(&src));
    assert_eq!(
        note(&src).unwrap(),
        "Mermaid: the diagram is too large to draw"
    );
}

#[test]
fn edges_go_round_notes() {
    let src = "stateDiagram-v2\n  [*] --> Done\n  state f <<fork>>\n  Done --> f\n  f --> A\n  \
               f --> B\n  note left of A : a note on the left\n  note right of B : and one on the right\n  \
               A --> A : self\n  B --> A\n";
    let m = metrics();
    mermaid_svg::vomd::set_measure(Some(Box::new(move |s| m.measure(s))));
    let notes = mermaid_svg::vomd::state_note_boxes(src);
    let edges = mermaid_svg::vomd::state_edges(src);
    mermaid_svg::vomd::set_measure(None);
    assert_eq!(notes.len(), 2);
    assert!(!edges.is_empty());
    // Whether segment p–q crosses box b (sampled finely: the boxes are tens of px).
    let crosses = |p: (f64, f64), q: (f64, f64), b: [f64; 4]| {
        (0..=200).any(|i| {
            let t = f64::from(i) / 200.0;
            let (x, y) = (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
            x > b[0] + 0.5 && x < b[2] - 0.5 && y > b[1] + 0.5 && y < b[3] - 0.5
        })
    };
    for e in &edges {
        for w in e.windows(2) {
            for (_, note) in &notes {
                assert!(!crosses(w[0], w[1], *note), "{e:?} crosses {note:?}");
            }
        }
    }
}

#[test]
fn colours_are_judged_light_or_dark() {
    use mermaid_svg::vomd::is_light;
    for c in [
        "#fff",
        "#ECECFF",
        "white",
        "rgb(250, 250, 250)",
        "rgba(200,200,200,0.5)",
        "yellow",
    ] {
        assert!(is_light(c), "{c}");
    }
    for c in [
        "#333",
        "#000000",
        "black",
        "rgb(10 10 10)",
        "navy",
        "nonsense",
    ] {
        assert!(!is_light(c), "{c}");
    }
}

#[cfg(feature = "find")]
#[test]
fn find_follows_a_diagram_that_fails_into_code() {
    lock(&HELD).push("SearchSentinel".into());
    let mut view = view_of("```mermaid\nnoSuchKind SearchSentinel\n```\n");
    view.set_find(true);
    view.find_type("SearchSentinel");
    // Being made: a picture to be, not searched.
    assert!(pending(&view.doc().images[0].src));
    assert_eq!(view.find_total(), 0);
    lock(&HELD).retain(|h| h != "SearchSentinel");
    settle(&mut view);
    assert!(shown_as_code(&view.doc().images[0].src));
    // Shown as its code now: found, the query untouched.
    assert_eq!(view.find_total(), 1);
}

#[test]
fn diagrams_made_before_a_font_change_are_made_again() {
    // A, then B shown, the faces change (B's diagrams forgotten), then A again.
    let a =
        crate::doc::Doc::parse("```mermaid\nflowchart TD\n  oldmetrics[WWWWWWWWWW] --> a\n```\n");
    let b = crate::doc::Doc::parse("```mermaid\nflowchart TD\n  other --> b\n```\n");
    let path = virtual_path(&a.images[0].src).unwrap();
    let made = |doc: &crate::doc::Doc| {
        let t = Instant::now();
        while pending(&doc.images[0].src) {
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let mut maker = Maker::default();
    maker.request(&a);
    made(&a);
    let before = bytes(&path).unwrap();
    maker.request(&b);
    made(&b);
    let mut wide = Metrics::default();
    wide.fallback *= 2.0;
    assert!(maker.set_metrics(wide.clone()));
    maker.forget(&b);
    maker.request(&b);
    made(&b);
    maker.request(&a);
    made(&a);
    let after = bytes(&path).unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(
        *after,
        make(&a.images[0].diagram.clone().unwrap(), false, &wide)
            .unwrap()
            .into_bytes()
    );
}

#[test]
fn each_maker_hears_of_its_own_diagrams() {
    // Two windows show the same diagram; the first asks for it, the second finds it being made.
    // Each is told when it is done, and taking one's news leaves the other's.
    lock(&HELD).push("bothmakers".into());
    let doc = crate::doc::Doc::parse("```mermaid\nflowchart TD\n  bothmakers --> told\n```\n");
    let other = crate::doc::Doc::parse("```mermaid\nflowchart TD\n  elsewhere --> b\n```\n");
    let (mut a, mut b, mut c) = (Maker::default(), Maker::default(), Maker::default());
    a.request(&doc);
    b.request(&doc);
    c.request(&other);
    let wait = |cond: &dyn Fn() -> bool| {
        let t = Instant::now();
        while !cond() {
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&|| c.shared.finished.load(Ordering::SeqCst));
    // A third window's diagram is none of theirs.
    assert!(!a.take_finished() && !b.take_finished());
    assert!(c.take_finished());
    lock(&HELD).retain(|h| h != "bothmakers");
    wait(&|| a.shared.finished.load(Ordering::SeqCst) && b.shared.finished.load(Ordering::SeqCst));
    assert!(!pending(&doc.images[0].src));
    assert!(a.take_finished());
    assert!(!a.take_finished());
    assert!(b.take_finished());
    assert!(!c.take_finished());
}
