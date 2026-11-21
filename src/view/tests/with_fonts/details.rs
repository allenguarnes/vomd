//! Collapsible sections in the window: a click on a summary, what opens them by itself, what a
//! reload keeps, and what can't be seen in a closed one.

use super::*;

const DOC: &str = "# Top\n\n<details>\n<summary>First <a href=\"https://example.com/s\">link</a> \
                   summary</summary>\n\n## Inside\n\nHidden words here.\n\n</details>\n\n\
                   <details open>\n<summary>Second summary</summary>\n\nShown words.\n\n\
                   </details>\n\nAfter the sections.\n";

/// A point on section `s`'s summary, away from its link (the start of its line).
fn summary_point(v: &View, s: usize) -> (f64, f64) {
    let (w, h) = v.size();
    (0..h)
        .flat_map(|y| (0..w / 4).map(move |x| (f64::from(x), f64::from(y))))
        .find(|&(x, y)| v.summary_at(x, y) == Some(s) && v.link_at(x, y).is_none())
        .expect("the summary on screen")
}

fn click(v: &mut View, (x, y): (f64, f64), to: (f64, f64)) -> Option<String> {
    let now = Instant::now();
    v.press(x, y, now);
    #[cfg(feature = "clipboard")]
    if to != (x, y) {
        v.select_to(to.0, to.1, now);
        v.release_select();
    }
    v.release(now);
    v.click(to.0, to.1)
}

/// A click anywhere on the summary's line opens or closes its section (the hand shows there); a
/// drag selects and doesn't; its link is followed and doesn't.
#[test]
fn a_click_on_a_summary_toggles_its_section() {
    let mut v = view(DOC, 600, 800);
    assert_eq!(v.sections_open(), [false, true]);
    let at = summary_point(&v, 0);
    assert!(v.hover(at.0, at.1) | v.over_force(), "the hand");
    assert!(v.over_force());
    let hidden_block = v.doc().sections[0].body.start as usize;
    assert!(v.layout().hidden(hidden_block));
    assert_eq!(click(&mut v, at, at), None);
    assert_eq!(v.sections_open(), [true, true]);
    assert!(!v.layout().hidden(hidden_block));
    let at = summary_point(&v, 0);
    click(&mut v, at, at);
    assert_eq!(v.sections_open(), [false, true]);
    // A drag past the slop selects.
    #[cfg(feature = "clipboard")]
    {
        let at = summary_point(&v, 0);
        click(&mut v, at, (at.0 + 40.0, at.1));
        assert_eq!(v.sections_open(), [false, true]);
    }
    // The link in the summary.
    let (w, h) = v.size();
    let link = (0..h)
        .flat_map(|y| (0..w).map(move |x| (f64::from(x), f64::from(y))))
        .find(|&(x, y)| v.link_at(x, y).is_some())
        .unwrap();
    assert_eq!(
        click(&mut v, link, link).as_deref(),
        Some("https://example.com/s")
    );
    assert_eq!(v.sections_open(), [false, true]);
}

/// A `#fragment` link, the contents panel and Back go into a closed section by opening it, as a
/// browser does; the next-heading key skips a heading in one.
#[test]
fn going_to_what_a_closed_section_holds_opens_it() {
    let mut v = view(&format!("{DOC}\n{}", paras(60)), 600, 300);
    let now = Instant::now();
    let inside = v
        .doc()
        .headings
        .iter()
        .find(|h| h.slug == "inside")
        .unwrap()
        .block;
    assert!(v.layout().hidden(inside as usize));
    v.set_anchor(Anchor::TOP);
    while v.heading(1, now) {
        assert_ne!(v.anchor().block, inside, "a hidden heading is skipped");
    }
    assert!(v.jump_to_anchor("inside", now));
    assert_eq!(v.sections_open(), [true, true]);
    assert_eq!(v.anchor().block, inside);
    // Closed again, Back to a place in it opens it.
    v.toggle_section(0);
    assert_eq!(v.sections_open(), [false, true]);
    v.set_anchor(Anchor {
        block: inside + 1,
        frac: 0.0,
    });
    assert_eq!(v.sections_open(), [true, true]);
}

/// The contents panel lists a heading in a closed section, but doesn't mark it as the one being
/// read: it can't be seen.
#[cfg(feature = "toc")]
#[test]
fn the_contents_panel_skips_hidden_headings() {
    let mut v = view(&format!("{DOC}\n{}", paras(60)), 600, 300);
    v.set_toc(true);
    v.toc_sync();
    assert_eq!(v.toc_rows(), 2, "Top and Inside");
    v.set_scroll(v.max_scroll());
    assert_eq!(v.toc_current(), Some(0));
    v.toggle_section(0);
    v.set_scroll(v.max_scroll());
    assert_eq!(v.toc_current(), Some(1));
}

/// A reading position in a section that closes moves to its summary.
#[test]
fn the_position_leaves_a_closed_section() {
    let mut v = view(&format!("{DOC}\n{}", paras(60)), 600, 300);
    let inside = v.doc().sections[0].body.start + 1;
    v.set_anchor(Anchor {
        block: inside,
        frac: 0.5,
    });
    v.toggle_section(0);
    assert_eq!(v.anchor().block, v.doc().sections[0].summary.start);
}

/// Find counts the matches in closed sections, and showing one opens its section, which stays
/// open.
#[cfg(feature = "find")]
#[test]
fn find_opens_the_section_of_its_match() {
    let mut v = view(DOC, 600, 800);
    v.set_find(true);
    v.find_type("words");
    assert_eq!(v.find_total(), 2);
    let now = Instant::now();
    // The first from the top is the hidden one.
    v.find_step(1, now);
    v.find_step(-1, now);
    assert_eq!(v.sections_open(), [true, true]);
    v.set_find(false);
    assert_eq!(v.sections_open(), [true, true], "it stays open");
}

/// Select all copies a closed section's summary and leaves its body out, as browsers do.
#[cfg(feature = "clipboard")]
#[test]
fn copy_leaves_closed_sections_out() {
    let mut v = view(DOC, 600, 800);
    v.apply(Action::SelectAll, Instant::now());
    let text = |v: &View| v.copy_content().unwrap().chunks().collect::<String>();
    let closed = text(&v);
    assert!(closed.contains("First link summary"), "{closed}");
    assert!(!closed.contains("Hidden words"), "{closed}");
    assert!(closed.contains("Shown words"), "{closed}");
    v.toggle_section(0);
    v.apply(Action::SelectAll, Instant::now());
    assert!(text(&v).contains("Hidden words"));
}

/// Sol r1, finding 1: a live reload of many sibling sections whose `open` went (the window keeps
/// them open) costs time in proportion to the document; a click on a summary whose section in
/// it is empty (finding 2) opens the outer body.
#[test]
fn reloading_many_sections_and_a_summary_with_a_section_in_it() {
    let doc = |n: usize, open: &str| {
        Doc::parse(&format!("<details{open}><summary>S</summary>body</details>\n\n").repeat(n))
    };
    crate::doc::assert_linear("a reload restoring sections", 4_000, |n| {
        let mut v = view("", 600, 400);
        v.replace(doc(n, " open"), None);
        v.replace(doc(n, ""), None);
        assert!(v.sections_open().iter().all(|&o| o));
    });
    let mut v = view(
        "<details><summary><details><summary>Inner</summary></details></summary>outer body\
         </details>\n",
        600,
        400,
    );
    let at = summary_point(&v, 0);
    click(&mut v, at, at);
    assert_eq!(v.sections_open(), [true]);
    assert!(!v.layout().hidden(1), "the outer body shows");
}

/// A reload keeps each section open or closed, found by its summary's text and its place among
/// those with that text, so a section added above changes nothing; a new section starts as it
/// says.
#[test]
fn a_reload_keeps_the_sections() {
    let mut v = view(DOC, 600, 800);
    v.toggle_section(0);
    v.toggle_section(1);
    assert_eq!(v.sections_open(), [true, false]);
    let new = format!(
        "<details open><summary>Added</summary>\n\nNew.\n\n</details>\n\n{DOC}\n\n\
         <details><summary>Second summary</summary>\n\nA repeat.\n\n</details>\n"
    );
    v.replace(Doc::parse(&new), None);
    assert_eq!(v.sections_open(), [true, true, false, false]);
    let hidden = v.doc().sections[2].body.start as usize;
    assert!(v.layout().hidden(hidden), "the layout has the states");
    // Through a message (the file briefly missing).
    v.replace(Doc::default(), Some("gone".into()));
    v.replace(Doc::parse(&new), None);
    assert_eq!(v.sections_open(), [true, true, false, false]);
    // Zoom and the wide layout keep them.
    v.set_zoom(150);
    v.set_wide(true);
    assert!(v.layout().hidden(hidden));
}

/// What a closed section holds can't be seen: the viewer doesn't step to its images, and they
/// are never asked for.
#[cfg(feature = "images")]
#[test]
fn images_in_closed_sections_are_out_of_reach() {
    let md = |open: &str| {
        format!(
            "![big](big.png)\n\n<details{open}><summary>S</summary>\n\n![other](other.png)\n\n\
             </details>\n\n![wide](wide.png)\n\n{}",
            paras(3)
        )
    };
    let (mut v, dir) = image_view("details", &md(" open"), 600, 800);
    assert_eq!(v.viewer_list(), [1, 2, 3]);
    v.toggle_section(0);
    assert_eq!(v.viewer_list(), [1, 3]);
    assert!(v.hidden_images().contains(&2));
    // Closed from the start, its image is never looked at.
    let mut v = view(&md(""), 600, 800);
    v.set_image_base(Some(dir.clone()));
    for _ in 0..10 {
        frame(&mut v, Instant::now());
        v.poll_images(Instant::now());
    }
    assert_eq!(v.layout.image_size(2), None, "never measured");
    assert!(!v.images.shown(2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation in a closed section doesn't play: nothing is due while it is closed.
#[cfg(feature = "images")]
#[test]
fn animations_in_closed_sections_wait() {
    let dir = std::env::temp_dir().join(format!("vomd-details-anim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let gif = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/anim.gif");
    std::fs::copy(gif, dir.join("anim.gif")).unwrap();
    let md = "<details><summary>S</summary>\n\n![a](anim.gif)\n\n</details>\n\nText.\n";
    let mut v = view(md, 600, 400);
    v.set_image_base(Some(dir.clone()));
    v.images.set_animations(true);
    for _ in 0..20 {
        frame(&mut v, Instant::now());
        v.poll_images(Instant::now());
        assert_eq!(v.anim_deadline(), None);
        std::thread::sleep(Duration::from_millis(2));
    }
    v.toggle_section(0);
    let t = Instant::now();
    loop {
        frame(&mut v, Instant::now());
        v.poll_images(Instant::now());
        if v.anim_deadline().is_some() {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(20), "plays once shown");
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
