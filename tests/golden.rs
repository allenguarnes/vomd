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
use vomd::layout::{Anchor, Layout, Params};
use vomd::render::{self, Clip, Frame, Viewport};
use vomd::text::cache::GlyphCache;
use vomd::theme::{DARK, LIGHT, Theme};

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
}

const CASES: &[Case] = &[
    Case {
        name: "golden-light",
        fixture: "golden.md",
        width: 760.0,
        scale: 1.0,
        theme: &LIGHT,
    },
    Case {
        name: "golden-dark",
        fixture: "golden.md",
        width: 760.0,
        scale: 1.0,
        theme: &DARK,
    },
    Case {
        name: "golden-light-2x",
        fixture: "golden.md",
        width: 760.0,
        scale: 2.0,
        theme: &LIGHT,
    },
    Case {
        name: "golden-narrow",
        fixture: "golden.md",
        width: 360.0,
        scale: 1.0,
        theme: &LIGHT,
    },
    Case {
        name: "tables-dark",
        fixture: "tables.md",
        width: 900.0,
        scale: 1.0,
        theme: &DARK,
    },
    Case {
        name: "tiny",
        fixture: "tiny.md",
        width: 640.0,
        scale: 1.0,
        theme: &LIGHT,
    },
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The whole document (up to [`MAX_H`]) painted at the case's size: `(width, height, pixels)`.
fn render_case(case: &Case, fonts: &Fonts) -> (usize, usize, Vec<u32>) {
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
    while !layout.idle(&doc, fonts, Instant::now() + Duration::from_secs(1)) {}
    let h = (layout.total_height().ceil() as usize).clamp(1, MAX_H);
    layout.ensure(&doc, fonts, Anchor::TOP, h as f32);
    let w = width as usize;
    let mut px = vec![0; w * h];
    let vp = Viewport {
        area: Clip {
            x0: 0,
            y0: 0,
            x1: w as i32,
            y1: h as i32,
        },
        scroll: 0.0,
    };
    let mut frame = Frame::new(&mut px, w, h, w);
    render::paint(
        &mut frame,
        vp,
        &layout,
        fonts,
        &mut GlyphCache::default(),
        case.theme,
    );
    (w, h, px)
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
    for (e, a) in expected.chunks_exact(3).zip(actual.chunks_exact(3)) {
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
    for case in CASES {
        let (w, h, px) = render_case(case, &fonts);
        let rgb = to_rgb(&px);
        let golden: PathBuf = root()
            .join("tests/golden")
            .join(format!("{}.png", case.name));
        if bless {
            write_png(&golden, w, h, &rgb);
            println!("blessed {} ({w}×{h})", golden.display());
            continue;
        }
        let actual_path = out_dir.join(format!("{}.png", case.name));
        let Some((ew, eh, expected)) = read_png(&golden) else {
            write_png(&actual_path, w, h, &rgb);
            failures.push(format!(
                "{}: no golden (run `cargo xtask bless`)",
                case.name
            ));
            continue;
        };
        if (ew, eh) != (w, h) {
            write_png(&actual_path, w, h, &rgb);
            failures.push(format!("{}: size {w}×{h}, golden {ew}×{eh}", case.name));
            continue;
        }
        let (bad, diff) = compare(&expected, &rgb);
        let share = bad as f64 / (w * h) as f64;
        if share > MAX_BAD {
            write_png(&actual_path, w, h, &rgb);
            write_png(
                &out_dir.join(format!("{}-diff.png", case.name)),
                w,
                h,
                &diff,
            );
            failures.push(format!(
                "{}: {bad} pixels ({:.3}%) differ; see {}",
                case.name,
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
