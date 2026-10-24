use super::super::{
    CACHE_BYTES, Format, Limits, NO_CANCEL, decode, decode_cost, header_of, info_of,
};
use super::*;
use std::time::{Duration, Instant};

/// How long what is refused or stopped at once may take: far under what it would take if it
/// weren't (minutes, or never), far over what a loaded machine makes of it. The tests count what
/// was refused, not how fast.
const SANE: Duration = Duration::from_secs(10);

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

/// The SVG fixtures, with the size usvg gives each.
const FIXTURES: [&str; 8] = [
    "badge.svg",
    "badge-arial.svg",
    "logo-viewbox.svg",
    "percent.svg",
    "units.svg",
    "one-side.svg",
    "nested-logo.svg",
    "external.svg",
];

/// usvg's tree of `bytes`, parsed as the decode does.
fn tree_of(bytes: &[u8]) -> usvg::Tree {
    let h = info_of(bytes).unwrap();
    let mut buf = Vec::new();
    let xml = xml_of(bytes, &h.svg, &mut buf, MAX_SVG_XML).unwrap();
    let budget = Budget::new(usize::MAX, 0, Caps::NORMAL);
    parse(xml, &budget, 0, &NO_CANCEL, None).unwrap()
}

/// Red, green, blue and alpha of a premultiplied `0xAARRGGBB`.
fn rgba(p: u32) -> [u32; 4] {
    [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF, p >> 24]
}

#[test]
fn the_header_size_is_usvgs() {
    for name in FIXTURES.iter().copied().chain(["badge.svgz"]) {
        let bytes = fixture(name);
        let (format, w, h) = header_of(&bytes).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(format, Format::Svg, "{name}");
        let size = tree_of(&bytes).size();
        assert_eq!(
            (w, h),
            (size.width().round() as u32, size.height().round() as u32),
            "{name}"
        );
    }
    // usvg's rules: the viewBox's size; a percentage of it; units at 96 dpi; the viewBox's
    // other side when one is missing (not its aspect).
    let size = |name| header_of(&fixture(name)).map(|(_, w, h)| (w, h));
    assert_eq!(size("logo-viewbox.svg"), Some((48, 32)));
    assert_eq!(size("percent.svg"), Some((40, 15)));
    assert_eq!(size("units.svg"), Some((38, 32)));
    assert_eq!(size("one-side.svg"), Some((64, 16)));
    assert_eq!(size("badge.svgz"), Some((90, 20)));
}

#[test]
fn headers_of_odd_and_broken_svgs() {
    let size = |s: &str| header_of(s.as_bytes()).map(|(_, w, h)| (w, h));
    // What may come before the root tag.
    assert_eq!(
        size(
            "\u{FEFF}  <?xml version=\"1.0\"?>\n<!-- a > b --><?pi x?><!DOCTYPE svg [ <!ENTITY a \"]>\"> ]>\n<svg width='3' height='4'/>"
        ),
        Some((3, 4))
    );
    assert_eq!(size("<svg>"), Some((100, 100)));
    assert_eq!(size("<svg width=\"50%\">"), Some((50, 100)));
    assert_eq!(size("<svg width=\"2em\" height=\"1ex\">"), Some((32, 8)));
    assert_eq!(
        size("<svg width=\"1.5e1\" height=\"3in\">"),
        Some((15, 288))
    );
    // Broken values (`NaN` too) are 100%; usvg has no size for zero, negative or infinite ones.
    assert_eq!(size("<svg width=\"abc\" height=\"2\">"), Some((100, 2)));
    assert_eq!(size("<svg width=\"12vw\" height=\"2\">"), Some((100, 2)));
    assert_eq!(size("<svg width=\"NaN\" height=\"2\">"), Some((100, 2)));
    for bad in ["0", "-5", "1e99"] {
        assert_eq!(
            size(&format!("<svg width=\"{bad}\" height=\"2\">")),
            None,
            "{bad}"
        );
    }
    // Huge: scaled down, the aspect kept.
    assert_eq!(
        size("<svg width=\"1e9\" height=\"5e8\">"),
        Some((16_384, 8192))
    );
    assert_eq!(size("<svg width=\"1e9\" height=\"1\">"), Some((16_384, 1)));
    // Not SVGs.
    for not in [
        "<html><svg>",
        "<svgx>",
        "<!-- unclosed",
        "",
        "<?xml?>",
        "<svg width='1",
    ] {
        assert_eq!(size(not), None, "{not}");
    }
    // The root tag must start within 4 KB.
    let late = format!("<!--{}--><svg>", " ".repeat(5000));
    assert_eq!(size(&late), None);
    // UTF-16 isn't read.
    let utf16: Vec<u8> = "<svg>".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(header_of(&[&[0xFF, 0xFE][..], &utf16].concat()), None);
    // An svgz cut short; saying more than 4 MiB of XML, over VOMD's limits (a click loads it),
    // or more than 64 MiB, which isn't loaded.
    let z = fixture("badge.svgz");
    assert_eq!(header_of(&z[..z.len() / 2]).map(|h| h.0), None);
    let mut big = z.clone();
    let n = big.len();
    big[n - 4..].copy_from_slice(&(5u32 << 20).to_le_bytes());
    assert!(header_of(&big).is_some());
    assert_eq!(
        super::super::refusal_of(&big),
        Some(crate::layout::Refusal::Bytes(5 << 20))
    );
    big[n - 4..].copy_from_slice(&(65u32 << 20).to_le_bytes());
    assert_eq!(header_of(&big), None);
}

#[test]
fn svgs_are_drawn_at_the_box() {
    let logo = fixture("logo-viewbox.svg");
    // Red on the left, blue on the right, a black line of one pixel along the top.
    for scale in [1, 2, 3] {
        let (w, h) = (48 * scale, 32 * scale);
        let b = decode(&logo, w, h).unwrap();
        assert_eq!((b.width, b.height), (w, h));
        let px = |x: u32, y: u32| rgba(b.data[(y * w + x) as usize]);
        for y in 0..scale {
            assert_eq!(px(5, y), [0, 0, 0, 255], "line at {scale}×, row {y}");
        }
        // Crisp: the row under the line is red, not a blend.
        assert_eq!(px(5, scale), [0xEE, 0, 0, 255], "{scale}×");
        assert_eq!(px(w - 5, h - 1), [0, 0, 0xEE, 255], "{scale}×");
    }
    // Another box than the size: fitted uniformly and centered.
    let b = decode(&logo, 96, 32).unwrap();
    assert_eq!(rgba(b.data[96 * 10 + 10]), [0, 0, 0, 0]);
    assert_eq!(rgba(b.data[96 * 10 + 30]), [0xEE, 0, 0, 255]);
    // An svgz, and the SVG nested in a `data:` URL.
    assert!(decode(&fixture("badge.svgz"), 90, 20).is_some());
    let nested = decode(&fixture("nested-logo.svg"), 60, 20).unwrap();
    assert_eq!(rgba(nested.data[60 * 10 + 5]), [0xEE, 0, 0, 255]);
    assert_eq!(rgba(nested.data[60 * 10 + 20]), [0, 0, 0xEE, 255]);
    assert_eq!(rgba(nested.data[60 * 10 + 40]), [0x55, 0x55, 0x55, 255]);
}

#[test]
fn nothing_outside_the_svg_is_read() {
    let before = RESOLVED_STRINGS.load(std::sync::atomic::Ordering::Relaxed);
    let tree = tree_of(&fixture("external.svg"));
    let after = RESOLVED_STRINGS.load(std::sync::atomic::Ordering::Relaxed);
    // The file, the relative path, the URL and the image beside it: each asked for, and refused.
    assert!(after - before >= 4, "{}", after - before);
    fn images(g: &usvg::Group) -> usize {
        g.children()
            .iter()
            .map(|n| match n {
                usvg::Node::Image(_) => 1,
                usvg::Node::Group(g) => images(g),
                _ => 0,
            })
            .sum()
    }
    assert_eq!(images(tree.root()), 0);
    let b = decode(&fixture("external.svg"), 40, 20).unwrap();
    assert!(
        b.data.iter().all(|&p| p == 0xFF00_CC00),
        "{:08x}",
        b.data[0]
    );
}

/// `decode` of `svg` at `w` × `h`, which must not take long ([`SANE`]).
fn quick(svg: &[u8], w: u32, h: u32) -> Option<Bitmap> {
    let start = Instant::now();
    let b = decode(svg, w, h);
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    b
}

#[test]
fn pathological_svgs_are_refused_quickly() {
    let svg = |body: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"64\" height=\"64\">{body}</svg>"
        )
    };
    // `<use>` doubling 25 times.
    let mut uses = String::from("<defs><rect id=\"u0\" width=\"1\" height=\"1\"/>");
    for i in 1..=25 {
        uses += &format!(
            "<g id=\"u{i}\"><use xlink:href=\"#u{0}\"/><use xlink:href=\"#u{0}\"/></g>",
            i - 1
        );
    }
    uses += "</defs><use xlink:href=\"#u25\"/>";
    assert!(quick(svg(&uses).as_bytes(), 64, 64).is_none());
    // Billion laughs, and one big entity referenced many times.
    let mut laughs = String::from("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY l0 \"lol\">");
    for i in 1..10 {
        let refs = format!("&l{};", i - 1).repeat(10);
        laughs += &format!("<!ENTITY l{i} \"{refs}\">");
    }
    laughs += "]>";
    laughs += &svg("<text>&l9;</text>");
    assert_eq!(scan(laughs.as_bytes()), Err(Refusal::Complex));
    let h = info_of(laughs.as_bytes()).unwrap();
    let fonts = &mut Fonts::default();
    let refused = render(
        laughs.as_bytes(),
        &h.svg,
        (64, 64),
        CACHE_BYTES,
        &NO_CANCEL,
        fonts,
    );
    assert!(matches!(refused, Rendered::Refused(Refusal::Complex)));
    assert!(quick(laughs.as_bytes(), 64, 64).is_none());
    let mut quadratic = format!("<!DOCTYPE svg [<!ENTITY big \"{}\">]>", "x".repeat(200_000));
    quadratic += &svg(&"<text>&big;</text>".repeat(50));
    assert_eq!(scan(quadratic.as_bytes()), Err(Refusal::Complex));
    assert!(quick(quadratic.as_bytes(), 64, 64).is_none());
    // A thousand entities, each nested ten times over: counted at once.
    let mut nested = String::from("<!DOCTYPE svg [<!ENTITY e0 \"x\">");
    for i in 1..1000 {
        nested += &format!("<!ENTITY e{i} \"{}\">", format!("&e{};", i - 1).repeat(10));
    }
    nested += "]>";
    nested += &svg("<text>&e999;</text>");
    let start = Instant::now();
    assert_eq!(scan(nested.as_bytes()), Err(Refusal::Complex));
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    // An Illustrator file's few entities are fine.
    let ai = format!(
        "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\" [<!ENTITY ns_svg \"http://www.w3.org/2000/svg\"><!ENTITY st0 \"fill:#e00;\">]>{}",
        "<svg xmlns=\"&ns_svg;\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\" style=\"&st0;\"/></svg>"
    );
    let b = quick(ai.as_bytes(), 8, 8).unwrap();
    assert_eq!(rgba(b.data[0]), [0xEE, 0, 0, 255]);
    // 10,000 deep.
    let deep = svg(&format!(
        "{}{}",
        "<g>".repeat(10_000),
        "</g>".repeat(10_000)
    ));
    assert_eq!(scan(deep.as_bytes()), Err(Refusal::Complex));
    assert!(quick(deep.as_bytes(), 64, 64).is_none());
    // 256 deep is fine.
    let ok = svg(&format!(
        "{}<rect width=\"9\" height=\"9\"/>{}",
        "<g>".repeat(250),
        "</g>".repeat(250)
    ));
    assert!(quick(ok.as_bytes(), 64, 64).is_some());
    // More than 100,000 elements.
    let many = svg(&"<g/>".repeat(100_001));
    assert!(quick(many.as_bytes(), 64, 64).is_none());
}

#[test]
fn a_job_charged_too_little_says_what_it_needs() {
    // An element-dense SVG of 1 MB: more than the first charge's guess.
    let body =
        "<g transform=\"translate(1 1)\"><rect width=\"1\" height=\"1\" style=\"fill:#e00\"/></g>"
            .repeat(12_000);
    let dense = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\">{body}</svg>"
    );
    let h = info_of(dense.as_bytes()).unwrap();
    assert_eq!(h.svg.parse, 0);
    let charge = |c: &SvgCost| decode_cost(&Header { svg: *c, ..h }, 64, 64);
    let need = match render(
        dense.as_bytes(),
        &h.svg,
        (64, 64),
        charge(&h.svg),
        &NO_CANCEL,
        &mut Fonts::default(),
    ) {
        Rendered::Need(n) => n,
        _ => panic!("rendered within the guess"),
    };
    assert!(need.parse > guess_parse(dense.len(), false));
    // Charged what it said (and what the next said: the drawing is measured once it's parsed),
    // it renders.
    let mut need = need;
    for _ in 0..3 {
        match render(
            dense.as_bytes(),
            &need,
            (64, 64),
            charge(&need),
            &NO_CANCEL,
            &mut Fonts::default(),
        ) {
            Rendered::Pixels(b, _) => {
                assert_eq!(rgba(b.data[64 + 1]), [0xEE, 0, 0, 255]);
                return;
            }
            Rendered::Need(n) => need = n,
            _ => panic!("refused"),
        }
    }
    panic!("not rendered with what it needed");
}

#[test]
fn large_svgs_load_by_what_they_hold() {
    // Element-dense, 4 MiB: over the budget whatever it is charged.
    let rect =
        "<g transform=\"translate(1 1)\"><rect width=\"1\" height=\"1\" style=\"fill:#e00\"/></g>";
    let n = (4 << 20) / rect.len() - 1;
    let dense = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\">{}</svg>",
        rect.repeat(n.min(99_000 / 2))
    );
    let s = scan(dense.as_bytes()).unwrap();
    assert!(
        parse_cost(&s, 0, 0) > CACHE_BYTES,
        "{}",
        parse_cost(&s, 0, 0)
    );
    assert!(decode(dense.as_bytes(), 64, 64).is_none());
    // Path-dense, 3 MiB, in paths of 1,000 shapes: in it. (One path of all of them is far
    // dearer to draw: its edges are all made at once.)
    let mut paths = String::new();
    let mut d = String::new();
    for i in 0.. {
        if paths.len() >= 3 << 20 {
            break;
        }
        d += &format!("M{} {}h1v1h-1z", i % 1000 * 2, i / 1000 * 2);
        if i % 1000 == 999 {
            paths += &format!("<path d=\"{d}\"/>");
            d.clear();
        }
    }
    let paths = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\" viewBox=\"0 0 2000 2000\">{paths}</svg>"
    );
    let s = scan(paths.as_bytes()).unwrap();
    assert!(
        parse_cost(&s, 0, 0) < CACHE_BYTES * 3 / 4,
        "{}",
        parse_cost(&s, 0, 0)
    );
    let limits = Limits::normal();
    assert!(super::super::decode_with(paths.as_bytes(), 64, 64, &limits, &NO_CANCEL).is_some());
}

/// VOMD's faces, as the window gives them.
fn vomd_fonts() -> Fonts {
    let mut fonts = Fonts::default();
    let sources = crate::fonts::Fonts::new().unwrap().svg_sources();
    fonts.set(Some(Arc::new(sources)));
    fonts
}

/// `svg` drawn at its size with VOMD's faces.
fn with_fonts(svg: &[u8], fonts: &mut Fonts) -> Bitmap {
    let h = info_of(svg).unwrap();
    match render(svg, &h.svg, (h.w, h.h), CACHE_BYTES, &NO_CANCEL, fonts) {
        Rendered::Pixels(b, _) => b,
        _ => panic!("not drawn"),
    }
}

/// How many pixels of `b` are near white (the badges' text).
fn whitish(b: &Bitmap) -> usize {
    b.data
        .iter()
        .filter(|&&p| rgba(p).iter().take(3).all(|&c| c > 200))
        .count()
}

#[test]
fn text_is_set_in_vomds_faces() {
    let mut fonts = vomd_fonts();
    // A badge naming Verdana, and one naming only Arial: both drawn, in the same face.
    let badge = with_fonts(&fixture("badge.svg"), &mut fonts);
    let arial = with_fonts(&fixture("badge-arial.svg"), &mut fonts);
    assert!(whitish(&badge) > 40, "{}", whitish(&badge));
    assert_eq!(badge.data, arial.data);
    // Without fonts, no text.
    let h = info_of(&fixture("badge.svg")).unwrap();
    let Rendered::Pixels(bare, _) = render(
        &fixture("badge.svg"),
        &h.svg,
        (90, 20),
        CACHE_BYTES,
        &NO_CANCEL,
        &mut Fonts::default(),
    ) else {
        panic!();
    };
    assert_eq!(whitish(&bare), 0);
    // A monospace-looking name gets code's face, which sets the text differently.
    let courier = String::from_utf8(fixture("badge-arial.svg"))
        .unwrap()
        .replace("Arial", "Courier New")
        .replace(" textLength=\"270\"", "")
        .replace(" textLength=\"430\"", "");
    let sans = courier.replace("Courier New", "Arial");
    assert_ne!(
        with_fonts(courier.as_bytes(), &mut fonts).data,
        with_fonts(sans.as_bytes(), &mut fonts).data
    );
}

#[test]
fn the_font_database_is_made_once_and_only_for_text() {
    use std::sync::atomic::Ordering::Relaxed;
    let mut fonts = vomd_fonts();
    let before = BUILT.load(Relaxed);
    with_fonts(&fixture("logo-viewbox.svg"), &mut fonts);
    assert!(fonts.built.is_none(), "made for an SVG without text");
    with_fonts(&fixture("badge.svg"), &mut fonts);
    with_fonts(&fixture("badge-arial.svg"), &mut fonts);
    assert!(fonts.built.is_some());
    // (Other tests make theirs meanwhile: at least one, and this one's once.)
    assert!(BUILT.load(Relaxed) > before);
    let built = fonts.built.as_ref().map(|b| b.1.clone()).unwrap();
    with_fonts(&fixture("badge.svg"), &mut fonts);
    assert!(Arc::ptr_eq(&fonts.built.as_ref().unwrap().1, &built));
    // Other faces: made again.
    fonts.set(Some(Arc::new(
        crate::fonts::Fonts::new().unwrap().svg_sources(),
    )));
    with_fonts(&fixture("badge.svg"), &mut fonts);
    assert!(!Arc::ptr_eq(&fonts.built.as_ref().unwrap().1, &built));
}

#[test]
fn raster_images_inside_are_drawn_at_their_size() {
    let svg = fixture("raster-inside.svg");
    let h = info_of(&svg).unwrap();
    let draw = |w, h2| {
        let charge = CACHE_BYTES;
        match render(
            &svg,
            &h.svg,
            (w, h2),
            charge,
            &NO_CANCEL,
            &mut Fonts::default(),
        ) {
            Rendered::Pixels(b, skipped) => (b, skipped),
            _ => panic!("not drawn"),
        }
    };
    let (b, skipped) = draw(60, 40);
    // The bytes that aren't an image are left out, and that's not for VOMD's limits.
    assert!(!skipped);
    let px = |x: usize, y: usize| rgba(b.data[y * 60 + x]);
    let near = |p: [u32; 4], want: [u32; 4]| p.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 24);
    // The PNG: red, green, blue over white, black, yellow.
    assert!(near(px(3, 3), [255, 0, 0, 255]), "{:?}", px(3, 3));
    assert!(near(px(20, 12), [255, 255, 0, 255]), "{:?}", px(20, 12));
    // At half opacity.
    assert!(near(px(33, 3), [128, 0, 0, 128]), "{:?}", px(33, 3));
    // The JPEG turned upright (orientation 6: a quarter clockwise), 16 × 24 fitted in the
    // 24 × 16 box usvg made of its stored size: the turned image's top left is the stored
    // bottom left, white.
    assert!(near(px(9, 21), [255, 255, 255, 255]), "{:?}", px(9, 21));
    assert_eq!(px(1, 30), [0, 0, 0, 0], "letterboxed");
    // Turned a quarter, under a mask of its top half.
    assert!(
        px(40, 25)[3] > 200 && px(40, 35)[3] == 0,
        "{:?} {:?}",
        px(40, 25),
        px(40, 35)
    );
    // Decoded at the size drawn, and no bigger than the image.
    draw(30, 20);
    let (w, h2) = DECODED_AT.with(std::cell::Cell::get);
    assert!(w <= 12 && h2 <= 12, "{w}×{h2}");
    draw(240, 160);
    assert_eq!(DECODED_AT.with(std::cell::Cell::get), (24, 16));
}

#[test]
fn too_many_raster_images_are_left_out() {
    let png = base64(&fixture("exif-3.png"));
    let image = format!("<image width=\"2\" height=\"2\" href=\"data:image/png;base64,{png}\"/>");
    for (n, skipped) in [(64, false), (65, true)] {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\">{}</svg>",
            image.repeat(n)
        );
        let h = info_of(svg.as_bytes()).unwrap();
        let fonts = &mut Fonts::default();
        match render(
            svg.as_bytes(),
            &h.svg,
            (8, 8),
            CACHE_BYTES,
            &NO_CANCEL,
            fonts,
        ) {
            Rendered::Pixels(_, s) => assert_eq!(s, skipped, "{n}"),
            _ => panic!("not drawn"),
        }
    }
}

/// `data` in base64, for `data:` URLs.
pub(in crate::image) fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[test]
fn a_raster_image_over_the_limits_is_left_out() {
    // A PNG header of 20,000 × 20,000 (over `MAX_PIXELS`), and nothing after it.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(20_000u32.to_be_bytes());
    png.extend(20_000u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\" fill=\"#e00\"/><image width=\"8\" height=\"8\" href=\"data:image/png;base64,{}\"/></svg>",
        base64(&png)
    );
    let h = info_of(svg.as_bytes()).unwrap();
    let fonts = &mut Fonts::default();
    let start = Instant::now();
    match render(
        svg.as_bytes(),
        &h.svg,
        (8, 8),
        CACHE_BYTES,
        &NO_CANCEL,
        fonts,
    ) {
        Rendered::Pixels(b, skipped) => {
            assert!(skipped);
            assert_eq!(rgba(b.data[0]), [0xEE, 0, 0, 255]);
        }
        _ => panic!("not drawn"),
    }
    assert!(start.elapsed() < SANE);
}

/// A 4 × 4 PNG of vertical stripes, black and white, one pixel wide.
fn striped_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 4, 4);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let rows: Vec<u8> = (0..16).map(|i| if i % 2 == 0 { 0 } else { 255 }).collect();
    enc.write_header().unwrap().write_image_data(&rows).unwrap();
    out
}

/// A raster image drawn smaller than it is stored is reduced as its `image-rendering` says:
/// averaged by default, but the pixel under the center of each pixel drawn for `optimizeSpeed`,
/// `crisp-edges` and `pixelated`, as resvg samples an image it is given whole.
#[test]
fn a_reduced_raster_image_is_sampled_as_its_rendering_mode_says() {
    let png = base64(&striped_png());
    let draw = |mode: &str, side: u32| {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image width=\"4\" height=\"4\"{mode} href=\"data:image/png;base64,{png}\"/></svg>"
        );
        let h = info_of(svg.as_bytes()).unwrap();
        match render(
            svg.as_bytes(),
            &h.svg,
            (side, side),
            CACHE_BYTES,
            &NO_CANCEL,
            &mut Fonts::default(),
        ) {
            Rendered::Pixels(b, skipped) => {
                assert!(!skipped, "{mode}");
                b.data
            }
            _ => panic!("not drawn"),
        }
    };
    // The odd columns are white: nearest picks them, averaging mixes them with black.
    for mode in [
        " image-rendering=\"optimizeSpeed\"",
        // (These two are only read from a style, as usvg does.)
        " style=\"image-rendering:crisp-edges\"",
        " style=\"image-rendering:pixelated\"",
    ] {
        assert_eq!(draw(mode, 2), [0xFFFF_FFFF; 4], "{mode}");
    }
    for mode in [
        "",
        " image-rendering=\"optimizeQuality\"",
        " image-rendering=\"smooth\"",
    ] {
        assert_eq!(draw(mode, 2), [0xFF80_8080; 4], "{mode}");
    }
    // Not reduced, the stripes are as stored.
    assert_eq!(
        draw(" image-rendering=\"optimizeSpeed\"", 4)[..4],
        [0xFF00_0000, 0xFFFF_FFFF, 0xFF00_0000, 0xFFFF_FFFF]
    );
}

/// A raster image drawn by nearest neighbor, reduced and placed between pixels, is the pixels
/// resvg picks from the image whole: reducing it first (to 3 × 3, from the left) would keep
/// columns 0, 2 and 3, and not the white one 1 the second pixel drawn is under.
#[test]
fn a_raster_image_placed_between_pixels_is_drawn_from_the_whole_image() {
    let bytes = striped_png();
    let png = base64(&bytes);
    let whole = crate::image::decode(&bytes, 4, 4).unwrap();
    for mode in ["optimizeSpeed", "crisp-edges", "pixelated"] {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image x=\"0.3\" width=\"3\" height=\"3\" style=\"image-rendering:{mode}\" href=\"data:image/png;base64,{png}\"/></svg>"
        );
        let h = info_of(svg.as_bytes()).unwrap();
        let drawn = match render(
            svg.as_bytes(),
            &h.svg,
            (4, 4),
            CACHE_BYTES,
            &NO_CANCEL,
            &mut Fonts::default(),
        ) {
            Rendered::Pixels(b, skipped) => {
                assert!(!skipped, "{mode}");
                b.data
            }
            _ => panic!("not drawn"),
        };
        // The same tree drawn from the image as it is stored.
        let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default()).unwrap();
        let mut want = tiny_skia::Pixmap::new(4, 4).unwrap();
        let raster = |_: &usvg::ImageKind, _: tiny_skia::Transform, _: usvg::ImageRendering| {
            let mut p = tiny_skia::Pixmap::new(whole.width, whole.height).unwrap();
            for (o, c) in p
                .data_mut()
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(&whole.data)
            {
                let [a, r, g, b] = c.to_be_bytes();
                *o = [r, g, b, a];
            }
            Some(p)
        };
        let hooks = resvg::Hooks {
            raster: &raster,
            cancel: &NO_CANCEL,
        };
        resvg::render_with(&tree, fit(&tree, 4, 4), &mut want.as_mut(), &hooks);
        let want: Vec<u32> = want
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[r, g, b, a]| u32::from_be_bytes([a, r, g, b]))
            .collect();
        assert_eq!(drawn, want, "{mode}");
        // Black, then white, inside (the stripes under the centers 1.5 and 2.5).
        assert_eq!(drawn[1..3], [0xFFFF_FFFF, 0xFF00_0000], "{mode}");
    }
}

/// A raster image drawn by nearest neighbor is decoded whole, so it is charged what it holds
/// that way: more than the canvas's, a memory the box can't hold refuses it (and a click loads it).
#[test]
fn a_raster_image_drawn_whole_is_charged_as_the_whole_image() {
    // A PNG of 64 × 64.
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, 64, 64);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&[7; 64 * 64])
        .unwrap();
    let png = base64(&bytes);
    let svg = |mode: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image width=\"4\" height=\"4\"{mode} href=\"data:image/png;base64,{png}\"/></svg>"
        )
    };
    // What it takes to draw on a canvas of 1 × 1: charged what each job says it needs, until
    // drawn.
    let charge = |svg: &str| {
        let mut need = info_of(svg.as_bytes()).unwrap().svg;
        let mut charge = 0;
        loop {
            match render(
                svg.as_bytes(),
                &need,
                (1, 1),
                charge,
                &NO_CANCEL,
                &mut Fonts::default(),
            ) {
                Rendered::Pixels(..) => return charge,
                Rendered::Need(n) => (need, charge) = (n, cost(&n, 1, 1)),
                _ => panic!("not drawn"),
            }
        }
    };
    // Averaged, a 1 × 1 canvas holds a 1 × 1 raster image (3 pixmaps of it); whole, the 64 × 64
    // one, decoded and in the pixmap resvg is given.
    let area = charge(&svg(""));
    let whole = charge(&svg(" image-rendering=\"optimizeSpeed\""));
    assert!(whole >= area + 2 * 64 * 64 * 4 - 100, "{whole} {area}");
}

/// A PNG of `w` × `h` gray pixels all of one value, or, with `columns`, alternating black and
/// white columns, with an EXIF `orientation` before its data.
fn gray_png(w: u32, h: u32, columns: bool, orientation: Option<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    if let Some(o) = orientation {
        // A TIFF of one entry (the orientation, a short), big-endian.
        let mut tiff = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
        tiff.extend([0, o, 0, 0, 0, 0, 0, 0]);
        writer.write_chunk(png::chunk::eXIf, &tiff).unwrap();
    }
    let data: Vec<u8> = (0..w * h)
        .map(|i| {
            if columns && i % 2 == 0 {
                0
            } else if columns {
                255
            } else {
                7
            }
        })
        .collect();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    out
}

/// An SVG of `w` × `h` of the image `png` drawn by nearest neighbor.
fn nearest_svg(png: &[u8], (w, h): (u32, u32), mode: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"><image width=\"{w}\" height=\"{h}\" style=\"image-rendering:{mode}\" href=\"data:image/png;base64,{}\"/></svg>",
        base64(png)
    )
}

/// What the job of `svg` to `size` is charged, from the record `need` (what the SVG is known to
/// need: the one a job of it gives), until it draws; and the record then.
fn drawn_charged(svg: &str, mut need: SvgCost, (w, h): (u32, u32)) -> (SvgCost, usize) {
    for _ in 0..8 {
        let charge = cost(&need, w, h);
        match render(
            svg.as_bytes(),
            &need,
            (w, h),
            charge,
            &NO_CANCEL,
            &mut Fonts::default(),
        ) {
            Rendered::Pixels(..) => return (need, charge),
            Rendered::Need(n) => need = n,
            _ => panic!("not drawn"),
        }
    }
    panic!("still not charged enough");
}

/// What a raster image drawn whole holds is its own, whatever the box: a record made at one box
/// size charges another what a job that had never run charges it, and not the whole image's
/// bytes for each layer of the canvas (a zoom, or a HiDPI window, would be refused for it).
#[test]
fn a_raster_image_drawn_whole_is_charged_the_same_at_any_box() {
    let svg = nearest_svg(&gray_png(1024, 1024, false, None), (100, 100), "pixelated");
    let fresh = |side| drawn_charged(&svg, info_of(svg.as_bytes()).unwrap().svg, (side, side));
    let (small, at_100) = fresh(100);
    let (large, at_200) = fresh(200);
    // Both fit with room, a megabyte of the image in each of three layers being 12 of them.
    assert!(at_100 < CACHE_BYTES / 2 && at_200 < CACHE_BYTES / 2);
    // Zoomed in, and out: the record of the other size.
    let (_, zoomed) = drawn_charged(&svg, small, (200, 200));
    assert_eq!(zoomed, at_200);
    let (_, back) = drawn_charged(&svg, large, (100, 100));
    assert!(back <= at_100 + 4 * 100 * 100, "{back} {at_100}");
    // What a remote one's record says of it when its body is gone: the image is part of what it
    // holds whatever the box, not of what each pixel of the box holds.
    assert!(cost(&large, 0, 0) >= 1024 * 1024 * 4);
    let small_image = nearest_svg(&gray_png(8, 8, false, None), (100, 100), "pixelated");
    let (record, _) = drawn_charged(
        &small_image,
        info_of(small_image.as_bytes()).unwrap().svg,
        (200, 200),
    );
    assert_eq!(box_costs(&large), box_costs(&record));
}

/// `svg` drawn at `size`, and what it covers: the pixels that are more than half opaque.
fn covered(svg: &str, size: (u32, u32)) -> Vec<bool> {
    let header = info_of(svg.as_bytes()).unwrap();
    match render(
        svg.as_bytes(),
        &header.svg,
        size,
        CACHE_BYTES,
        &NO_CANCEL,
        &mut Fonts::default(),
    ) {
        Rendered::Pixels(b, skipped) => {
            assert!(!skipped);
            b.data.iter().map(|p| p >> 24 > 127).collect()
        }
        _ => panic!("not drawn"),
    }
}

/// The columns and rows `drawn` covers, as ranges (the pixels more than half opaque), in a box
/// `width` across.
fn extent(drawn: &[bool], width: usize) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let columns = |x: usize| drawn.chunks(width).any(|row| row[x]);
    let rows = |y: usize| drawn[y * width..][..width].iter().any(|&c| c);
    let range = |n: usize, any: &dyn Fn(usize) -> bool| {
        let first = (0..n).find(|&i| any(i)).unwrap();
        first..(first..n).find(|&i| !any(i)).unwrap_or(n)
    };
    (range(width, &columns), range(drawn.len() / width, &rows))
}

/// A raster image turned by an EXIF orientation that swaps its sides is fitted in the box usvg
/// made for it exactly as big and as centered as it is shown, by area or by nearest neighbor,
/// where the box is drawn at: its fit isn't rounded at the size the image is stored in and then
/// magnified. The bounds are worked out from the box (the turned image's aspect, its half-pixel
/// centering), not from the code's own arithmetic:
/// * 6 × 4 at 12 × 8: turned it is 4 × 6, which fills the 8 rows at 5⅓ columns, from 3⅓ to 8⅔:
///   the pixels from the 4th to the 9th are the ones whose centers it holds.
/// * 2 × 1 at 8 × 4: turned, 1 × 2, which fills the 4 rows at 2 columns, from 3 to 5: an odd
///   padding of 6 pixels, split evenly.
/// * 6 × 4 at 6 × 4: 2⅔ columns from 1⅔ to 4⅓, the 3rd and 4th pixels.
#[test]
fn a_turned_raster_image_is_as_big_and_as_centered_as_it_is_shown() {
    for ((w, h), size, columns) in [
        ((6, 4), (12, 8), 3..9),
        ((2, 1), (8, 4), 3..5),
        ((6, 4), (6, 4), 2..4),
    ] {
        for o in 5..=8 {
            let png = gray_png(w, h, false, Some(o));
            for mode in ["auto", "optimizeSpeed", "crisp-edges", "pixelated"] {
                let drawn = covered(&nearest_svg(&png, size, mode), size);
                let (x, y) = extent(&drawn, size.0 as usize);
                assert_eq!(
                    (x, y),
                    (columns.clone(), 0..size.1 as usize),
                    "{w}×{h} {o} {mode} at {size:?}"
                );
                // Every pixel of that is drawn, and nothing else.
                let count = drawn.iter().filter(|&&c| c).count();
                assert_eq!(count, columns.len() * size.1 as usize, "{w}×{h} {o} {mode}");
            }
        }
    }
}

/// An SVG of one 4 × 4 raster image, the WebP `webp`, and what drawing it gives: the pixel at the
/// corner and whether the SVG is noted as drawn without an image.
fn drawn_with_webp(webp: &[u8]) -> (u32, bool) {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image width=\"4\" height=\"4\" href=\"data:image/webp;base64,{}\"/></svg>",
        base64(webp)
    );
    let h = info_of(svg.as_bytes()).unwrap();
    match render(
        svg.as_bytes(),
        &h.svg,
        (4, 4),
        CACHE_BYTES,
        &NO_CANCEL,
        &mut Fonts::default(),
    ) {
        Rendered::Pixels(b, skipped) => (b.data[0], skipped),
        _ => panic!("not drawn"),
    }
}

#[test]
fn a_raster_image_refused_when_it_is_decoded_is_noted_and_broken_data_is_not() {
    // A valid 4 × 4 WebP whose Huffman codes are over the limit: it passes the header's checks
    // and is refused as it is drawn (a click draws it).
    let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/seeds/image/webp-lossless-many-huffman-groups.bin");
    let groups = std::fs::read(seed).unwrap();
    assert_eq!(drawn_with_webp(&groups), (0, true));
    // A WebP with its pixels cut short is broken: nothing a click would change.
    let good = std::fs::read("fixtures/images/lossless-alpha.webp").unwrap();
    assert!(info_of(&good[..good.len() - 6]).is_some());
    assert_eq!(drawn_with_webp(&good[..good.len() - 6]), (0, false));
    // One that is drawn is not noted.
    assert!(!drawn_with_webp(&good).1);
}

#[test]
fn slow_svgs_are_refused_quickly() {
    let svg = |side: u32, body: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{side}\" height=\"{side}\" viewBox=\"0 0 4 4\">{body}</svg>"
        )
    };
    let filter = |f: &str| {
        format!(
            "<filter id=\"f\" x=\"0\" y=\"0\" width=\"1\" height=\"1\">{f}</filter><rect width=\"4\" height=\"4\" fill=\"#e00\" filter=\"url(#f)\"/>"
        )
    };
    let zigzag = format!("M0 0{}", " L1.5 2.25 L3 0.5".repeat(185_000));
    let short = format!("M0 0{}", " L1.5 2.25 L3 0.5".repeat(30_000));
    let slow = [
        filter("<feTurbulence baseFrequency=\"0.5\" numOctaves=\"1000000\"/>"),
        filter("<feMorphology radius=\"0.8\"/>"),
        filter(&format!("<feConvolveMatrix order=\"200\" kernelMatrix=\"{}\"/>", "1 ".repeat(40_000))),
        "<path d=\"M0 0 L4 4 L0 4 L4 0\" stroke=\"#000\" stroke-width=\"0.01\" stroke-dasharray=\"0.000001\"/>".into(),
        format!("<path d=\"{zigzag}\" fill=\"none\" stroke=\"#000\" stroke-width=\"0.1\"/>"),
        // The same edges, where they are as slow and aren't a path of the image: filled in a clip
        // path (60,000 segments of them), or drawn by an `feImage`.
        format!(
            "<clipPath id=\"c\"><path d=\"{}\"/></clipPath><rect width=\"4\" height=\"4\" fill=\"#e00\" clip-path=\"url(#c)\"/>",
            short
        ),
        format!(
            "<defs><path id=\"z\" d=\"{}\" fill=\"#e00\"/></defs><filter id=\"f\" x=\"0\" y=\"0\" width=\"1\" height=\"1\"><feImage href=\"#z\"/></filter><rect width=\"4\" height=\"4\" filter=\"url(#f)\"/>",
            short
        ),
    ];
    for body in &slow {
        // Morphology of radius 400 pixels on 2,000 × 2,000, and the others at 512 × 512.
        let side = if body.contains("Morphology") {
            2000
        } else {
            512
        };
        let bytes = svg(side, body);
        let h = info_of(bytes.as_bytes()).unwrap();
        let start = Instant::now();
        let fonts = &mut Fonts::default();
        let r = render(
            bytes.as_bytes(),
            &h.svg,
            (side, side),
            usize::MAX,
            &NO_CANCEL,
            fonts,
        );
        assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
        // (Past a million dashes tiny-skia gives up on a path, at once: it may be drawn.)
        let dashes = body.contains("dasharray");
        assert!(
            matches!(r, Rendered::Refused(Refusal::Slow)) || dashes,
            "{}",
            &body[..40]
        );
    }
    // Small, the filters of radius and matrix are drawn (a million octaves are too many for a
    // pixel; the zig-zag's edges all cross whatever the size).
    for body in &slow[1..3] {
        assert!(
            quick(svg(16, body).as_bytes(), 16, 16).is_some(),
            "{}",
            &body[..40]
        );
    }
}

#[test]
fn a_render_stops_when_told_to() {
    // Morphology of radius 400 pixels on 2,000 × 2,000: minutes, were it drawn.
    let body = "<filter id=\"f\" x=\"0\" y=\"0\" width=\"1\" height=\"1\"><feMorphology radius=\"0.8\"/></filter><rect width=\"4\" height=\"4\" fill=\"#e00\" filter=\"url(#f)\"/>";
    let bytes = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2000\" height=\"2000\" viewBox=\"0 0 4 4\">{body}</svg>"
    );
    let tree = tree_of(bytes.as_bytes());
    let cancel = Arc::new(AtomicBool::new(false));
    let theirs = cancel.clone();
    let start = Instant::now();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        theirs.store(true, std::sync::atomic::Ordering::Relaxed);
        Instant::now()
    });
    let mut pixmap = tiny_skia::Pixmap::new(2000, 2000).unwrap();
    let hooks = resvg::Hooks {
        raster: &|_, _, _| None,
        cancel: &cancel,
    };
    resvg::render_with(&tree, fit(&tree, 2000, 2000), &mut pixmap.as_mut(), &hooks);
    let stopped = Instant::now();
    let set = setter.join().unwrap();
    // (Within a pixel of the filter, but for what can't stop: the layer's colors turned to
    // linear RGB and back, tens of ms for 4 MP, more on a loaded machine.)
    assert!(
        stopped.duration_since(set) < SANE,
        "{:?}",
        stopped.duration_since(set)
    );
    assert!(start.elapsed() < SANE);
}

/// More embedded raster images than VOMD draws: the SVG is drawn without the rest, and says so;
/// loaded past the limits, it is drawn with all of them.
#[test]
fn rasters_over_the_cap_are_drawn_when_forced() {
    let png = base64(&fixture("exif-3.png"));
    let images: String = (0..MAX_RASTERS + 1)
        .map(|i| {
            format!(
                "<image x=\"{i}\" width=\"1\" height=\"1\" href=\"data:image/png;base64,{png}\"/>"
            )
        })
        .collect();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"4\">{images}</svg>"
    );
    let h = info_of(svg.as_bytes()).unwrap();
    let fonts = &mut Fonts::default();
    let skipped = |caps: &Caps, fonts: &mut Fonts| match render_in(
        svg.as_bytes(),
        &h.svg,
        (80, 4),
        64 << 20,
        caps,
        &NO_CANCEL,
        fonts,
    ) {
        Rendered::Pixels(_, skipped) => skipped,
        _ => panic!("not drawn"),
    };
    assert!(skipped(&Caps::NORMAL, fonts));
    let forced = Caps::of(&super::super::Limits::forced(64 << 20));
    assert!(!skipped(&forced, fonts));
}

/// What [`instances`] counts in the SVG `body`.
fn weighed(body: &str) -> Result<Made, Refusal> {
    let svg =
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\">{body}</svg>");
    let doc = roxmltree::Document::parse(&svg).unwrap();
    instances(&doc, &Limit::none(), &Matched::default(), &NO_CANCEL)
}

/// The elements of [`weighed`].
fn instanced(body: &str) -> Result<usize, Refusal> {
    weighed(body).map(|m| m.elements)
}

#[test]
fn numbers_in_path_data_start_at_each_point() {
    assert_eq!(numbers(b"M0 0L1.5,2 -3-4"), 6);
    assert_eq!(numbers(b".5.5.5"), 3);
    assert_eq!(numbers(b"1.5.5 2"), 3);
    assert_eq!(numbers(b""), 0);
}

#[test]
fn markers_are_counted_at_the_vertices_they_are_drawn() {
    // A marker of three elements (itself and two circles).
    let marker = "<defs><marker id=\"m\"><circle r=\"1\"/><circle r=\"1\"/></marker></defs>";
    let path = |attrs: &str| format!("{marker}<path d=\"M0 0 L1 1 L2 2 L3 3\" {attrs}/>");
    // Without one, or naming no marker: nothing.
    assert_eq!(instanced(&path("")), Ok(0));
    assert_eq!(instanced(&path("marker-mid=\"url(#none)\"")), Ok(0));
    assert_eq!(instanced(&path("marker-mid=\"none\"")), Ok(0));
    // The end and the start are drawn once each; the middle at no more vertices than the path's
    // numbers.
    assert_eq!(instanced(&path("marker-end=\"url(#m)\"")), Ok(3));
    assert_eq!(instanced(&path("marker-start='url( \"#m\" )'")), Ok(3));
    let mid = instanced(&path("marker-mid=\"url(#m)\"")).unwrap();
    assert!((2 * 3..=8 * 3).contains(&mid), "{mid}");
    // In `style`, and `marker` there sets all three.
    assert_eq!(
        instanced(&path("style=\"marker-mid: url(#m) !important\"")),
        Ok(mid)
    );
    assert_eq!(
        instanced(&path("style=\"marker:url(#m)\"")),
        Ok(mid + 2 * 3)
    );
    // Inherited from what the shape is in, unless that is overridden.
    let inherited =
        format!("{marker}<g marker-mid=\"url(#m)\"><path d=\"M0 0 L1 1 L2 2 L3 3\"/></g>");
    assert_eq!(instanced(&inherited), Ok(mid));
    let none = format!(
        "{marker}<g marker-mid=\"url(#m)\"><path d=\"M0 0 L1 1\" marker-mid=\"none\"/></g>"
    );
    assert_eq!(instanced(&none), Ok(0));
    // Every shape of a polygon, line, rectangle, circle has them too; a clip path's have not.
    for shape in [
        "<polygon points=\"0,0 1,1 2,0\"",
        "<line x2=\"1\"",
        "<rect width=\"1\" height=\"1\"",
        "<circle r=\"1\"",
    ] {
        assert!(
            instanced(&format!("{marker}{shape} marker-mid=\"url(#m)\"/>")).unwrap() > 0,
            "{shape}"
        );
    }
    let clipped = format!(
        "{marker}<clipPath id=\"c\"><path d=\"M0 0 L1 1 L2 2\" marker-mid=\"url(#m)\"/></clipPath>"
    );
    assert_eq!(instanced(&clipped), Ok(0));
    // What is in a marker counts: its `<use>`s and its shapes' markers.
    let nested = "<defs><marker id=\"a\"><circle r=\"1\"/></marker><marker id=\"b\"><path d=\"M0 0 L1 1\" marker-start=\"url(#a)\" marker-end=\"url(#a)\"/></marker></defs><path d=\"M0 0 L1 1\" marker-start=\"url(#b)\"/>";
    // `b` (itself and its path), and `a` twice (itself and its circle each).
    assert_eq!(instanced(nested), Ok(2 + 2 * 2));
    // A marker with itself in it draws it nowhere there, as usvg does.
    let looped = "<defs><marker id=\"l\"><path d=\"M0 0 L1 1\" marker-mid=\"url(#l)\"/></marker></defs><path d=\"M0 0 L1 1 L2 2\" marker-mid=\"url(#l)\"/>";
    assert!(instanced(looped).is_ok());
    // A style sheet that sets markers may set them on any shape; so may a `<use>` on its copy.
    let sheet = format!(
        "{marker}<style>path {{ marker-mid: url(#m) }}</style><path d=\"M0 0 L1 1 L2 2 L3 3\"/>"
    );
    assert!(instanced(&sheet).unwrap() > 0);
    let used = format!(
        "{marker}<defs><path id=\"p\" d=\"M0 0 L1 1 L2 2 L3 3\"/></defs><use href=\"#p\" marker-mid=\"url(#m)\"/>"
    );
    assert!(instanced(&used).unwrap() > 0);
    // A used shape's markers are made with each use.
    let twice = format!(
        "{marker}<defs><path id=\"p\" d=\"M0 0 L1 1 L2 2 L3 3\" marker-mid=\"url(#m)\"/></defs><use href=\"#p\"/><use href=\"#p\"/>"
    );
    assert!(instanced(&twice).unwrap() >= 2 * mid);
}

/// A marker of 25 circles on every vertex of a path (what usvg copies at each of them, with
/// nothing to stop it): refused, by what the file would make, before usvg builds a tree of it.
#[test]
fn markers_on_a_long_path_are_refused_quickly() {
    let circles = r#"<circle cx="1" cy="1" r="1"/>"#.repeat(25);
    let d = "L1 1 L2 2 L3 3 L4 4 ".repeat(1_000);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><defs><marker id=\"m\" markerWidth=\"4\" markerHeight=\"4\">{circles}</marker></defs><path d=\"M0 0 {d}\" fill=\"none\" stroke=\"#000\" marker-mid=\"url(#m)\"/></svg>"
    );
    let h = info_of(svg.as_bytes()).unwrap();
    let start = Instant::now();
    let r = render(
        svg.as_bytes(),
        &h.svg,
        (64, 64),
        usize::MAX,
        &NO_CANCEL,
        &mut Fonts::default(),
    );
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    // A forced load is let make them, and what they hold is in what it is charged.
    let forced = Caps::of(&super::super::Limits::forced(1 << 30));
    let room = 90 << 20;
    let r = render_in(
        svg.as_bytes(),
        &h.svg,
        (64, 64),
        room,
        &forced,
        &NO_CANCEL,
        &mut Fonts::default(),
    );
    match r {
        Rendered::Need(need) => assert!(cost(&need, 64, 64) > room, "{}", need.parse),
        _ => panic!("charged less than it holds"),
    }
}

/// Path data of `n` segments: 2 + 2n numbers.
fn data(n: usize) -> String {
    format!("M0 0{}", " L1 1".repeat(n))
}

/// What a copy holds is its elements, the numbers of its path data and its text, not one element
/// each: for what `<use>`, markers and the elements that name something usvg converts for each
/// of them make.
#[test]
fn copies_are_counted_by_what_they_hold() {
    let n = numbers(data(100).as_bytes());
    assert_eq!(n, 202);
    let defs = format!("<defs><path id=\"p\" d=\"{}\"/></defs>", data(100));
    let made = |body: &str| weighed(&format!("{defs}{body}")).unwrap();
    // A `<use>`'s copy has its own path data.
    let uses = made(
        "<use href=\"#p\"/><use href=\"#p\"/><use xlink:href=\"#p\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"/>",
    );
    assert_eq!((uses.elements, uses.numbers), (3, 3 * n));
    // So has a marker at each vertex of what has it (the start and the end, and the middle's).
    let marker = format!(
        "<defs><marker id=\"m\"><path d=\"{}\"/></marker></defs>",
        data(100)
    );
    let marked = weighed(&format!(
        "{marker}<polyline points=\"0,0 1,1 2,2 3,3\" marker-mid=\"url(#m)\"/>"
    ))
    .unwrap();
    assert!(
        marked.numbers >= 2 * n && marked.numbers.is_multiple_of(n),
        "{marked:?}"
    );
    // The filter of an `feImage` that names an element: converted for each element that has it,
    // but once when its units are user space.
    let filter = |units: &str| format!("<filter id=\"f\" {units}><feImage href=\"#p\"/></filter>");
    let rects = "<rect width=\"1\" height=\"1\" filter=\"url(#f)\"/>".repeat(3);
    assert_eq!(made(&format!("{}{rects}", filter(""))).numbers, 3 * n);
    let user = "filterUnits=\"userSpaceOnUse\"";
    assert_eq!(made(&format!("{}{rects}", filter(user))).numbers, n);
    let bbox = "filterUnits=\"userSpaceOnUse\" primitiveUnits=\"objectBoundingBox\"";
    assert_eq!(made(&format!("{}{rects}", filter(bbox))).numbers, 3 * n);
    // A mask's content is converted for each element that has it (by default), a clip path's once
    // (by default).
    let path = format!("<path d=\"{}\"/>", data(100));
    let masked = "<rect width=\"1\" height=\"1\" mask=\"url(#m)\"/>".repeat(3);
    let mask = |units: &str| format!("<mask id=\"m\" {units}>{path}</mask>{masked}");
    assert_eq!(weighed(&mask("")).unwrap().numbers, 3 * n);
    assert_eq!(
        weighed(&mask("maskUnits=\"userSpaceOnUse\""))
            .unwrap()
            .numbers,
        n
    );
    let clipped = "<rect width=\"1\" height=\"1\" clip-path=\"url(#c)\"/>".repeat(3);
    let clip = |units: &str| format!("<clipPath id=\"c\" {units}>{path}</clipPath>{clipped}");
    assert_eq!(weighed(&clip("")).unwrap().numbers, n);
    assert_eq!(
        weighed(&clip("clipPathUnits=\"objectBoundingBox\""))
            .unwrap()
            .numbers,
        3 * n
    );
    // A pattern's elements are cloned for each shape that has it (its path data is shared), and a
    // gradient's stops.
    let shapes = "<rect width=\"1\" height=\"1\" fill=\"url(#g)\"/>".repeat(3);
    let pattern = format!("<pattern id=\"g\">{path}<circle r=\"1\"/></pattern>{shapes}");
    let made_by = weighed(&pattern).unwrap();
    assert_eq!((made_by.elements, made_by.numbers), (3 * 3, 0));
    let gradient = format!(
        "<linearGradient id=\"g\">{}</linearGradient>{shapes}",
        "<stop offset=\"0\"/>".repeat(5)
    );
    assert_eq!(weighed(&gradient).unwrap().numbers, 3 * 5);
    // A `textPath` has its path's data, and the text of a `<use>`'s copy is its own.
    let on_path = weighed(&format!(
        "{defs}<text><textPath href=\"#p\">hi</textPath></text>"
    ))
    .unwrap();
    assert_eq!(on_path.numbers, n);
    let text = made("<text id=\"t\">abc def</text><use href=\"#t\"/><use href=\"#t\"/>");
    assert_eq!(text.text, 2 * 6);
    // What a style sheet names may be had by any element.
    let sheet = weighed(&format!(
        "{defs}<style>rect {{ mask: url(#m) }}</style><mask id=\"m\">{path}</mask><rect/><rect/>"
    ))
    .unwrap();
    assert!(sheet.numbers >= 2 * n, "{sheet:?}");
    // A reference that names itself makes nothing more.
    let looped = "<filter id=\"f\"><feImage href=\"#r\"/></filter><rect id=\"r\" width=\"1\" height=\"1\" filter=\"url(#f)\"/>";
    assert!(weighed(looped).is_ok());
}

/// What `<use>`, markers and the elements that name something usvg converts for each of them copy
/// is charged for what it holds, and in proportion to the file: SVGs that would make gigabytes
/// are refused by a count, before usvg builds them (which can't be stopped), and a forced load
/// is charged for them.
#[test]
fn copies_of_long_content_are_refused_quickly() {
    let long = data(25_000);
    let rects = |attr: &str| format!("<rect width=\"1\" height=\"1\" {attr}/>").repeat(100);
    let shapes = [
        format!(
            "<defs><path id=\"p\" d=\"{long}\"/></defs>{}",
            "<use href=\"#p\"/>".repeat(100)
        ),
        format!(
            "<defs><marker id=\"m\"><path d=\"{long}\"/></marker></defs><polyline fill=\"none\" stroke=\"#000\" marker-mid=\"url(#m)\" points=\"{}\"/>",
            "0,0 1,1 ".repeat(200)
        ),
        format!(
            "<defs><path id=\"p\" d=\"{long}\"/></defs><filter id=\"f\"><feImage href=\"#p\"/></filter>{}",
            rects("filter=\"url(#f)\"")
        ),
        format!(
            "<mask id=\"m\"><path d=\"{long}\"/></mask>{}",
            rects("mask=\"url(#m)\"")
        ),
        format!(
            "<defs><path id=\"p\" d=\"{long}\"/></defs><text><textPath href=\"#p\">hi</textPath></text>{}",
            "<text><textPath href=\"#p\">hi</textPath></text>".repeat(100)
        ),
    ];
    for body in &shapes {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\">{body}</svg>"
        );
        let h = info_of(svg.as_bytes()).unwrap();
        let start = Instant::now();
        let r = render(
            svg.as_bytes(),
            &h.svg,
            (64, 64),
            usize::MAX,
            &NO_CANCEL,
            &mut Fonts::default(),
        );
        assert!(
            matches!(r, Rendered::Refused(Refusal::Complex)),
            "{}",
            &body[..60]
        );
        assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
        // A forced load is let make them, and what they hold is in what it is charged (20 bytes
        // for each number: 100 MB, over its room).
        let room = 90 << 20;
        let forced = Caps::of(&super::super::Limits::forced(1 << 30));
        let r = render_in(
            svg.as_bytes(),
            &h.svg,
            (64, 64),
            room,
            &forced,
            &NO_CANCEL,
            &mut Fonts::default(),
        );
        match r {
            Rendered::Need(need) => assert!(
                cost(&need, 64, 64) > room,
                "{}: {}",
                need.parse,
                &body[..60]
            ),
            _ => panic!("charged less than it holds: {}", &body[..60]),
        }
    }
}

/// Text copied by `<use>` is shaped for each copy: charged for its bytes (with the fonts, as they
/// are drawn).
#[test]
fn copies_of_long_text_are_refused_quickly() {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><defs><text id=\"t\">{}</text></defs>{}</svg>",
        "word ".repeat(400),
        "<use href=\"#t\"/>".repeat(100)
    );
    let h = info_of(svg.as_bytes()).unwrap();
    let start = Instant::now();
    let r = render(
        svg.as_bytes(),
        &h.svg,
        (64, 64),
        usize::MAX,
        &NO_CANCEL,
        &mut vomd_fonts(),
    );
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
}

/// An SVG inside an SVG that can't be drawn is left out and the rest is drawn: broken, nothing
/// more; over a cap (too deep), the SVG is noted as drawn without part of it, which a click draws.
#[test]
fn a_nested_svg_that_cannot_be_drawn_is_left_out() {
    let wrap = |inner: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\" fill=\"#e00\"/><image width=\"8\" height=\"8\" href=\"data:image/svg+xml;base64,{}\"/></svg>",
            base64(inner.as_bytes())
        )
    };
    let draw = |svg: &str| {
        let h = info_of(svg.as_bytes()).unwrap();
        let fonts = &mut Fonts::default();
        match render(
            svg.as_bytes(),
            &h.svg,
            (8, 8),
            CACHE_BYTES,
            &NO_CANCEL,
            fonts,
        ) {
            Rendered::Pixels(b, skipped) => (b, skipped),
            Rendered::Refused(why) => panic!("refused {why:?}"),
            _ => panic!("not drawn"),
        }
    };
    let leaf = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"/>";
    let (b, skipped) = draw(&wrap(leaf));
    assert!(!skipped);
    assert_eq!(rgba(b.data[0]), [0xEE, 0, 0, 255]);
    let (b, skipped) = draw(&wrap("<svg xmlns=\"http://www.w3.org/2000/svg\"><g></svg>"));
    assert!(!skipped, "broken data isn't a limit");
    assert_eq!(rgba(b.data[0]), [0xEE, 0, 0, 255]);
    let (b, skipped) = draw(&wrap(&wrap(&wrap(leaf))));
    assert!(skipped, "three deep");
    assert_eq!(rgba(b.data[0]), [0xEE, 0, 0, 255]);
}

/// How many jobs it takes to draw the SVG `bytes` at its size: each charged what the one before
/// found it needs, as the main thread asks again.
fn jobs_for(bytes: &[u8]) -> usize {
    let h = info_of(bytes).unwrap();
    let mut svg = h.svg;
    let mut fonts = Fonts::default();
    for jobs in 1..=4 {
        let charge = decode_cost(&Header { svg, ..h }, h.w, h.h);
        match render(bytes, &svg, (h.w, h.h), charge, &NO_CANCEL, &mut fonts) {
            Rendered::Need(n) => svg = n,
            Rendered::Pixels(..) => return jobs,
            _ => panic!("refused"),
        }
    }
    panic!("not drawn by 4 jobs");
}

/// The first charge of an SVG covers what a badge or logo needs, so one job draws it (each job
/// is a parse of the file); one with an SVG or a raster image in it is found out by a second.
#[test]
fn a_small_svg_is_drawn_by_its_first_job() {
    for name in [
        "badge.svg",
        "badge-arial.svg",
        "badge.svgz",
        "logo-viewbox.svg",
        "percent.svg",
        "units.svg",
        "one-side.svg",
        "external.svg",
    ] {
        assert_eq!(jobs_for(&fixture(name)), 1, "{name}");
    }
    for name in ["nested-logo.svg", "raster-inside.svg"] {
        assert_eq!(jobs_for(&fixture(name)), 2, "{name}");
    }
}

/// Counting what a file makes is by what its elements are in and inherit, not a walk up from each
/// (seconds, on a file of 90,000 shapes 250 groups deep, which the worker can't be stopped in).
#[test]
fn counting_a_deep_file_is_quick() {
    let shapes = "<rect width=\"1\" height=\"1\"/>".repeat(90_000);
    let body = format!(
        "<linearGradient id=\"g\"><stop/></linearGradient>{}{shapes}{}",
        "<g style=\"opacity:1\">".repeat(250),
        "</g>".repeat(250)
    );
    let flat = format!("<linearGradient id=\"g\"><stop/></linearGradient>{shapes}");
    // The same shapes without the depth is the measure, so a loaded machine makes no difference
    // (counting took some 25 times as long at this depth once, and takes about as long now).
    let start = Instant::now();
    assert_eq!(weighed(&flat), Ok(Made::default()));
    let flat = start.elapsed();
    let start = Instant::now();
    assert_eq!(weighed(&body), Ok(Made::default()));
    let deep = start.elapsed();
    assert!(
        deep < flat * 10 + Duration::from_secs(1),
        "{deep:?} / {flat:?}"
    );
}

/// An SVG of `width` × `height` 64 whose body is `body`.
fn svg_of(body: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\">{body}</svg>")
}

/// Symbol reuse is how SVGs stay small: a map of pins, a pictogram chart and an icon sheet make
/// hundreds of copies of a few elements from a few kilobytes, which is ordinary, not too complex
/// to draw (what they make is charged against the budget all the same).
#[test]
fn small_svgs_that_reuse_a_symbol_are_drawn() {
    let uses = |n: usize, ids: usize| {
        (0..n)
            .map(|i| format!("<use href=\"#s{}\" x=\"{i}\" y=\"{i}\"/>", i % ids))
            .collect::<String>()
    };
    let symbols = |n: usize, segments: usize, paths: usize| {
        (0..n)
            .map(|i| {
                let path = format!("<path d=\"{}\"/>", data(segments)).repeat(paths);
                format!("<symbol id=\"s{i}\"><g>{path}</g></symbol>")
            })
            .collect::<String>()
    };
    let shapes = [
        // A map: 300 pins of two paths.
        format!("{}{}", symbols(1, 21, 2), uses(300, 1)),
        // A pictogram chart: 200 people of a path of 250 numbers.
        format!("{}{}", symbols(1, 124, 1), uses(200, 1)),
        // An icon sheet: 400 icons of ten symbols of 120 numbers.
        format!("{}{}", symbols(10, 59, 1), uses(400, 10)),
    ];
    for body in &shapes {
        let svg = svg_of(body);
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let made = instances(&doc, &Limit::none(), &Matched::default(), &NO_CANCEL).unwrap();
        // More than the file's size in numbers, which was the limit.
        assert!(made.numbers > 10_000 + svg.len(), "{made:?} {}", svg.len());
        let b = quick(svg.as_bytes(), 64, 64);
        assert!(b.is_some(), "{} bytes", svg.len());
    }
}

/// The floors are for what draws in a fraction of a second: what they let through is drawn
/// quickly, and what is over them is refused.
#[test]
fn what_the_floors_let_through_is_drawn_quickly() {
    let path = format!("<path id=\"p\" d=\"{}\"/>", data(24_000));
    let group = format!(
        "<g id=\"g\">{}</g>",
        "<rect width=\"1\" height=\"1\"/>".repeat(49)
    );
    let shapes = [
        // About a million numbers in copies of one path.
        (
            format!("<defs>{path}</defs>{}", "<use href=\"#p\"/>".repeat(20)),
            true,
        ),
        (
            format!("<defs>{path}</defs>{}", "<use href=\"#p\"/>".repeat(30)),
            false,
        ),
        // About 28,000 elements (what the budget has room for) and 55,000 in copies of one group.
        (
            format!("<defs>{group}</defs>{}", "<use href=\"#g\"/>".repeat(570)),
            true,
        ),
        (
            format!("<defs>{group}</defs>{}", "<use href=\"#g\"/>".repeat(1_100)),
            false,
        ),
    ];
    for (body, drawn) in &shapes {
        let svg = svg_of(body);
        let start = Instant::now();
        assert_eq!(decode(svg.as_bytes(), 64, 64).is_some(), *drawn);
        assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    }
}

/// Shapes get a marker of a style sheet that sets one (`marker-start`, `marker-mid`, `marker-end`
/// or `marker`), not by a class of the name: mermaid's sheet always has `.marker`.
#[test]
fn a_style_sheet_charges_markers_only_for_the_properties_it_sets() {
    assert_eq!(sets_markers(".marker{fill:#333}"), [false; 3]);
    assert_eq!(
        sets_markers(".marker:hover { x: y } #marker-mid {a:b}"),
        [false; 3]
    );
    assert_eq!(sets_markers("marker { fill: red }"), [false; 3]);
    assert_eq!(
        sets_markers("path{Marker-Mid : url(#m)}"),
        [false, true, false]
    );
    assert_eq!(
        sets_markers("path{fill:red;marker-end:none} /* c */ rect{marker-start:url(#m)}"),
        [true, false, true]
    );
    assert_eq!(sets_markers("g { marker : url(#m) }"), [true; 3]);
    // A mermaid-style flowchart (80 nodes with HTML labels, 80 edges of 9 curves with arrow
    // markers), which has the class: its markers are the edges' own.
    let nodes = (0..80)
        .map(|i| {
            format!(
                "<g class=\"node\" transform=\"translate({i},0)\"><rect width=\"90\" height=\"30\"/><foreignObject width=\"90\" height=\"30\"><div xmlns=\"http://www.w3.org/1999/xhtml\"><span class=\"nodeLabel\">Node {i}</span></div></foreignObject></g>"
            )
        })
        .collect::<String>();
    let curves = "C1 1 2 2 3 3 ".repeat(9);
    let edges = (0..80)
        .map(|i| {
            format!(
                "<path class=\"flowchart-link\" d=\"M{i} 0 {curves}\" marker-end=\"url(#arrow)\"/>"
            )
        })
        .collect::<String>();
    let flow = format!(
        "<style>.marker{{fill:#333;stroke:#333}}.node rect{{fill:#eee}}.flowchart-link{{fill:none}}</style><defs><marker id=\"arrow\"><path d=\"M0 0 L10 5 L0 10 z\"/></marker></defs>{nodes}{edges}"
    );
    // The arrow (itself and its path) once for each edge, and nothing for the rest.
    assert_eq!(instanced(&flow), Ok(80 * 2));
    // A sheet that does set one charges every shape for the dearest marker: for the middle's, at
    // every vertex.
    let sets = flow.replace(".marker{", "path{marker-mid:url(#arrow)}.marker{");
    assert!(instanced(&sets).unwrap() > 80 * 2 + 80 * 9 * 2);
}

/// A curve is a vertex, not its six numbers.
#[test]
fn path_data_has_a_vertex_for_each_segment() {
    assert_eq!(path_vertices(b""), 0);
    assert_eq!(path_vertices(b"M0 0"), 1);
    assert_eq!(path_vertices(b"M0 0C1 1 2 2 3 3"), 2);
    // Another curve with no command, a line of two numbers, and the `z`.
    assert_eq!(path_vertices(b"M0 0C1 1 2 2 3 3 4 4 5 5 6 6L1 1 2 2z"), 6);
    assert_eq!(path_vertices(b"M0 0h1v2H3V4Z"), 6);
    // An arc is two at most: its flags may be written with no space before the next number.
    assert_eq!(path_vertices(b"M0 0a1 1 0 0 1 2 2"), 3);
    // An exponent is not a command (it counts as a number, as `numbers` has it: too many).
    assert_eq!(path_vertices(b"M1e2 1E2l1e-1 2"), 4);
}

/// What is counted for a forced load is no more than fits its room, and counting a document made
/// to take long (20,000 copies of a path of 200,000 points) ends at once, and is let go of when
/// told to.
#[test]
fn counting_what_a_forced_load_could_never_hold_ends_at_once() {
    let svg = svg_of(&format!(
        "<defs><path id=\"p\" d=\"{}\" marker-mid=\"url(#m)\"/><marker id=\"m\"><rect width=\"1\" height=\"1\"/></marker></defs>{}",
        data(200_000),
        "<use href=\"#p\"/>".repeat(20_000)
    ));
    assert!(svg.len() > 1_200_000);
    let h = info_of(svg.as_bytes()).unwrap();
    let forced = Caps::of(&Limits::forced(256 << 20));
    let start = Instant::now();
    let r = render_in(
        svg.as_bytes(),
        &h.svg,
        (64, 64),
        256 << 20,
        &forced,
        &NO_CANCEL,
        &mut Fonts::default(),
    );
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    // Needs more than there is: the load is refused for memory.
    match r {
        Rendered::Need(need) => assert!(cost(&need, 64, 64) > 256 << 20, "{}", need.parse),
        _ => panic!("not refused for its memory"),
    }
    // Told to stop, counting does.
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let cancel = AtomicBool::new(true);
    let start = Instant::now();
    assert!(instances(&doc, &Limit::none(), &Matched::default(), &cancel).is_err());
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
}

/// `svg`'s render by a job charged all it asks for, forced or not, and how long it took.
fn rendered_in(svg: &str, caps: &Caps) -> (Rendered, Duration) {
    let h = info_of(svg.as_bytes()).unwrap();
    let start = Instant::now();
    let r = render_in(
        svg.as_bytes(),
        &h.svg,
        (64, 64),
        usize::MAX,
        caps,
        &NO_CANCEL,
        &mut vomd_fonts(),
    );
    (r, start.elapsed())
}

fn forced_caps() -> Caps {
    Caps::of(&Limits::forced(1 << 30))
}

/// usvg matches every rule to every element, and a selector of descendant combinators backtracks
/// over the elements it is in, a number of ways that grows by a factor for each (a 400 byte file
/// with a selector of eight took seven seconds, and one of twenty would never end): the matching
/// is counted as usvg does it, and refused past what is let, forced loads too.
#[test]
fn a_style_sheet_that_takes_long_to_match_is_refused_quickly() {
    let nested = format!("{}{}", "<g x=\"y\">".repeat(40), "</g>".repeat(40));
    let sheet = |css: &str| svg_of(&format!("<style>{css}</style>{nested}"));
    let selectors = [
        // Descendant combinators on an element that never matches the first.
        format!("a{} {{fill:red}}", " g".repeat(20)),
        format!("a{} {{fill:red}}", " g".repeat(12)),
        // Through `:first-child` and attribute selectors, which are asked a good deal each.
        format!("a{} {{fill:red}}", " g:first-child".repeat(20)),
        format!("a{} {{fill:red}}", " g[x]".repeat(20)),
        format!("a{} {{fill:red}}", " g[x=y]".repeat(20)),
        // Adjacent siblings and children cost a step each, but make no more ways.
        format!("a{} {{fill:red}}", " > g > g g".repeat(7)),
    ];
    for css in &selectors {
        let svg = sheet(css);
        assert!(svg.len() < 1_000, "{} bytes", svg.len());
        for caps in [Caps::NORMAL, forced_caps()] {
            let (r, took) = rendered_in(&svg, &caps);
            assert!(matches!(r, Rendered::Refused(Refusal::Complex)), "{css}");
            assert!(took < SANE, "{css}: {took:?}");
        }
    }
    // A selector of adjacent siblings recurses once for each: a long one (20,000, and as many
    // siblings) is refused, not matched on the worker's stack.
    let siblings = svg_of(&format!(
        "<style>{} g{{fill:red}}</style>{}",
        "g +".repeat(20_000),
        "<g/>".repeat(20_001)
    ));
    for caps in [Caps::NORMAL, forced_caps()] {
        let (r, took) = rendered_in(&siblings, &caps);
        assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
        assert!(took < SANE, "{took:?}");
    }
    // Told to stop, matching does (soon after: it looks every so many steps).
    let svg = sheet(&selectors[0]);
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let cancel = AtomicBool::new(true);
    let start = Instant::now();
    assert!(css_costs(&doc, &style_sheet(&doc), usize::MAX, &cancel).is_err());
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    // Many rules to many elements: 3,000 rules and 20,000 elements are 60 million steps.
    let rules = (0..3_000)
        .map(|i| format!(".c{i}{{fill:red}}"))
        .collect::<String>();
    let many = svg_of(&format!(
        "<style>{rules}</style>{}",
        "<rect width=\"1\" height=\"1\" class=\"c1\"/>".repeat(20_000)
    ));
    let (r, took) = rendered_in(&many, &Caps::NORMAL);
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
    // And as many declarations, which each matching rule applies to each element.
    let decls = format!("*{{{}}}", "fill:red;".repeat(20_000));
    let many = svg_of(&format!(
        "<style>{decls}</style>{}",
        "<rect width=\"1\" height=\"1\"/>".repeat(2_000)
    ));
    let (r, took) = rendered_in(&many, &Caps::NORMAL);
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
}

/// What real style sheets cost is far from what is let: a flowchart's sheet of forty rules
/// with descendant selectors, to three thousand elements, draws; a sheet with a few descendant
/// combinators on a deep tree too.
#[test]
fn ordinary_style_sheets_are_matched_and_drawn() {
    let rules = (0..20)
        .map(|i| format!(".flowchart .node .c{i} rect, g.edge path:first-child {{fill:#eee}}"))
        .collect::<String>();
    let body = (0..1_000)
        .map(|i| {
            format!(
                "<g class=\"node c{}\"><rect width=\"3\" height=\"3\"/><path d=\"M0 0L1 1\"/></g>",
                i % 20
            )
        })
        .collect::<String>();
    let svg = svg_of(&format!(
        "<style>{rules}</style><g class=\"flowchart\">{body}</g>"
    ));
    let start = Instant::now();
    let b = decode(svg.as_bytes(), 64, 64).unwrap();
    assert!(start.elapsed() < SANE, "{:?}", start.elapsed());
    assert!(b.data.iter().any(|&p| p >> 24 != 0));
    // An icon theme's sheet of a hundred rules, three declarations each, with a class on each of a
    // thousand paths: each sets its three, not all 300 of the sheet's on every element.
    let classes = (0..100)
        .map(|i| format!(".c{i}{{fill:#abc;stroke:#def;opacity:.5}}"))
        .collect::<String>();
    let paths = (0..1_000)
        .map(|i| format!("<path class=\"c{}\" d=\"M0 0L1 1\"/>", i % 100))
        .collect::<String>();
    let icon = svg_of(&format!("<style>{classes}</style>{paths}"));
    assert!(quick(icon.as_bytes(), 64, 64).is_some());
    // What the sheet makes of a deep tree: 30 deep, 4 combinators.
    let deep = svg_of(&format!(
        "<style>svg g g g g rect{{fill:#0f0}}</style>{}<rect width=\"64\" height=\"64\"/>{}",
        "<g>".repeat(30),
        "</g>".repeat(30)
    ));
    let b = quick(deep.as_bytes(), 64, 64).unwrap();
    assert_eq!(rgba(b.data[32 * 64 + 32]), [0, 0xFF, 0, 255]);
    // The matching a style sheet takes is counted for each copy of an element.
    let svg = svg_of(&format!(
        "<style>{rules}</style><defs><g id=\"g\" class=\"node\"><rect/></g></defs>{}",
        "<use href=\"#g\"/>".repeat(40)
    ));
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let costs = css_costs(&doc, &style_sheet(&doc), usize::MAX, &NO_CANCEL).unwrap();
    let made = instances(&doc, &Limit::none(), &costs, &NO_CANCEL).unwrap();
    let base: usize = costs.steps.iter().map(|&c| c as usize).sum();
    assert!(made.css > base + 40 * 2 * 100, "{} {base}", made.css);
}

/// usvg shapes the whole of a text chunk for each span in it, so spans make work with the square of
/// them (4,000 `tspan`s of two letters took fourteen seconds, with no other position than the
/// first): counted by what a chunk holds, and refused past what is let, forced loads and copies
/// too. What starts a chunk (a `tspan` with a position) makes it short.
#[test]
fn text_of_many_spans_in_a_chunk_is_refused_quickly() {
    let text = |n: usize| {
        svg_of(&format!(
            "<text y=\"9\">{}</text>",
            "<tspan>ab</tspan>".repeat(n)
        ))
    };
    for (n, caps) in [(4_000, Caps::NORMAL), (30_000, forced_caps())] {
        let (r, took) = rendered_in(&text(n), &caps);
        assert!(matches!(r, Rendered::Refused(Refusal::Complex)), "{n}");
        assert!(took < SANE, "{n}: {took:?}");
    }
    // Different fonts for each make no difference to what is counted.
    let fonts = svg_of(&format!(
        "<text y=\"9\">{}</text>",
        (0..4_000)
            .map(|i| format!("<tspan font-family=\"f{i}\">ab</tspan>"))
            .collect::<String>()
    ));
    let (r, took) = rendered_in(&fonts, &Caps::NORMAL);
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
    // Copies of 900 spans (a tenth of what a chunk may hold, forced), ten of them.
    let copies = svg_of(&format!(
        "<defs><text id=\"t\" y=\"9\">{}</text></defs>{}",
        "<tspan>ab</tspan>".repeat(900),
        "<use href=\"#t\"/>".repeat(10)
    ));
    let (r, took) = rendered_in(&copies, &forced_caps());
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
    // Fifty lines of a tspan each, with a position, as Inkscape writes them, are fifty chunks.
    let lines = svg_of(&format!(
        "<text font-size=\"2\">{}</text>",
        (0..50)
            .map(|i| format!(
                "<tspan x=\"1\" y=\"{}\">a line of text, {i}, <tspan font-weight=\"bold\">bold</tspan> and more</tspan>",
                i + 1
            ))
            .collect::<String>()
    ));
    let doc = roxmltree::Document::parse(&lines).unwrap();
    assert!(weighed_doc(&doc).shaping < MAX_SHAPING / 10);
    let b = with_fonts(lines.as_bytes(), &mut vomd_fonts());
    assert!(b.data.iter().any(|&p| p >> 24 != 0));
    // A sentence of three spans is nothing.
    let sentence =
        svg_of("<text y=\"9\">One <tspan font-weight=\"bold\">bold</tspan> word.</text>");
    let doc = roxmltree::Document::parse(&sentence).unwrap();
    assert!(weighed_doc(&doc).shaping < 1_000);
}

/// The glyphs of a `textPath` are set along it by a walk of its segments: a path of 250,000
/// segments and 14,000 letters took seconds, and is counted by both.
#[test]
fn text_on_a_long_path_is_refused_quickly() {
    let svg = |segments: usize, letters: usize| {
        svg_of(&format!(
            "<defs><path id=\"p\" d=\"M0 0{}\"/></defs><text><textPath href=\"#p\">{}</textPath></text>",
            " l1 1".repeat(segments),
            "a".repeat(letters)
        ))
    };
    let (r, took) = rendered_in(&svg(250_000, 14_000), &Caps::NORMAL);
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
    let (r, took) = rendered_in(&svg(500_000, 30_000), &forced_caps());
    assert!(matches!(r, Rendered::Refused(Refusal::Complex)));
    assert!(took < SANE, "{took:?}");
    // Short ones are drawn.
    let (r, _) = rendered_in(&svg(500, 200), &Caps::NORMAL);
    assert!(matches!(r, Rendered::Pixels(..)));
}

/// resvg keeps every primitive's result of a filter, and a few more pixmaps as it makes one: a
/// drop shadow of five primitives (as the flags of an icon theme have, with a region of four times
/// the icon's sides) holds six, not fourteen, and is drawn within what the cache holds.
#[test]
fn a_filter_is_charged_the_pixmaps_resvg_holds() {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"256\" height=\"256\"><defs><filter id=\"f\" width=\"400%\" height=\"400%\" x=\"-200%\" y=\"-200%\" color-interpolation-filters=\"sRGB\" filterUnits=\"objectBoundingBox\"><feGaussianBlur in=\"SourceGraphic\" stdDeviation=\"4.294\"/><feOffset dx=\"0\" dy=\"4\" result=\"o\"/><feFlood flood-opacity=\".4\"/><feComposite in2=\"o\" operator=\"in\" result=\"d\"/><feBlend in=\"SourceGraphic\" in2=\"d\" mode=\"normal\"/></filter></defs><g filter=\"url(#f)\"><path fill=\"#BC1938\" d=\"M165 12C239 12 250 22 250 96L250 155C250 229 239 240 165 240L90 240C16 240 5 229 5 155L5 96C5 22 16 12 90 12Z\"/></g></svg>";
    let tree = tree_of(svg.as_bytes());
    let usvg::Node::Group(g) = &tree.root().children()[0] else {
        panic!("no group");
    };
    assert_eq!(g.filters().len(), 1);
    assert_eq!(filter_pixmaps(&g.filters()[0]), 6);
    assert!(decode(svg.as_bytes(), 256, 256).is_some());
    let h = info_of(svg.as_bytes()).unwrap();
    let r = render(
        svg.as_bytes(),
        &h.svg,
        (256, 256),
        CACHE_BYTES,
        &NO_CANCEL,
        &mut Fonts::default(),
    );
    // Drawn by the first job or the second, within the cache's budget.
    match r {
        Rendered::Pixels(..) => {}
        Rendered::Need(need) => assert!(cost(&need, 256, 256) <= CACHE_BYTES, "{need:?}"),
        _ => panic!("not drawn"),
    }
}

/// What [`instances`] counts in `doc`, counting no style sheet.
fn weighed_doc(doc: &roxmltree::Document) -> Made {
    instances(doc, &Limit::none(), &Matched::default(), &NO_CANCEL).unwrap()
}

/// simplecss (vendored: see its `VOMD-PATCH.md`) took the square of a style sheet's length to read
/// it (40,000 rules, 440 KB, took eight seconds, in a call that can't be stopped), and copied the
/// declarations of a rule for each selector of its list (20,000 selectors and 2,000 declarations
/// are 40 million).
#[test]
fn a_style_sheet_is_read_in_linear_time() {
    let rect = "<rect width=\"1\" height=\"1\"/>";
    // The sheets at `n` rules, the third matched by the one rect: at 40,000, 20,000 rules of 2,000
    // declarations is far past what is let.
    let sheets = |n: usize| {
        [
            "a{fill:red}".repeat(n),
            format!("*{{{}}}", "fill:red;".repeat(n)),
            format!(
                "{}rect{{{}}}",
                "rect,".repeat(n / 2),
                "fill:red;".repeat(n / 20)
            ),
        ]
    };
    let read = |css: &str| {
        let svg = svg_of(&format!("<style>{css}</style>{rect}"));
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let start = Instant::now();
        let sheet = style_sheet(&doc);
        let took = start.elapsed();
        assert!(!sheet.rules.is_empty());
        (svg, took)
    };
    for (small, css) in sheets(4_000).iter().zip(&sheets(40_000)) {
        // Ten times the rules take about ten times as long, not a hundred: the measure is the same
        // sheet at a tenth of the size, so a loaded machine makes no difference (the square took
        // eight seconds at 40,000 rules).
        let (_, t4k) = read(small);
        let (svg, t40k) = read(css);
        assert!(
            t40k < t4k * 30 + Duration::from_millis(500),
            "{} bytes: {t40k:?} / {t4k:?}",
            css.len()
        );
        let (_, took) = rendered_in(&svg, &Caps::NORMAL);
        assert!(took < SANE, "{} bytes: {took:?}", css.len());
    }
    // The lists share their declarations, and what they hold is charged for the bytes.
    let svg = svg_of(&format!(
        "<style>{}rect{{{}}}</style>{rect}",
        "rect,".repeat(20_000),
        "fill:red;".repeat(2_000)
    ));
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let sheet = style_sheet(&doc);
    assert_eq!(sheet.rules.len(), 20_001);
    assert!(std::ptr::eq(
        sheet.rules[0].declarations.as_ptr(),
        sheet.rules[20_000].declarations.as_ptr()
    ));
}

/// What `decode` makes of `svg` at the sizes the `svg` fuzz target draws: no panic, and the size
/// asked for when anything is drawn.
fn drawn_at_the_fuzz_sizes(svg: &[u8]) {
    let Some((Format::Svg, w, h)) = header_of(svg) else {
        return;
    };
    assert!(w > 0 && h > 0);
    for (bw, bh) in [(64, 64), (512, 512)] {
        if let Some(b) = decode(svg, bw, bh) {
            assert_eq!((b.width, b.height), (bw, bh));
        }
    }
}

/// Every input of a past fuzz finding (fuzz/seeds/svg/).
#[test]
fn the_svgs_of_past_fuzz_findings_are_drawn_or_refused() {
    let dir = format!("{}/fuzz/seeds/svg", env!("CARGO_MANIFEST_DIR"));
    let mut seeds = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        drawn_at_the_fuzz_sizes(&std::fs::read(entry.unwrap().path()).unwrap());
        seeds += 1;
    }
    assert!(seeds > 0);
}

/// Numbers an `f32` can hardly hold, in the sizes, positions, transforms and radii of every kind
/// of element that makes a rectangle or a size out of them: each of these panicked, in usvg's
/// parser or in resvg's drawing, or in the walk that charges the drawing (where overflow is
/// checked). The SVG is drawn or refused.
#[test]
fn extreme_numbers_in_shapes_are_drawn_or_refused() {
    let png = format!("data:image/png;base64,{}", base64(&striped_png()));
    let nested = format!(
        "data:image/svg+xml;base64,{}",
        base64(b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='16'><rect width='24' height='16'/></svg>")
    );
    let mut bodies = Vec::new();
    // A miter limit past `f32::MAX` is infinite (usvg asserted it was finite).
    for limit in ["1e39", "1e300"] {
        bodies.push(format!(
            "<path d='M0 0 L50 20 L0 30' stroke='red' stroke-miterlimit='{limit}'/>"
        ));
    }
    for n in ["1e38", "1e30", "1e300", "1e-38", "4444444444444444402"] {
        for href in [&png, &nested] {
            // Far from zero, an image's size has no room beside its position (`f32`).
            for (x, y, w, h) in [
                ("0", "0", "24", n),
                ("0", "0", n, "16"),
                (n, n, n, n),
                ("0", n, "24", "40"),
            ] {
                for aspect in ["", " preserveAspectRatio='xMidYMid slice'"] {
                    bodies.push(format!(
                        "<image x='{x}' y='{y}' width='{w}' height='{h}'{aspect} href='{href}'/>"
                    ));
                }
            }
            bodies.push(format!(
                "<g transform='translate({n} {n}) rotate(90)'><image width='24' height='{n}' href='{href}'/></g>"
            ));
        }
        let path = "<path d='M0 0 L10 10 L20 0'";
        bodies.extend([
            // A stroke too wide for the marker's size.
            format!("<defs><marker id='m' markerWidth='5' markerHeight='5' viewBox='0 0 10 10'><circle r='5'/></marker></defs>{path} stroke-width='{n}' marker-mid='url(#m)'/>"),
            // A region, mask or pattern in the shape's own units, of numbers too large.
            format!("<defs><filter id='f' x='{n}' y='{n}' width='{n}' height='{n}'><feGaussianBlur stdDeviation='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><mask id='k' x='{n}' y='{n}' width='{n}' height='{n}'><rect width='5' height='5'/></mask></defs><rect width='40' height='40' mask='url(#k)'/>"),
            format!("<defs><pattern id='p' width='{n}' height='{n}' viewBox='0 0 {n} {n}'><rect width='5' height='5'/></pattern></defs><rect width='40' height='40' fill='url(#p)'/>"),
            format!("<defs><pattern id='p' width='{n}' height='{n}' patternUnits='userSpaceOnUse'><rect width='5' height='5'/></pattern></defs><rect width='40' height='40' fill='url(#p)'/>"),
            // Filter primitives: the numbers a drawing turns into pixels and offsets.
            format!("<defs><filter id='f'><feMorphology radius='{n}'/><feDropShadow dx='{n}' dy='{n}' stdDeviation='{n}'/><feOffset dx='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='{n}' height='{n}'><feConvolveMatrix order='3' divisor='{n}' kernelMatrix='1 1 1 1 1 1 1 1 1'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f'><feConvolveMatrix order='3' divisor='{n}' kernelMatrix='1 1 1 1 1 1 1 1 1'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            // A stroke or a transform that carries the outline far from the canvas (`geom::stroke`
            // of resvg), also with joins and curves of its own.
            format!("<defs><marker id='m' markerWidth='5' markerHeight='5' viewBox='0 0 {n} {n}'><circle r='5'/></marker></defs>{path} stroke='red' stroke-width='{n}' marker-end='url(#m)'/>"),
            format!("{path} stroke='red' stroke-width='{n}' stroke-linejoin='miter-clip'/>"),
            format!("<path d='M0 0 C20 40 60 -40 10 30 S50 5 60 40' fill='red' stroke='blue' stroke-width='{n}' stroke-linecap='square'/>"),
            format!("<g transform='scale({n})'><path d='M0 0 C20 40 60 -40 10 30 S50 5 60 40' fill='red' stroke='blue'/></g>"),
            // A curve too large for the stroker's arithmetic, also after a transform that shrinks it.
            format!("<path d='M0 0 Q{n} {n} {n} 0 C{n} 9 5 {n} 7 8' fill='red' stroke='blue'/>"),
            format!("<g transform='scale(1e-30) rotate(30)'><path d='M0 0 Q{n} 5 {n} 0' stroke='blue' stroke-width='1e30'/></g>"),
            format!("<g transform='translate(0 {n})'><rect width='60' height='{n}' fill='red'/></g>"),
            // Numbers an overflow check or a finite-number assertion stops (the fuzz target runs both).
            format!("<defs><filter id='f'><feTurbulence baseFrequency='{n}' numOctaves='3' seed='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f'><feTurbulence baseFrequency='{n} 0.1' seed='-{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f'><feGaussianBlur stdDeviation='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<rect width='40' height='40' filter='blur({n}px)'/>"),
            format!("<defs><filter id='f' primitiveUnits='objectBoundingBox'><feGaussianBlur stdDeviation='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f'><feComponentTransfer><feFuncR type='gamma' amplitude='{n}' exponent='{n}' offset='{n}'/></feComponentTransfer><feComposite operator='arithmetic' k1='{n}' k2='{n}' k3='{n}' k4='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<g opacity='.5' transform='matrix({n} 0 0 {n} {n} {n})'><rect width='5' height='5' stroke='red'/></g>"),
            // A number bounded as an `f32` (infinite when it is too large), and orders whose
            // product overflows a `u32` (usvg asserted and multiplied unchecked).
            format!("<defs><filter id='f'><feColorMatrix type='saturate' values='{n}'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><linearGradient id='g'><stop offset='{n}' stop-color='red'/><stop offset='-{n}'/></linearGradient></defs><rect width='40' height='40' fill='url(#g)'/>"),
            format!("<defs><filter id='f'><feConvolveMatrix order='{n}' kernelMatrix='1'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
            format!("<defs><filter id='f'><feConvolveMatrix order='{n} 65536'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"),
        ]);
    }
    for order in [
        "65536",
        "65536 65536",
        "65537 65537",
        "1 4294967297",
        "100000 100000",
    ] {
        bodies.push(format!(
            "<defs><filter id='f'><feConvolveMatrix order='{order}' kernelMatrix='1'/></filter></defs><rect width='40' height='40' filter='url(#f)'/>"
        ));
    }
    for body in bodies {
        for root in [
            "width='60' height='40'",
            "width='60' height='40' viewBox='0 0 60 40'",
        ] {
            drawn_at_the_fuzz_sizes(
                format!("<svg xmlns='http://www.w3.org/2000/svg' {root}>{body}</svg>").as_bytes(),
            );
        }
    }
}

/// A path far from the canvas (`geom::fill` and `stroke` of resvg) is clipped to it and drawn:
/// tiny-skia's anti-aliased scan panics on coordinates of about 2²⁴ pixels and more. The visible
/// part is what the whole path draws there.
#[test]
fn a_path_far_from_the_canvas_is_clipped_and_drawn() {
    let px = |svg: &str, w: u32, h: u32, x: u32, y: u32| {
        let svg =
            format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'>{svg}</svg>");
        let b = decode(svg.as_bytes(), w, h).unwrap();
        rgba(b.data[(y * w + x) as usize])
    };
    let (red, blue, none) = ([255, 0, 0, 255], [0, 0, 0xFF, 255], [0, 0, 0, 0]);
    // Covering the canvas from far off: a wide curve stroke, a large scale, a position far above
    // (the fuzz finding, a stroke of 2³¹, is a seed: its outline is too coarse an `f32` to say).
    for far in [
        "<path d='M0 0 C20 40 60 -40 10 30' fill='none' stroke='red' stroke-width='686107850000' stroke-linejoin='miter-clip'/>",
        "<g transform='scale(1e9)'><rect x='-1' y='-1' width='2' height='2' fill='red'/></g>",
        "<rect y='-2e9' width='60' height='4e9' fill='red'/>",
    ] {
        assert_eq!(px(far, 60, 40, 50, 30), red, "{far}");
    }
    // Where HEAD drew, it is drawn alike: a background of a million pixels, a large scale, a
    // position far above, hairlines, wide strokes along a long path.
    for near in [
        "<rect width='1000000' height='1000000' fill='#00f'/>",
        "<g transform='scale(100000)'><rect width='10' height='10' fill='#00f'/></g>",
        "<rect y='-400000000' width='60' height='400000040' fill='#00f'/>",
        "<path d='M0 -3e8 L60 -3e8 L60 3e8 L0 3e8Z' fill='#00f' stroke='#00f' stroke-width='1e6'/>",
        "<line x1='30.5' y1='0' x2='30.5' y2='6e8' stroke='#00f'/>",
        "<line x1='30.5' y1='-6e8' x2='30.5' y2='40' stroke='#00f'/>",
        "<path d='M10 -50 L60 -50 L60 1.1e9 L10 1.1e9 Z' fill='#00f'/>",
        "<path d='M10 -1.3e9 L60 -1.3e9 L60 3e8 L10 3e8 Z' fill='#00f'/>",
        "<path d='M30 -50 L30 1.6e9' stroke='#00f' stroke-width='4'/>",
    ] {
        assert_eq!(px(near, 60, 40, 30, 20), blue, "{near}");
    }
    let frame = "<rect x='10.5' y='10.5' width='40' height='6e8' fill='none' stroke='#00f'/>";
    assert_eq!(px(frame, 60, 40, 10, 20), blue, "{frame}");
    // A subpath far above leaves the other subpath of its path alone (it once left the path out).
    for far_part in [
        "M0 -1073741696 L5 -1073741696 L5 -1073741440 Z",
        "M-1e9 -1e9 L1e9 1e9 L1e9 -1e9 Z",
    ] {
        let square = "M10 200 L60 200 L60 250 L10 250 Z";
        assert_eq!(
            px(
                &format!("<path d='{far_part} {square}' fill='red'/>"),
                300,
                300,
                30,
                220
            ),
            red,
            "{far_part}"
        );
        assert_eq!(
            px(
                &format!(
                    "<path d='{far_part} {square}' fill='red' stroke='red' stroke-width='4'/>"
                ),
                300,
                300,
                30,
                220
            ),
            red,
            "{far_part}"
        );
    }
    // A single vertex far to the side makes a triangle with a sliver on the canvas, between the
    // edges to the vertices (100, 110) and (200, 290): (50, 200) is inside, (250, 200) and
    // (50, 50) aren't. Of random such triangles, a third panicked in tiny-skia.
    for x in [
        "-1e7", "-1.8e7", "-3e7", "-6e8", "-1e9", "-2e9", "-1e12", "-1e30",
    ] {
        let tri = format!("<path d='M{x} 80 L200 290 L100 110 Z' fill='red'/>");
        assert_eq!(px(&tri, 300, 300, 50, 200), red, "{x}");
        assert_eq!(px(&tri, 300, 300, 250, 200), none, "{x}");
        assert_eq!(px(&tri, 300, 300, 50, 50), none, "{x}");
    }
    // Without anti-aliasing (`shape-rendering`) too: tiny-skia's cubic clipper unwraps on a path
    // of cubics some millions of pixels out.
    for x in ["-1e7", "-6e8"] {
        let tri = format!(
            "<path shape-rendering='crispEdges' d='M{x} 80 L200 290 L100 110 Z' fill='red'/>"
        );
        assert_eq!(px(&tri, 300, 300, 50, 200), red, "{x}");
        assert_eq!(px(&tri, 300, 300, 250, 200), none, "{x}");
    }
    px(
        "<path shape-rendering='crispEdges' d='M230.91464 0.17951131 C3520877 -3535824 -867679 -2707004 -1494787 1360698 Q-4238595 4001250 -2357208 1340155 C-8097520 -5181918 -2275875 7765701 -8330213 -7830216 Z' fill='red'/>",
        300,
        300,
        0,
        0,
    );
    // The same stroked: the sides of the triangle, not its inside.
    let b = decode(
        b"<svg xmlns='http://www.w3.org/2000/svg' width='300' height='300'><path d='M-6e8 80 L200 290 L100 110 Z' fill='none' stroke='red' stroke-width='6'/></svg>",
        300,
        300,
    )
    .unwrap();
    assert_eq!(rgba(b.data[(200 * 300 + 50) as usize]), none);
    assert_eq!(rgba(b.data[(290 * 300 + 50) as usize]), red);
}

/// The same on a canvas drawn in tiles (a side over 8,191 pixels), where tiny-skia moves the path
/// by each tile's offset.
#[test]
fn a_path_far_from_a_canvas_in_tiles_is_clipped_and_drawn() {
    let draw = |body: &str| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='9000'>{body}</svg>"
        );
        decode(svg.as_bytes(), 40, 9000).unwrap()
    };
    let at = |b: &crate::image::Bitmap, x: u32, y: u32| rgba(b.data[(y * 40 + x) as usize]);
    for top in [
        -536_870_912 + 5000,
        -536_870_912,
        -536_870_912 - 5000,
        -1_073_741_824,
    ] {
        let b = draw(&format!(
            "<path d='M10 {top} L30 {top} L30 9100 L10 9100Z' fill='#00f'/><rect x='35' y='8500' width='4' height='4' fill='#0c0'/>"
        ));
        assert_eq!(at(&b, 38, 8502), [0, 0xCC, 0, 255], "{top}");
        assert_eq!(at(&b, 20, 8800), [0, 0, 0xFF, 255], "{top}");
    }
    let b = draw("<path d='M10 -50 L30 -50 L30 1.1e9 L10 1.1e9Z' fill='#00f'/>");
    assert_eq!(at(&b, 20, 8800), [0, 0, 0xFF, 255]);
    // A cubic far out, which the cubic clipper of tiny-skia unwraps on at about 2²¹.
    draw("<path d='M20 4000 C-2e6 -1.7e6 -2e6 -1.7e6 1.7e6 9e5 L20 8000 Z' fill='#00f'/>");
}

/// A path that is clipped keeps its paint where it was: the shader of a gradient or a pattern
/// goes through the path's transform as it does for a path that isn't (`geom::Drawing::Device`).
/// A rectangle from far out covers the canvas as a near one does, pixel for pixel.
#[test]
fn a_clipped_path_keeps_its_gradient_and_pattern_in_place() {
    let draw = |body: &str| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='60' height='40'><defs><linearGradient id='l' gradientUnits='userSpaceOnUse' x1='5' y1='0' x2='55' y2='40'><stop offset='0' stop-color='#f00'/><stop offset='1' stop-color='#00f'/></linearGradient><radialGradient id='r' gradientUnits='userSpaceOnUse' cx='30' cy='20' r='25'><stop offset='0' stop-color='#ff0'/><stop offset='1' stop-color='#060'/></radialGradient><pattern id='p' width='8' height='8' patternUnits='userSpaceOnUse'><rect width='4' height='4' fill='#0c0'/><rect x='4' y='4' width='4' height='4' fill='#c0c'/></pattern></defs>{body}</svg>"
        );
        decode(svg.as_bytes(), 60, 40).unwrap().data
    };
    for (paint, transform) in [
        ("url(#l)", ""),
        ("url(#r)", ""),
        ("url(#p)", ""),
        ("url(#l)", " transform='translate(3 2) scale(1.5)'"),
        ("url(#p)", " transform='rotate(20 30 20)'"),
    ] {
        let near = draw(&format!(
            "<rect x='-200' y='-200' width='500' height='500' fill='{paint}'{transform}/>"
        ));
        let far = draw(&format!(
            "<rect x='-1e9' y='-1e9' width='2e9' height='2e9' fill='{paint}'{transform}/>"
        ));
        assert_eq!(near, far, "{paint}{transform}");
        // The stroke of a far path: wide enough to cover the canvas, painted alike.
        let near = draw(&format!(
            "<path d='M-300 20 L300 20' stroke='{paint}' stroke-width='400'{transform}/>"
        ));
        let far = draw(&format!(
            "<path d='M-1e9 20 L1e9 20' stroke='{paint}' stroke-width='400'{transform}/>"
        ));
        assert_eq!(near, far, "stroke {paint}{transform}");
    }
}

/// A raster image or a mask region far from the canvas (`geom::fill` in resvg's `image.rs` and
/// `mask.rs`) is clipped to it as a path is: each is a rectangle through a transform, which
/// tiny-skia scans as it scans a path, and the scan panics on the far part of a rotated one.
#[test]
fn an_image_or_a_mask_far_from_the_canvas_is_clipped_and_drawn() {
    let alpha = |body: &str, x: u32, y: u32| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>{body}</svg>"
        );
        rgba(decode(svg.as_bytes(), 100, 100).unwrap().data[(y * 100 + x) as usize])[3]
    };
    // A strip of image 1.86e9 wide (10 to 60 down), turned about (50, 50): (50, 50) is on it and
    // (50, 95) is off it. The one of a fuzz-like finding that panicked in `alpha_runs.rs`.
    let image = "<image x='10' y='10' width='1855850259.2205846' height='50' preserveAspectRatio='none' transform='rotate(-18.1705 50 50)' href='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAEklEQVR4nGP4z8DwHx9mGBkKAMLXf4EvceABAAAAAElFTkSuQmCC'/>";
    assert_eq!(alpha(image, 50, 50), 255);
    assert_eq!(alpha(image, 50, 95), 0);
    // The same as the region of a mask on a rotated rectangle, and on a rotated group (a mask
    // 4.4e8 tall, whose far edge was a panic, and one 1.86e9 wide).
    for (mask, inside, outside, rotated) in [
        (
            "x='10' y='10' width='1855850259.2205846' height='50'",
            (50, 50),
            (50, 95),
            "<rect width='100' height='100' fill='blue' mask='url(#m)' transform='rotate(-18.1705 50 50)'/>",
        ),
        (
            "x='0' y='10' width='50' height='440548105.6754508'",
            (30, 50),
            (75, 50),
            "<g transform='rotate(6.49897 50 50)'><rect x='-50' y='-50' width='300' height='300' fill='blue' mask='url(#m)'/></g>",
        ),
    ] {
        let body = format!(
            "<mask id='m' maskUnits='userSpaceOnUse' {mask}><rect x='-50' y='-50' width='300' height='300' fill='white'/></mask>{rotated}"
        );
        assert_eq!(alpha(&body, inside.0, inside.1), 255, "{mask}");
        assert_eq!(alpha(&body, outside.0, outside.1), 0, "{mask}");
    }
}

/// The clipper's crossings of a far edge with a side of the window are exact enough that a
/// thin wedge with a vertex out on a diagonal covers what it covers wherever the vertex is: the
/// crossing is worked out from the end of the edge nearer the side (from the far end, past 1e17
/// px, it lost its last digits and the wedge came out a 30 px wide triangle). Wedge
/// `M40 60 L(40+F) (60−1.7F) L44 61 Z` covers the same 285 pixels (278 without anti-aliasing)
/// for any F; and a path of zero area, an even-odd fill with a vertex at −2.8e24 under a `matrix`, draws
/// nothing (by exact arithmetic, none of its pixels is inside).
#[test]
fn a_thin_wedge_with_a_vertex_far_out_on_a_diagonal_is_clipped_exactly() {
    let wedge = |far: f64, mode: &str| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path d='M40 60 L{:e} {:e} L44 61 Z' fill='blue' shape-rendering='{mode}'/></svg>",
            40.0 + far,
            60.0 - 1.7 * far
        );
        covered(&svg, (100, 100)).iter().filter(|c| **c).count()
    };
    for (mode, count) in [("auto", 285), ("crispEdges", 278)] {
        assert_eq!(wedge(1e12, mode), count, "{mode}");
        for far in [1e17, 1e18, 1e19, 1e20, 1e30] {
            let n = wedge(far, mode);
            assert!(n.abs_diff(count) <= 2, "{mode} {far:e}: {n} of {count}");
        }
    }
    let zero = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fuzz/seeds/svg/even-odd-vertex-far-left-under-a-matrix.svg"
    ))
    .unwrap();
    assert!(
        decode(&zero, 100, 100)
            .unwrap()
            .data
            .iter()
            .all(|p| *p == 0)
    );
}

/// A subpath of a path that is clipped is drawn whatever else the path holds: the clipper's work
/// on one subpath doesn't depend on another's. Four thousand loops of curves below the canvas,
/// past its edge but inside the margin the clipper keeps, and a triangle far out (which makes the
/// path too far for tiny-skia) once used up a budget the whole path shared, and the visible curves
/// after them were cut to chords: the shape was gone. Now the loops (which don't reach the canvas)
/// cost nothing, and the subpath is the one drawn without the rest of the path (to within what
/// the clipper's finer flattening changes at its edge, which anti-aliasing or not is some pixels).
#[test]
fn a_visible_subpath_is_drawn_whatever_else_its_path_holds() {
    let eyelet =
        "M-1000 -2000 C-1000 -600 -3000 -600 -3000 -2000 C-3000 -3400 -1000 -3400 -1000 -2000 Z ";
    let shape = "M20 50 C20 10 80 10 80 50 C80 90 20 90 20 50 Z ";
    let far = "M300000 300000 L300010 300000 L300000 300010 Z";
    for mode in ["auto", "crispEdges"] {
        let drawn = |d: &str| {
            let svg = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path fill='blue' shape-rendering='{mode}' d='{d}'/></svg>"
            );
            covered(&svg, (100, 100)).iter().filter(|c| **c).count()
        };
        let alone = drawn(shape);
        assert!((2800..3000).contains(&alone), "{mode}: {alone}");
        for n in [1, 100, 4000] {
            let d = format!("{}{shape}{far}", eyelet.repeat(n));
            let count = drawn(&d);
            assert!(
                count.abs_diff(alone) <= 30,
                "{mode} {n} loops: {count} of {alone}"
            );
        }
        // A subpath of curves that crosses the canvas, with a far one in the path: the same.
        let wide = "M-30 50 C-30 -200 130 -200 130 50 C130 300 -30 300 -30 50 Z ";
        assert!(
            drawn(&format!("{wide}{far}")).abs_diff(drawn(wide)) <= 40,
            "{mode}"
        );
    }
}

/// An edge between two points far out on opposite sides of the canvas crosses a side of the
/// clipper's window where the line is, though both its ends are 1e20 and more from it: the
/// crossing is worked out without rounding (`geom::cross`: its terms are as large as the ends'
/// distance from the side, and an `f64` rounds them far over the pixels). Triangles with two
/// vertices on a line through the corner of the canvas (`-15 -17` and `15 17`, times powers of
/// two, which an `f32` holds exactly) and a third far out cover the same pixels at any distance
/// of the vertices: the half the canvas the line divides. At 1e20 the diagonal `y = x` came out as
/// a canvas of one colour.
#[test]
fn an_edge_with_both_ends_far_out_is_cut_where_its_line_is() {
    let covers = |d: &str, mode: &str| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><path d='{d}' fill='red' shape-rendering='{mode}'/></svg>"
        );
        covered(&svg, (64, 64))
    };
    for mode in ["auto", "crispEdges"] {
        let diagonal = |far: &str| {
            covers(
                &format!("M-{far} -{far} L{far} {far} L{far} -{far} Z"),
                mode,
            )
        };
        let reference = diagonal("10000");
        let half = reference.iter().filter(|c| **c).count();
        assert!((1900..2200).contains(&half), "{mode}: {half}");
        for far in ["1e12", "1e20", "1e25", "1e30", "1e35", "1e38"] {
            assert!(diagonal(far) == reference, "{mode} {far}");
        }
        // Not on the diagonal: the line `17 x = 15 y`, the third vertex out at the same scale.
        let at = |a: f64, b: f64, c: f64| {
            let p = |e: f64, k: f64| format!("{:e} {:e}", 15.0 * k * e.exp2(), 17.0 * k * e.exp2());
            covers(
                &format!(
                    "M{} L{} L{:e} {:e} Z",
                    p(a, -1.0),
                    p(b, 1.0),
                    3.0 * c.exp2(),
                    -5.0 * c.exp2()
                ),
                mode,
            )
        };
        let reference = at(30.0, 31.0, 30.0);
        assert!(
            reference.iter().any(|c| *c) && !reference.iter().all(|c| *c),
            "{mode}"
        );
        for (a, b, c) in [
            (60.0, 78.0, 40.0),
            (100.0, 90.0, 105.0),
            (110.0, 120.0, 60.0),
            (118.0, 100.0, 70.0),
        ] {
            assert!(at(a, b, c) == reference, "{mode} {a} {b} {c}");
        }
    }
}

/// A path is stroked as it was, less the subpaths that have a curve too big for tiny-skia's
/// stroker (`usvg::without_big_curves`): one that spans over 1e18 on both axes panics it
/// (`find_quad_max_curvature`), and none that is long on one axis only does.
#[test]
fn a_big_curve_costs_a_stroke_its_own_subpath_only() {
    let draw = |body: &str| {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>{body}</svg>"
        );
        decode(svg.as_bytes(), 100, 100).unwrap().data
    };
    let at = |d: &[u32], x: u32, y: u32| d[(y * 100 + x) as usize];
    let line = "M20 50.5 L80.5 50.5 L80.5 70";
    for (curve, long_on_both) in [
        (
            "M-2.81924 33.6695 Q2.24328e+32 2.84844e+31 -5.4753 1.17487e+32",
            true,
        ),
        ("M10 10 C1e20 1e20 -1e20 3e20 90 90", true),
        ("M10 10 C1e19 1e19 -1e19 3e19 90 90", true),
        ("M10 10 Q50 -1.6e20 90 10", false),
        ("M10 10 C1e20 5 -1e20 8 90 90", false),
    ] {
        for (width, extra) in [
            ("3", ""),
            ("3", " shape-rendering='crispEdges'"),
            ("0.4", " shape-rendering='crispEdges'"),
        ] {
            let d = draw(&format!(
                "<path d='{curve} {line}' fill='none' stroke='#345' stroke-width='{width}'{extra}/>"
            ));
            // Nothing is panicked at, and the polyline's corner is where it was.
            assert_ne!(at(&d, 50, 50), 0, "{curve} {width}{extra}");
            assert_ne!(at(&d, 80, 60), 0, "{curve} {width}{extra}");
            let alone = draw(&format!(
                "<path d='{curve}' fill='none' stroke='#345' stroke-width='{width}'{extra}/>"
            ));
            // The long curve itself: drawn when it is long on one axis only, as it ever was.
            if long_on_both {
                assert!(alone.iter().all(|p| *p == 0), "{curve} {width}{extra}");
            } else if extra.is_empty() {
                assert!(alone.iter().any(|p| *p != 0), "{curve} {width}{extra}");
            }
        }
    }
    // A curve far on one axis only is stroked wide across the canvas and covers it, as before:
    // a 1.4e32 spike over a width of 440.
    let spike = draw(
        "<path d='M-19.8772 24.0887 Q87.6089 1.43972e+32 -14.3931 27.3979 C118.358 10.0395 12.2756 60.4416 97.1587 109.272 M88.5451 79.1311' fill='none' fill-rule='evenodd' stroke='#345' stroke-width='439.446' stroke-linejoin='miter-clip' stroke-linecap='square' transform='rotate(-66.8239)'/>",
    );
    assert_eq!(rgba(at(&spike, 50, 50)), [0x33, 0x44, 0x55, 255]);
    // And a cubic subpath beside one long on one axis keeps its stroke (a 9000 by 40 canvas).
    let svg = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fuzz/seeds/svg/stroke-quad-long-on-one-axis.svg"
    ))
    .unwrap();
    let b = decode(&svg, 9000, 40).unwrap();
    assert_eq!(b.data.iter().filter(|p| **p != 0).count(), 459);
}
