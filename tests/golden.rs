//! Golden-image render tests: fixtures painted headless and compared with
//! `tests/golden/*.png`, allowing at most 0.5% of pixels to differ by more than 8/255 in some
//! channel. Only the embedded fonts are used, so the images don't depend on the machine.
//!
//! Regenerate with `cargo xtask bless` (sets `VOMD_BLESS=1`). On a mismatch the actual image
//! and a diff are written to `target/golden-out/`.
#![cfg(feature = "embed-fonts")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::image::Bitmap;
use vomd::layout::{Anchor, ImageSize, Layout, Params};
use vomd::render::{self, Clip, Frame, Pixels, Viewport};
#[cfg(feature = "clipboard")]
use vomd::select::Pos;
use vomd::text::cache::GlyphCache;
use vomd::theme::{DARK, LIGHT, Theme};
#[cfg(feature = "clipboard")]
use vomd::view::{ThemeMode, View};

/// Tallest golden image; documents are cut off below this.
const MAX_H: usize = 4000;
const TOLERANCE: u8 = 8;
/// Maximum share of pixels allowed past the tolerance.
const MAX_BAD: f64 = 0.005;

struct Case {
    name: &'static str,
    fixture: &'static str,
    /// Logical width; the image is `width × scale` pixels wide.
    width: f32,
    scale: f32,
    theme: &'static Theme,
    /// `Some(h)`: a window `h` logical pixels tall scrolled to the middle of the document, with
    /// the scrollbar painted. `None`: the whole document from the top, no scrollbar.
    view: Option<f32>,
    /// `Some((from, to))`: the text from the start of the first `from` to the end of the first
    /// `to` is selected, and images are shown.
    select: Option<(&'static str, &'static str)>,
}

const CASES: &[Case] = &[
    Case {
        name: "golden-light",
        fixture: "golden.md",
        width: 760.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: None,
    },
    Case {
        name: "golden-dark",
        fixture: "golden.md",
        width: 760.0,
        scale: 1.0,
        theme: &DARK,
        view: None,
        select: None,
    },
    Case {
        name: "golden-light-2x",
        fixture: "golden.md",
        width: 760.0,
        scale: 2.0,
        theme: &LIGHT,
        view: None,
        select: None,
    },
    Case {
        name: "golden-narrow",
        fixture: "golden.md",
        width: 360.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: None,
    },
    Case {
        name: "golden-scrolled-1.5x",
        fixture: "golden.md",
        width: 760.0,
        scale: 1.5,
        theme: &LIGHT,
        view: Some(600.0),
        select: None,
    },
    Case {
        name: "tables-dark",
        fixture: "tables.md",
        width: 900.0,
        scale: 1.0,
        theme: &DARK,
        view: None,
        select: None,
    },
    Case {
        name: "inline",
        fixture: "inline.md",
        width: 700.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: None,
    },
    Case {
        name: "tiny",
        fixture: "tiny.md",
        width: 640.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: None,
    },
    // From inside the first paragraph to the middle of the table: the image washed, the list's
    // bullets, the code block's tab and blank line, the marked line ends.
    #[cfg(feature = "clipboard")]
    Case {
        name: "selection-light",
        fixture: "selection.md",
        width: 640.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: Some(("middle", "three")),
    },
    #[cfg(feature = "clipboard")]
    Case {
        name: "selection-dark",
        fixture: "selection.md",
        width: 640.0,
        scale: 1.0,
        theme: &DARK,
        view: None,
        select: Some(("middle", "three")),
    },
    // Bidi: from the Arabic paragraph through the Hebrew one into the mixed line.
    #[cfg(feature = "clipboard")]
    Case {
        name: "selection-rtl",
        fixture: "rtl.md",
        width: 640.0,
        scale: 1.0,
        theme: &LIGHT,
        view: None,
        select: Some(("لغة", "means")),
    },
    // HiDPI 2× at 150% zoom: the rectangles snap to whole pixels.
    #[cfg(feature = "clipboard")]
    Case {
        name: "selection-2x",
        fixture: "selection.md",
        width: 400.0,
        scale: 3.0,
        theme: &LIGHT,
        view: None,
        select: Some(("inline", "wrap onto")),
    },
];

/// What the pointer is doing at a code block's copy button.
#[cfg(feature = "clipboard")]
#[derive(Clone, Copy)]
enum Look {
    /// Over the block, off the button: it shows, not lit.
    Rest,
    /// On the button.
    Hover,
    /// On the button just after a click: the check mark.
    Checked,
}

/// A copy button, painted by the viewer itself with the pointer where `look` says (the button
/// shows only over a code block, so these go through [`View`], not the painter alone).
#[cfg(feature = "clipboard")]
struct ButtonCase {
    name: &'static str,
    /// Logical width of the window, and the window system's scale.
    width: f32,
    scale: f32,
    dark: bool,
    look: Look,
}

#[cfg(feature = "clipboard")]
const BUTTON_CASES: &[ButtonCase] = &[
    ButtonCase {
        name: "copybtn-rest-light",
        width: 520.0,
        scale: 1.0,
        dark: false,
        look: Look::Rest,
    },
    ButtonCase {
        name: "copybtn-hover-light",
        width: 520.0,
        scale: 1.0,
        dark: false,
        look: Look::Hover,
    },
    ButtonCase {
        name: "copybtn-hover-dark",
        width: 520.0,
        scale: 1.0,
        dark: true,
        look: Look::Hover,
    },
    ButtonCase {
        name: "copybtn-checked-light",
        width: 520.0,
        scale: 1.0,
        dark: false,
        look: Look::Checked,
    },
    ButtonCase {
        name: "copybtn-checked-dark",
        width: 520.0,
        scale: 1.0,
        dark: true,
        look: Look::Checked,
    },
    // HiDPI 2×: the button, its border and its icon scale with the window.
    ButtonCase {
        name: "copybtn-hover-2x",
        width: 400.0,
        scale: 2.0,
        dark: false,
        look: Look::Hover,
    },
];

/// An image: width, height and pixels.
type Rendered = (usize, usize, Vec<u32>);

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The case painted: `(width, height, pixels)`. A whole-document case is cut off at [`MAX_H`].
fn render_case(case: &Case, fonts: &Fonts) -> Rendered {
    let src = std::fs::read_to_string(root().join("fixtures").join(case.fixture)).unwrap();
    let doc = Doc::parse(&src);
    let width = (case.width * case.scale).round();
    let mut layout = Layout::new(
        &doc,
        Params {
            width,
            scale: case.scale,
            ..Params::default()
        },
    );
    // The images of a selection case are shown, at a size of their own.
    if case.select.is_some() {
        for id in 1..=doc.images.len() as u32 {
            layout.set_image(&doc, id, ImageSize::Pixels(120, 60));
        }
    }
    while !layout.idle(&doc, fonts, Instant::now() + Duration::from_secs(1)) {}
    let total = layout.total_height();
    let h = match case.view {
        Some(view) => (view * case.scale).round() as usize,
        None => (total.ceil() as usize).clamp(1, MAX_H),
    };
    // The block at the middle of the scroll range goes at the top of the window; everything is
    // measured, so its y is the scroll offset.
    let scroll = match case.view {
        Some(_) => {
            let anchor = layout.anchor_at((total - h as f32) / 2.0);
            layout.ensure(&doc, fonts, anchor, h as f32)
        }
        None => {
            layout.ensure(&doc, fonts, Anchor::TOP, h as f32);
            0.0
        }
    };
    let w = width as usize;
    let mut px = vec![0; w * h];
    let vp = Viewport {
        area: Clip {
            x0: 0,
            y0: 0,
            x1: w as i32,
            y1: h as i32,
        },
        scroll: f64::from(scroll),
    };
    let mut frame = Frame::new(&mut px, w, h, w);
    #[cfg(feature = "clipboard")]
    let sel = case.select.map(|(from, to)| {
        // The caret before the first character of `from`, after the last of `to`.
        (pos_of(&doc, from, false), pos_of(&doc, to, true))
    });
    #[cfg(not(feature = "clipboard"))]
    assert!(case.select.is_none());
    render::paint(
        &mut frame,
        vp,
        &layout,
        fonts,
        &mut GlyphCache::default(),
        case.theme,
        #[cfg(feature = "clipboard")]
        sel,
    );
    if case.select.is_some() {
        render::paint_images(
            &mut frame,
            vp,
            &layout,
            &mut Gradient(None),
            case.theme,
            #[cfg(feature = "clipboard")]
            sel,
        );
    }
    if case.view.is_some() {
        assert!(total > h as f32, "{}: the window must scroll", case.name);
        render::paint_scrollbar(&mut frame, vp, total, case.scale, case.theme);
    }
    (w, h, px)
}

/// The button case painted: `(width, height, pixels)`, the whole of `codecopy.md`. The pointer
/// is over its first code block, and the button is the first block's.
#[cfg(feature = "clipboard")]
fn render_button(case: &ButtonCase) -> Rendered {
    let src = std::fs::read_to_string(root().join("fixtures/codecopy.md")).unwrap();
    let doc = Doc::parse(&src);
    let w = (case.width * case.scale).round() as u32;
    let mut view = View::new(
        Fonts::new().unwrap(),
        doc,
        None,
        (w, MAX_H as u32),
        f64::from(case.scale),
    );
    view.set_theme_mode(if case.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    });
    while !view.idle(Instant::now() + Duration::from_secs(1)) {}
    view.ensure();
    let h = (view.layout().total_height().ceil() as u32).clamp(1, MAX_H as u32);
    view.resize(w, h);
    view.ensure();
    let first = view
        .doc()
        .blocks
        .iter()
        .position(|b| b.kind == vomd::doc::BlockKind::Code)
        .expect("the fixture has a code block");
    // The middle of the block's background, and the button at its corner.
    let content = view.layout().geoms()[first].content.as_deref().unwrap();
    let r = content
        .rects
        .iter()
        .find(|r| r.role == vomd::theme::Role::CodeBg)
        .unwrap();
    let inside = (
        f64::from(view.layout().column().0 + r.x + r.w / 2.0),
        view.layout().top(first) + f64::from(r.y + r.h / 2.0),
    );
    let button = view
        .copy_button_at(inside.0, inside.1)
        .expect("the block has room for a button");
    let on = (
        f64::from(button.x0 + button.x1) / 2.0,
        f64::from(button.y0 + button.y1) / 2.0,
    );
    let now = Instant::now();
    match case.look {
        Look::Rest => {
            view.hover_at(inside.0, inside.1, now);
        }
        Look::Hover => {
            view.hover_at(on.0, on.1, now);
        }
        Look::Checked => {
            view.hover_at(on.0, on.1, now);
            view.code_copied(first as u32, now);
        }
    }
    let mut px = vec![0; w as usize * h as usize];
    view.paint(&mut px, w as usize, now + Duration::from_millis(100));
    (w as usize, h as usize, px)
}

/// The caret before (`end` false) or after the first `needle` of the document's text.
#[cfg(feature = "clipboard")]
fn pos_of(doc: &Doc, needle: &str, end: bool) -> Pos {
    let start = doc.text.find(needle).expect("the needle is in the text");
    let off = start + if end { needle.len() } else { 0 };
    // The block holding the character the caret is next to.
    let at = if end { off - 1 } else { off } as u32;
    let holds = |spans: &[vomd::doc::Span]| spans.iter().any(|s| s.start <= at && at < s.end);
    let block = doc
        .blocks
        .iter()
        .position(|b| holds(doc.block_spans(b)) || doc.row_cells(b).any(|(_, s)| holds(s)))
        .expect("a block holds it");
    Pos::new(block as u32, off as u32)
}

/// A bitmap that shades from blue to orange, for the images that are shown.
struct Gradient(Option<Bitmap>);

impl Pixels for Gradient {
    fn pixels(&mut self, _: u32, w: u32, h: u32) -> Option<&Bitmap> {
        let data = (0..w * h)
            .map(|i| {
                let t = (i % w) * 255 / w.max(1);
                0xff00_0000 | (255 - t) << 16 | (t / 2 + 90) << 8 | t
            })
            .collect();
        self.0 = Some(Bitmap {
            width: w,
            height: h,
            data,
        });
        self.0.as_ref()
    }
}

fn to_rgb(px: &[u32]) -> Vec<u8> {
    px.iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
        .collect()
}

fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(rgb).unwrap();
}

fn read_png(path: &Path) -> Option<(usize, usize, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}: golden must be 8-bit RGB",
        path.display()
    );
    buf.truncate(info.buffer_size());
    Some((info.width as usize, info.height as usize, buf))
}

/// Pixels with some channel differing by more than [`TOLERANCE`], and a diff image (bad pixels
/// red over a faded copy of the expected image).
fn compare(expected: &[u8], actual: &[u8]) -> (usize, Vec<u8>) {
    let mut bad = 0;
    let mut diff = Vec::with_capacity(expected.len());
    for (e, a) in expected
        .as_chunks::<3>()
        .0
        .iter()
        .zip(actual.as_chunks::<3>().0)
    {
        if e.iter().zip(a).any(|(e, a)| e.abs_diff(*a) > TOLERANCE) {
            bad += 1;
            diff.extend_from_slice(&[255, 0, 0]);
        } else {
            diff.extend(e.iter().map(|c| 192 + c / 4));
        }
    }
    (bad, diff)
}

#[test]
fn goldens() {
    let fonts = Fonts::new().unwrap();
    let bless = std::env::var_os("VOMD_BLESS").is_some_and(|v| v == "1");
    let out_dir = root().join("target/golden-out");
    let mut failures = Vec::new();
    let cases = CASES
        .iter()
        .map(|case| (case.name, render_case(case, &fonts)));
    #[cfg(feature = "clipboard")]
    let cases = cases.chain(
        BUTTON_CASES
            .iter()
            .map(|case| (case.name, render_button(case))),
    );
    for (name, (w, h, px)) in cases {
        let rgb = to_rgb(&px);
        let golden: PathBuf = root().join("tests/golden").join(format!("{name}.png"));
        if bless {
            write_png(&golden, w, h, &rgb);
            println!("blessed {} ({w}×{h})", golden.display());
            continue;
        }
        let actual_path = out_dir.join(format!("{name}.png"));
        let Some((ew, eh, expected)) = read_png(&golden) else {
            write_png(&actual_path, w, h, &rgb);
            failures.push(format!("{name}: no golden (run `cargo xtask bless`)"));
            continue;
        };
        if (ew, eh) != (w, h) {
            write_png(&actual_path, w, h, &rgb);
            failures.push(format!("{name}: size {w}×{h}, golden {ew}×{eh}"));
            continue;
        }
        let (bad, diff) = compare(&expected, &rgb);
        let share = bad as f64 / (w * h) as f64;
        if share > MAX_BAD {
            write_png(&actual_path, w, h, &rgb);
            write_png(&out_dir.join(format!("{name}-diff.png")), w, h, &diff);
            failures.push(format!(
                "{name}: {bad} pixels ({:.3}%) differ; see {}",
                share * 100.0,
                out_dir.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}

/// The comparison itself: within tolerance passes, a real change fails.
#[test]
fn tolerance_rules() {
    let a = vec![100u8; 3 * 1000];
    let mut b = a.clone();
    b[0] = 108; // exactly at the tolerance
    assert_eq!(compare(&a, &b).0, 0);
    for p in 0..6 {
        b[p * 3 + 1] = 109;
    }
    let bad = compare(&a, &b).0;
    assert_eq!(bad, 6);
    assert!(
        bad as f64 / 1000.0 > MAX_BAD,
        "six bad pixels in a thousand fail"
    );
}
