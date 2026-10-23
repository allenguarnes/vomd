use std::borrow::Cow;

use super::super::{NO_CANCEL, decode as decode_any, play_frames};
use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

const RED: u32 = 0xFFFF_0000;
const GREEN: u32 = 0xFF00_FF00;
const BLUE: u32 = 0xFF00_00FF;
const WHITE: u32 = 0xFFFF_FFFF;
const CLEAR: u32 = 0;

/// The global palette of the GIFs here: red, green, blue, white (indices 0 to 3).
const PALETTE: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];

/// A frame of `rect` all of palette index `color`.
fn solid(rect: [u16; 4], color: u8, dispose: gif::DisposalMethod) -> gif::Frame<'static> {
    let [left, top, width, height] = rect;
    gif::Frame {
        left,
        top,
        width,
        height,
        dispose,
        delay: 5,
        buffer: Cow::Owned(vec![color; usize::from(width) * usize::from(height)]),
        ..gif::Frame::default()
    }
}

/// A GIF of `frames` on a screen of `w` × `h` with [`PALETTE`], played `repeat` (none: no loop
/// extension).
fn gif_of(w: u16, h: u16, repeat: Option<gif::Repeat>, frames: &[gif::Frame]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = gif::Encoder::new(&mut out, w, h, &PALETTE).unwrap();
        if let Some(r) = repeat {
            e.set_repeat(r).unwrap();
        }
        for f in frames {
            e.write_frame(f).unwrap();
        }
    }
    out
}

/// What playing `bytes` for `n` frames at most draws: each frame's number, canvas and dirty
/// rectangle.
fn play(bytes: &[u8], n: usize) -> Vec<(usize, Vec<u32>, [u32; 4])> {
    let mut out = Vec::new();
    play_frames(bytes, n, usize::MAX, |i, px, dirty| {
        out.push((i, px.to_vec(), dirty))
    })
    .expect("an animation");
    out
}

/// A canvas of `w` × `h`, each pixel what `f` says.
fn canvas(w: u32, h: u32, f: impl Fn(u32, u32) -> u32) -> Vec<u32> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| f(x, y))
        .collect()
}

fn inside(x: u32, y: u32, [l, t, w, h]: [u32; 4]) -> bool {
    (l..l + w).contains(&x) && (t..t + h).contains(&y)
}

#[test]
fn an_entry_is_24_bytes() {
    assert_eq!(size_of::<Entry>(), 24);
}

#[test]
fn short_delays_play_as_browsers_play_them() {
    for (ms, want) in [
        (0, 100),
        (10, 100),
        (11, 11),
        (20, 20),
        (100, 100),
        (5000, 5000),
    ] {
        assert_eq!(delay(ms), Duration::from_millis(want), "{ms} ms");
    }
    // A GIF's delay is in hundredths: 1 plays as 100 ms, 2 as 20.
    let mut frames = [
        solid([0, 0, 2, 2], 0, gif::DisposalMethod::Keep),
        solid([0, 0, 2, 2], 1, gif::DisposalMethod::Keep),
        solid([0, 0, 2, 2], 2, gif::DisposalMethod::Keep),
    ];
    for (f, d) in frames.iter_mut().zip([1, 2, 0]) {
        f.delay = d;
    }
    let mut index = Index::new(&gif_of(2, 2, None, &frames)).unwrap();
    let bytes = gif_of(2, 2, None, &frames);
    let mut c = Canvas::new(&index);
    let delays: Vec<_> = (0..3)
        .map(|_| match c.step(&mut index, &bytes, &NO_CANCEL) {
            Advance::Drawn { delay, .. } => delay.as_millis(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(delays, [100, 20, 100]);
}

/// Disposal 1 (keep), 2 (background: transparent) and 3 (previous), and a transparent index,
/// on 8 × 8 frames worked out by hand; then the second play from a clear canvas, and the end.
#[test]
fn gif_frames_are_disposed_as_browsers_do() {
    use gif::DisposalMethod::{Background, Keep, Previous};
    let mut last = solid([4, 4, 4, 4], 3, Keep);
    last.transparent = Some(0);
    // Its left half is index 0, transparent: what is under it stays.
    last.buffer = Cow::Owned((0..16).map(|i| if i % 4 < 2 { 0 } else { 3 }).collect());
    let bytes = gif_of(
        8,
        8,
        Some(gif::Repeat::Finite(1)),
        &[
            solid([0, 0, 8, 8], 0, Keep),
            solid([2, 2, 4, 4], 1, Background),
            solid([0, 0, 2, 2], 2, Previous),
            last,
        ],
    );
    let green = [2, 2, 4, 4];
    let blue = [0, 0, 2, 2];
    let want = [
        (canvas(8, 8, |_, _| RED), [0, 0, 8, 8]),
        (
            canvas(8, 8, |x, y| if inside(x, y, green) { GREEN } else { RED }),
            green,
        ),
        // The green square cleared, the blue one drawn.
        (
            canvas(8, 8, |x, y| match () {
                _ if inside(x, y, blue) => BLUE,
                _ if inside(x, y, green) => CLEAR,
                _ => RED,
            }),
            [0, 0, 6, 6],
        ),
        // The blue square put back as it was (red), the white one drawn over its transparent
        // half.
        (
            canvas(8, 8, |x, y| match () {
                _ if x >= 6 && y >= 4 => WHITE,
                _ if inside(x, y, green) => CLEAR,
                _ => RED,
            }),
            [0, 0, 8, 8],
        ),
    ];
    let drawn = play(&bytes, 100);
    assert_eq!(drawn.len(), 8, "two plays (one repetition), then the end");
    for (k, (i, px, dirty)) in drawn.iter().enumerate() {
        assert_eq!(*i, k % 4);
        assert_eq!(px, &want[k % 4].0, "frame {k}");
        // The second play starts from a clear canvas: all of it is new.
        assert_eq!(*dirty, want[k % 4].1, "frame {k}");
    }
}

#[test]
fn previous_on_the_first_frame_clears_like_background() {
    use gif::DisposalMethod::{Keep, Previous};
    let bytes = gif_of(
        8,
        8,
        None,
        &[
            solid([0, 0, 4, 4], 0, Previous),
            solid([4, 4, 4, 4], 1, Keep),
        ],
    );
    let drawn = play(&bytes, 10);
    assert_eq!(drawn.len(), 2, "played once");
    let want = canvas(8, 8, |x, y| if x >= 4 && y >= 4 { GREEN } else { CLEAR });
    assert_eq!(drawn[1].1, want);
    assert_eq!(drawn[1].2, [0, 0, 8, 8]);
}

/// A local palette is the frame's own; an index past it leaves the canvas as it is, as the
/// transparent one does.
#[test]
fn a_local_palette_is_the_frames_own() {
    use gif::DisposalMethod::Keep;
    let mut own = solid([0, 0, 4, 1], 0, Keep);
    // Two colors, black and gray; indices 0, 1, then 2 and 3, past it.
    own.palette = Some(vec![0, 0, 0, 128, 128, 128]);
    own.buffer = Cow::Owned(vec![0, 1, 2, 3]);
    let bytes = gif_of(4, 1, None, &[solid([0, 0, 4, 1], 3, Keep), own]);
    let drawn = play(&bytes, 2);
    assert_eq!(drawn[1].1, [0xFF00_0000, 0xFF80_8080, WHITE, WHITE]);
}

/// A frame reaching past the screen is clipped to it (the screen is made big enough for the first
/// frame only, as browsers do).
#[test]
fn a_frame_past_the_screen_is_clipped() {
    use gif::DisposalMethod::Keep;
    let bytes = gif_of(
        8,
        8,
        None,
        &[solid([0, 0, 8, 8], 0, Keep), solid([6, 6, 4, 4], 1, Keep)],
    );
    let drawn = play(&bytes, 2);
    assert_eq!(
        drawn[1].1,
        canvas(8, 8, |x, y| if x >= 6 && y >= 6 { GREEN } else { RED })
    );
    assert_eq!(drawn[1].2, [6, 6, 2, 2]);
    // All of it past the screen: nothing changes.
    let bytes = gif_of(
        8,
        8,
        None,
        &[solid([0, 0, 8, 8], 0, Keep), solid([9, 1, 4, 4], 1, Keep)],
    );
    let drawn = play(&bytes, 2);
    assert_eq!(drawn[1].1, canvas(8, 8, |_, _| RED));
    assert_eq!(drawn[1].2[2] * drawn[1].2[3], 0);
}

#[test]
fn an_interlaced_frame_is_put_in_order() {
    use gif::DisposalMethod::Keep;
    // Rows 0 to 7 of colors 0, 1, 2, 3, 0, 1, 2, 3, given in the four passes' order.
    let order = [0, 4, 2, 6, 1, 3, 5, 7];
    let mut f = solid([0, 0, 2, 8], 0, Keep);
    f.interlaced = true;
    f.buffer = Cow::Owned(order.iter().flat_map(|&y| [y as u8 % 4; 2]).collect());
    let bytes = gif_of(2, 8, None, &[solid([0, 0, 2, 8], 3, Keep), f]);
    let drawn = play(&bytes, 2);
    let colors = [RED, GREEN, BLUE, WHITE];
    assert_eq!(drawn[1].1, canvas(2, 8, |_, y| colors[y as usize % 4]));
}

#[test]
fn loop_counts_are_read_as_browsers_read_them() {
    use gif::DisposalMethod::Keep;
    let frames = [solid([0, 0, 1, 1], 0, Keep), solid([0, 0, 1, 1], 1, Keep)];
    for (repeat, loops, drawn) in [
        (None, Loops::Plays(1), 2),
        // Repetitions after the first play.
        (Some(gif::Repeat::Finite(2)), Loops::Plays(3), 6),
        (Some(gif::Repeat::Infinite), Loops::Forever, 50),
    ] {
        let bytes = gif_of(1, 1, repeat, &frames);
        assert_eq!(Index::new(&bytes).unwrap().loops, loops, "{repeat:?}");
        assert_eq!(play(&bytes, 50).len(), drawn, "{repeat:?}");
        assert_eq!(
            detect(&bytes),
            Some(AnimInfo {
                size: (1, 1),
                loops
            })
        );
    }
    // A WebP's count is of plays.
    let webp = webp_anim(1, 1, 3, &vec![anmf([0, 0, 1, 1], 0, 2, &red(1, 1)); 2]);
    assert_eq!(Index::new(&webp).unwrap().loops, Loops::Plays(3));
    assert_eq!(play(&webp, 50).len(), 6);
    let webp = webp_anim(1, 1, 0, &vec![anmf([0, 0, 1, 1], 0, 2, &red(1, 1)); 2]);
    assert_eq!(Index::new(&webp).unwrap().loops, Loops::Forever);
}

/// A frame cut short is left out of the index, and one that doesn't decode ends the animation
/// at the one before: what decoded loops.
#[test]
fn a_broken_frame_ends_the_animation_at_the_one_before() {
    use gif::DisposalMethod::Keep;
    let frames = [
        solid([0, 0, 4, 4], 0, Keep),
        solid([0, 0, 4, 4], 1, Keep),
        solid([0, 0, 4, 4], 2, Keep),
    ];
    let bytes = gif_of(4, 4, Some(gif::Repeat::Infinite), &frames);
    let index = Index::new(&bytes).unwrap();
    assert_eq!(index.frames.len(), 3);
    let third = index.frames[2];
    // Cut in the third frame's data: two frames.
    let cut = &bytes[..third.at as usize + third.len as usize - 2];
    assert_eq!(Index::new(cut).unwrap().frames.len(), 2);
    let order: Vec<_> = play(cut, 6).iter().map(|d| d.0).collect();
    assert_eq!(order, [0, 1, 0, 1, 0, 1]);
    // Garbage for the second frame's data: it is found when it is decoded.
    let second = index.frames[1];
    let mut junk = bytes.clone();
    // (After the descriptor, the code size and the first sub-block's length: its codes.)
    let data = second.at as usize + 12;
    assert!(junk[data - 1] >= 2);
    junk[data..data + 2].copy_from_slice(&[0xFF, 0xFF]);
    let mut index = Index::new(&junk).unwrap();
    assert_eq!(index.frames.len(), 3);
    let mut c = Canvas::new(&index);
    let mut order = Vec::new();
    for _ in 0..4 {
        match c.step(&mut index, &junk, &NO_CANCEL) {
            Advance::Drawn { frame, .. } => order.push(frame),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(order, [0, 0, 0, 0], "only the first frame decodes");
    assert_eq!(index.frames.len(), 1);
    assert_eq!(c.px, canvas(4, 4, |_, _| RED));
}

#[test]
fn a_cancelled_frame_leaves_the_canvas_as_it_was() {
    use gif::DisposalMethod::Keep;
    let bytes = gif_of(
        4,
        4,
        None,
        &[solid([0, 0, 4, 4], 0, Keep), solid([0, 0, 4, 4], 1, Keep)],
    );
    let mut index = Index::new(&bytes).unwrap();
    let mut c = Canvas::new(&index);
    assert!(matches!(
        c.step(&mut index, &bytes, &NO_CANCEL),
        Advance::Drawn { frame: 0, .. }
    ));
    let stop = AtomicBool::new(true);
    assert_eq!(c.step(&mut index, &bytes, &stop), Advance::Cancelled);
    assert_eq!(c.px, canvas(4, 4, |_, _| RED));
    assert_eq!(index.frames.len(), 2, "not cut for it");
    assert!(matches!(
        c.step(&mut index, &bytes, &NO_CANCEL),
        Advance::Drawn { frame: 1, .. }
    ));
    assert_eq!(c.step(&mut index, &bytes, &NO_CANCEL), Advance::Over);
}

#[test]
fn frames_past_the_most_are_left_out() {
    use gif::DisposalMethod::Keep;
    let frames: Vec<_> = (0..MAX_FRAMES + 5)
        .map(|i| solid([0, 0, 1, 1], (i % 4) as u8, Keep))
        .collect();
    let index = Index::new(&gif_of(1, 1, None, &frames)).unwrap();
    assert_eq!(index.frames.len(), MAX_FRAMES);
    assert_eq!(index.cost(), MAX_FRAMES * 24, "allocated at its number");
}

/// The first frame drawn is the still the decode worker shows, pixel for pixel.
#[test]
fn the_first_frame_is_the_still() {
    for name in ["anim.gif", "anim.webp", "big-anim.webp"] {
        let bytes = fixture(name);
        let drawn = play(&bytes, 1);
        let index = Index::new(&bytes).unwrap();
        let (w, h) = index.size;
        let still = decode_any(&bytes, w, h).unwrap();
        assert_eq!(drawn[0].1, still.data, "{name}");
    }
}

#[test]
fn animations_are_detected_by_their_second_frame() {
    assert_eq!(
        detect(&fixture("anim.gif")),
        Some(AnimInfo {
            size: (6, 4),
            loops: Loops::Forever
        })
    );
    assert_eq!(
        detect(&fixture("anim.webp")),
        Some(AnimInfo {
            size: (6, 4),
            loops: Loops::Forever
        })
    );
    // One frame, or not an animation's format.
    for name in [
        "offset.gif",
        "zero-screen.gif",
        "lossy.webp",
        "alpha.png",
        "gray.jpg",
    ] {
        assert_eq!(detect(&fixture(name)), None, "{name}");
        assert!(Index::new(&fixture(name)).is_none(), "{name}");
    }
    let one = webp_anim(2, 2, 0, &[anmf([0, 0, 2, 2], 0, 2, &red(2, 2))]);
    assert_eq!(detect(&one), None);
}

/// The detection walks the headers of a 2 MB GIF of 400 frames to its second frame: about a
/// microsecond (the index walks all of them, about 0.1 ms).
#[test]
fn detection_reads_headers_only() {
    use gif::DisposalMethod::Keep;
    // Noise, so the data doesn't compress: about 12 KB a frame.
    let mut seed = 1u32;
    let frames: Vec<_> = (0..400)
        .map(|_| {
            let mut f = solid([0, 0, 128, 128], 0, Keep);
            f.buffer = Cow::Owned(
                (0..128 * 128)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        (seed >> 16) as u8 & 3
                    })
                    .collect(),
            );
            f
        })
        .collect();
    let bytes = gif_of(128, 128, None, &frames);
    assert!(bytes.len() > 1 << 20, "{}", bytes.len());
    let start = std::time::Instant::now();
    for _ in 0..10 {
        assert!(detect(&bytes).is_some());
    }
    eprintln!(
        "detect: {:?} each, {} bytes",
        start.elapsed() / 10,
        bytes.len()
    );
    // Walking every frame's sub-blocks (the index): a few ms at most, even unoptimized.
    let start = std::time::Instant::now();
    assert_eq!(Index::new(&bytes).unwrap().frames.len(), 400);
    eprintln!("index: {:?}", start.elapsed());
}

// ---- WebP ---------------------------------------------------------------------------------

/// The `VP8L` chunk (header included) of a lossless still of `w` × `h` RGBA `pixels`.
fn vp8l(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(pixels, w, h, image_webp::ColorType::Rgba8)
        .unwrap();
    assert_eq!(&out[12..16], b"VP8L");
    out[12..].to_vec()
}

fn red(w: u32, h: u32) -> Vec<u8> {
    vp8l(w, h, &[255, 0, 0, 255].repeat((w * h) as usize))
}

/// An `ANMF` chunk at `rect` (its left and top even) of `delay` ms, `flags` (2: no blend, 1:
/// dispose), holding `chunks`.
fn anmf(rect: [u32; 4], delay: u32, flags: u8, chunks: &[u8]) -> Vec<u8> {
    let [x, y, w, h] = rect;
    let mut d = Vec::new();
    for v in [x / 2, y / 2, w - 1, h - 1, delay] {
        d.extend(&v.to_le_bytes()[..3]);
    }
    d.push(flags);
    d.extend(chunks);
    let mut out = b"ANMF".to_vec();
    out.extend((d.len() as u32).to_le_bytes());
    out.extend(&d);
    if d.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// An animated WebP of a `w` × `h` canvas, `loops` (0: forever) and the frames `anmfs`.
fn webp_anim(w: u32, h: u32, loops: u16, anmfs: &[Vec<u8>]) -> Vec<u8> {
    let mut body = b"WEBPVP8X".to_vec();
    body.extend(10u32.to_le_bytes());
    body.extend([0x12, 0, 0, 0]);
    body.extend(&(w - 1).to_le_bytes()[..3]);
    body.extend(&(h - 1).to_le_bytes()[..3]);
    body.extend(b"ANIM");
    body.extend(6u32.to_le_bytes());
    body.extend([0, 0, 0, 0]);
    body.extend(loops.to_le_bytes());
    for f in anmfs {
        body.extend(f);
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// Blending (source over, premultiplied) and not, and disposal to transparent, on 8 × 8 frames
/// worked out by hand.
#[test]
fn webp_frames_blend_and_dispose_by_the_container_rules() {
    let half_blue = vp8l(4, 4, &[0, 0, 255, 128].repeat(16));
    let half_green = vp8l(4, 4, &[0, 255, 0, 128].repeat(16));
    let bytes = webp_anim(
        8,
        8,
        1,
        &[
            anmf([0, 0, 8, 8], 100, 2, &red(8, 8)),
            // Blended, then disposed.
            anmf([2, 2, 4, 4], 100, 1, &half_blue),
            // Not blended: it replaces what is there, its alpha too.
            anmf([0, 0, 4, 4], 100, 2, &half_green),
        ],
    );
    let drawn = play(&bytes, 10);
    assert_eq!(drawn.len(), 3, "played once");
    assert_eq!(drawn[0].1, canvas(8, 8, |_, _| RED));
    // Blue at 128 (premultiplied 128) over red: alpha 128 + 255·127/255, red 255·127/255.
    let over = 0xFF7F_0080;
    assert_eq!(over, super::over(premultiplied(0, 0, 255, 128), RED, 128));
    assert_eq!(
        drawn[1].1,
        canvas(8, 8, |x, y| if inside(x, y, [2, 2, 4, 4]) {
            over
        } else {
            RED
        })
    );
    assert_eq!(drawn[1].2, [2, 2, 4, 4]);
    let green = premultiplied(0, 255, 0, 128);
    assert_eq!(green, 0x8000_8000);
    let want = canvas(8, 8, |x, y| match () {
        _ if inside(x, y, [0, 0, 4, 4]) => green,
        _ if inside(x, y, [2, 2, 4, 4]) => CLEAR,
        _ => RED,
    });
    assert_eq!(drawn[2].1, want);
    assert_eq!(drawn[2].2, [0, 0, 6, 6]);
}

/// A lossy frame without alpha is opaque, one with an `ALPH` chunk has its alpha: the frames of
/// a file `libwebp` made.
#[test]
fn lossy_frames_decode_with_and_without_alpha() {
    let bytes = fixture("big-anim.webp");
    let index = Index::new(&bytes).unwrap();
    assert_eq!(index.size, (1024, 768));
    let drawn = play(&bytes, 2);
    assert!(
        drawn[0].1.iter().any(|&p| p >> 24 != 0xFF),
        "the first has alpha"
    );
    assert!(
        drawn[1].1.iter().all(|&p| p >> 24 == 0xFF),
        "the second covers it, opaque"
    );
}

/// A frame whose image is another size than the frame says doesn't decode (the patched
/// decoder's check holds for every frame), and one reaching past the canvas ends the walk.
#[test]
fn a_webp_frame_of_another_size_is_refused() {
    let wrong = webp_anim(
        4,
        4,
        0,
        &[
            anmf([0, 0, 4, 4], 100, 2, &red(4, 4)),
            anmf([0, 0, 4, 4], 100, 2, &red(3, 3)),
        ],
    );
    let order: Vec<_> = play(&wrong, 3).iter().map(|d| d.0).collect();
    assert_eq!(order, [0, 0, 0]);
    let past = webp_anim(
        4,
        4,
        0,
        &[
            anmf([0, 0, 4, 4], 100, 2, &red(4, 4)),
            anmf([0, 0, 4, 4], 100, 2, &red(4, 4)),
            anmf([2, 2, 4, 4], 100, 2, &red(4, 4)),
        ],
    );
    assert_eq!(Index::new(&past).unwrap().frames.len(), 2);
    // A frame of the lossy file's, said to be a size other than its image's.
    let big = fixture("big-anim.webp");
    let index = Index::new(&big).unwrap();
    let e = index.frames[1];
    let chunks = &big[e.at as usize..(e.at + e.len) as usize];
    let lie = webp_anim(
        1024,
        768,
        0,
        &[
            anmf([0, 0, 1024, 768], 100, 2, chunks),
            anmf([0, 0, 1022, 768], 100, 2, chunks),
        ],
    );
    let order: Vec<_> = play(&lie, 2).iter().map(|d| d.0).collect();
    assert_eq!(order, [0, 0]);
}

// ---- What it holds ------------------------------------------------------------------------

/// An index of `n` frames of `w` × `h` on a canvas of the same size.
fn index_of(n: usize, w: u16, h: u16) -> Index {
    let e = Entry {
        at: 0,
        len: 0,
        rect: [0, 0, w, h],
        delay: 100,
        dispose: Dispose::Keep,
        blend: false,
        transparent: None,
    };
    Index {
        format: Format::Gif,
        size: (u32::from(w), u32::from(h)),
        loops: Loops::Forever,
        frames: vec![e; n],
        table: 0,
        largest: usize::from(w) * usize::from(h),
        working: super::super::GIF_FIXED,
        previous: 0,
    }
}

#[test]
fn small_animations_are_kept_and_large_ones_streamed() {
    // An emoji-sized spinner of 12 frames: 12 KB kept, far less than a canvas and a decoder.
    let spinner = index_of(12, 16, 16);
    assert!(spinner.keeps((16, 16)));
    // A README demo of 640 × 360 and 60 frames: 55 MB kept.
    let demo = index_of(60, 640, 360);
    assert_eq!(demo.kept((640, 360)), 60 * 640 * 360 * 4 + 60 * 24);
    assert!(!demo.keeps((640, 360)));
    // Under the most kept, though more than streaming holds: kept (no decode after a loop).
    let long = index_of(2000, 8, 8);
    assert!(long.kept((8, 8)) <= KEEP_MAX);
    assert!(long.kept((8, 8)) > long.streamed(0, (8, 8), 2));
    assert!(long.keeps((8, 8)));
    // Just over: streamed.
    let n = KEEP_MAX / (8 * 8 * 4);
    assert!(!index_of(n, 8, 8).keeps((8, 8)));
    // Streaming holds the canvas, the decoder, the resampler and two bitmaps of the box.
    let s = demo.streamed(1000, (320, 180), 2);
    let canvas = 640 * 360 * 4;
    assert!(s >= 1000 + canvas + 640 * 360 + super::super::GIF_FIXED + 2 * 320 * 180 * 4);
    assert_eq!(
        demo.held(),
        demo.cost() + canvas + 640 * 360 + super::super::GIF_FIXED
    );
}

// ---- Playing at a box ---------------------------------------------------------------------

/// A 40 × 30 GIF of `n` frames: small squares of every color that move over a kept field,
/// with every disposal, so most frames change a little of the canvas.
fn sprites(n: usize, repeat: Option<gif::Repeat>) -> Vec<u8> {
    use gif::DisposalMethod::{Background, Keep, Previous};
    let mut frames = vec![solid([0, 0, 40, 30], 3, Keep)];
    for i in 1..n {
        let (x, y) = ((i * 7 % 34) as u16, (i * 5 % 25) as u16);
        let dispose = [Keep, Background, Previous][i % 3];
        frames.push(solid([x, y, 6, 5], (i % 3) as u8, dispose));
    }
    gif_of(40, 30, repeat, &frames)
}

/// A random-ish premultiplied pixel from a counter.
fn noise(i: &mut u32) -> u32 {
    *i = i.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let a = *i >> 24;
    let c = |s: u32| ((*i >> s) & 0xFF) * a / 255;
    a << 24 | c(16) << 16 | c(8) << 8 | c(0)
}

#[test]
fn a_dirty_rectangle_resamples_like_the_whole_canvas() {
    let mut seed = 7;
    for (src, dst) in [
        ((40, 30), (17, 13)),
        ((40, 30), (40, 30)),
        ((40, 30), (61, 47)),
        ((40, 30), (40, 47)),
        ((13, 50), (5, 9)),
        ((1, 1), (3, 2)),
    ] {
        let whole = Rect {
            x: 0,
            y: 0,
            w: src.0,
            h: src.1,
        };
        let mut px: Vec<u32> = (0..src.0 * src.1).map(|_| noise(&mut seed)).collect();
        let mut s = Scaler::new(src, dst);
        let mut out = vec![0; dst.0 as usize * dst.1 as usize];
        s.run(&px, whole, &mut out);
        for _ in 0..40 {
            let x = noise(&mut seed) % src.0;
            let y = noise(&mut seed) % src.1;
            let r = Rect {
                x,
                y,
                w: 1 + noise(&mut seed) % (src.0 - x),
                h: 1 + noise(&mut seed) % (src.1 - y),
            };
            for row in y..y + r.h {
                for col in x..x + r.w {
                    px[(row * src.0 + col) as usize] = noise(&mut seed);
                }
            }
            s.run(&px, r, &mut out);
            let mut want = vec![0; out.len()];
            Scaler::new(src, dst).run(&px, whole, &mut want);
            assert_eq!(out, want, "{src:?} to {dst:?}, {r:?}");
        }
    }
}

#[test]
fn whole_pixels_are_copied_as_the_sums_make_them() {
    let mut seed = 11;
    for (src, dst, copy) in [
        ((40, 30), (40, 30), true),
        ((40, 30), (80, 60), true),
        ((13, 7), (52, 28), true),
        ((40, 30), (60, 45), false),
        ((40, 30), (80, 30), true),
        ((40, 30), (20, 15), false),
    ] {
        let px: Vec<u32> = (0..src.0 * src.1).map(|_| noise(&mut seed)).collect();
        let whole = Rect {
            x: 0,
            y: 0,
            w: src.0,
            h: src.1,
        };
        let mut s = Scaler::new(src, dst);
        assert_eq!(s.copy, copy, "{src:?} to {dst:?}");
        let mut out = vec![0; dst.0 as usize * dst.1 as usize];
        s.run(&px, whole, &mut out);
        let mut want = vec![0; out.len()];
        Scaler::with(src, dst, false).run(&px, whole, &mut want);
        if copy {
            assert_eq!(out, want, "{src:?} to {dst:?}");
        }
    }
}

#[test]
fn enlarging_by_a_fraction_blends_within_a_level_of_the_sums() {
    let mut seed = 13;
    for (src, dst, blends) in [
        ((40, 30), (60, 45), true),
        ((800, 60), (1440, 108), true),
        ((40, 30), (41, 30), true),
        ((40, 30), (40, 47), true),
        ((1, 1), (3, 2), false),
        ((40, 30), (80, 60), false),
        ((40, 30), (60, 20), false),
        ((40, 30), (20, 15), false),
    ] {
        let px: Vec<u32> = (0..src.0 * src.1).map(|_| noise(&mut seed)).collect();
        let whole = Rect {
            x: 0,
            y: 0,
            w: src.0,
            h: src.1,
        };
        let mut s = Scaler::new(src, dst);
        assert_eq!(s.blend.is_some(), blends, "{src:?} to {dst:?}");
        let mut out = vec![0; dst.0 as usize * dst.1 as usize];
        s.run(&px, whole, &mut out);
        let mut want = vec![0; out.len()];
        Scaler::with(src, dst, false).run(&px, whole, &mut want);
        for (&o, &w) in out.iter().zip(&want) {
            for shift in [0, 8, 16, 24] {
                let (a, b) = ((o >> shift) & 0xFF, (w >> shift) & 0xFF);
                assert!(a.abs_diff(b) <= 1, "{src:?} to {dst:?}: {o:08x} {w:08x}");
            }
            // Still premultiplied.
            let a = o >> 24;
            assert!([0, 8, 16].iter().all(|s| (o >> s) & 0xFF <= a), "{o:08x}");
        }
    }
}

#[test]
fn frames_made_in_the_one_before_the_last_are_the_frames_made_anew() {
    let bytes = Arc::new(sprites(12, Some(gif::Repeat::Infinite)));
    for size in [(17, 13), (40, 30), (61, 47)] {
        let mut e = Engine::new(bytes.clone(), false, Index::new(&bytes).unwrap());
        let mut fresh = Engine::new(bytes.clone(), false, Index::new(&bytes).unwrap());
        let (mut front, mut spare) = (None, None);
        // Past the first play: its restart dirties all of the canvas.
        for i in 0..30 {
            let Made::Frame { bitmap, made, .. } = e.step(size, spare.take(), &NO_CANCEL) else {
                panic!("frame {i}");
            };
            let Made::Frame { bitmap: want, .. } = fresh.step(size, None, &NO_CANCEL) else {
                panic!("frame {i}");
            };
            assert_eq!(bitmap.data, want.data, "{size:?}, frame {i}");
            spare = front.replace((bitmap, made));
        }
    }
}

#[test]
fn the_first_frame_is_the_still_at_the_box() {
    for (name, size) in [
        ("anim.gif", (6, 4)),
        ("anim.gif", (3, 2)),
        ("anim.webp", (13, 9)),
        ("big-anim.webp", (300, 225)),
    ] {
        let bytes = Arc::new(fixture(name));
        let mut e = Engine::new(bytes.clone(), false, Index::new(&bytes).unwrap());
        let Made::Frame { bitmap, .. } = e.step(size, None, &NO_CANCEL) else {
            panic!("{name}");
        };
        let still = decode_any(&bytes, size.0, size.1).unwrap();
        // (Premultiplied once on the canvas, rounded, and resampled: a level apart at most.)
        for (a, b) in bitmap.data.iter().zip(&still.data) {
            for s in [0, 8, 16, 24] {
                let (x, y) = ((a >> s) & 0xFF, (b >> s) & 0xFF);
                assert!(x.abs_diff(y) <= 1, "{name} at {size:?}: {a:08x} vs {b:08x}");
            }
        }
    }
}

/// The frames shown by playing `bytes` at `size`, `shows` at most.
fn shown(bytes: &[u8], size: (u32, u32), shows: usize) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    play_box(Arc::new(bytes.to_vec()), size, shows, usize::MAX, |b| {
        out.push(b.data.clone())
    })
    .unwrap();
    out
}

#[test]
fn frames_are_made_at_the_canvas_size_for_a_whole_enlargement() {
    let src = (640, 360);
    for (b, want) in [
        ((640, 360), src),
        ((1280, 720), src),
        ((1920, 1080), src),
        ((1280, 360), src),
        ((1440, 810), (1440, 810)),
        ((1281, 720), (1281, 720)),
        ((320, 180), (320, 180)),
        ((960, 540), (960, 540)),
    ] {
        assert_eq!(frame_size(src, b), want, "{b:?}");
    }
    // (A box past the largest side made is cut to it.)
    let side = super::super::MAX_SIDE;
    assert_eq!(frame_size((7, 7), (side + 1, 7)), (side, 7));
}

#[test]
fn frames_resampled_as_they_are_shown_are_the_frames_made_ahead() {
    let bytes = sprites(12, Some(gif::Repeat::Infinite));
    // (Boxes whose frames are more than are kept.)
    for size in [(610, 470), (400, 300), (400, 470), (601, 300)] {
        let two = shown(&bytes, size, 30);
        let mut one = Vec::new();
        let room = two_bitmaps(&bytes, size) - 1;
        let charge = play_box(Arc::new(bytes.clone()), size, 30, room, |b| {
            one.push(b.data.clone())
        })
        .unwrap();
        assert_eq!(two.len(), 30);
        assert_eq!(one, two, "{size:?}");
        let box_px = size.0 as usize * size.1 as usize * 4;
        assert_eq!(charge, two_bitmaps(&bytes, size) - box_px, "{size:?}");
    }
    // Not for a box smaller than the canvas: its resample would cost the window more.
    let frames: Vec<_> = (0..30)
        .map(|i| {
            solid(
                [i * 9, i * 7, 100, 80],
                (i % 4) as u8,
                gif::DisposalMethod::Keep,
            )
        })
        .collect();
    let bytes = gif_of(400, 300, Some(gif::Repeat::Infinite), &frames);
    let size = (170, 130);
    let room = two_bitmaps(&bytes, size) - 1;
    assert!(play_box(Arc::new(bytes.clone()), size, 3, room, |_| {}).is_none());
}

/// What playing `bytes` at `size` is charged with two bitmaps of the box.
fn two_bitmaps(bytes: &[u8], size: (u32, u32)) -> usize {
    let e = Engine::new(Arc::new(bytes.to_vec()), false, Index::new(bytes).unwrap());
    assert!(!e.keeps(size));
    e.cost(size)
}

#[test]
fn kept_frames_play_like_streamed_ones_and_end_as_they_do() {
    let size = (10, 8);
    for (repeat, plays) in [
        (None, Some(1)),
        (Some(gif::Repeat::Finite(1)), Some(2)),
        (Some(gif::Repeat::Infinite), None),
    ] {
        let bytes = sprites(5, repeat);
        let index = Index::new(&bytes).unwrap();
        assert!(index.keeps(size), "kept");
        let frames = shown(&bytes, size, 23);
        assert_eq!(frames.len(), plays.map_or(23, |p| p * 5), "{repeat:?}");
        // Streamed, from a fresh engine each play.
        let bytes = Arc::new(bytes);
        let mut e = Engine::new(bytes.clone(), false, Index::new(&bytes).unwrap());
        for (i, f) in frames.iter().enumerate() {
            let Made::Frame { bitmap, .. } = e.step(size, None, &NO_CANCEL) else {
                panic!("frame {i}");
            };
            assert_eq!(*f, bitmap.data, "{repeat:?}, frame {i}");
        }
    }
    // A large one is streamed, to the same end.
    let bytes = sprites(5, Some(gif::Repeat::Finite(1)));
    assert!(!Index::new(&bytes).unwrap().keeps((400, 300)));
    assert_eq!(shown(&bytes, (400, 300), 23).len(), 10);
}

#[test]
fn a_frame_is_made_only_once_the_one_before_is_shown() {
    let bytes = Arc::new(sprites(4, None));
    let engine = Box::new(Engine::new(
        bytes.clone(),
        false,
        Index::new(&bytes).unwrap(),
    ));
    let mut p = Playing::new(1, engine, (40, 30), false);
    // The first is made at once.
    assert!(p.wants_step(0) && p.front().is_none() && p.due().is_none());
    let (mut e, size, spare) = p.task().unwrap();
    assert!(p.running && !p.wants_step(0));
    let made = e.step(size, spare, &NO_CANCEL);
    p.took(e, made);
    assert!(p.front().is_some());
    // Not painted: nothing more is made.
    assert!(!p.wants_step(0) && p.due().is_none());
    let t = Instant::now();
    p.shown = Some(t);
    assert!(!p.wants_step(0), "painted in another frame");
    p.painted = 1;
    assert!(p.wants_step(0));
    let (mut e, size, spare) = p.task().unwrap();
    let made = e.step(size, spare, &NO_CANCEL);
    p.took(e, made);
    // Made ahead: due once the one shown has been shown its delay (5 cs), and nothing more.
    assert_eq!(p.due(), Some(t + Duration::from_millis(50)));
    assert!(!p.wants_step(0));
    p.swap();
    assert!(p.shown.is_none() && p.due().is_none() && !p.wants_step(0));
    // What it is charged holds from start to end.
    assert!(p.charge() <= p.cost);
}

#[test]
fn a_new_box_makes_the_next_frame_at_it() {
    let bytes = Arc::new(sprites(4, None));
    let engine = Box::new(Engine::new(
        bytes.clone(),
        false,
        Index::new(&bytes).unwrap(),
    ));
    let mut p = Playing::new(1, engine, (40, 30), false);
    let (mut e, size, spare) = p.task().unwrap();
    let made = e.step(size, spare, &NO_CANCEL);
    p.took(e, made);
    let before = p.charge();
    p.want = (80, 60);
    let (e, size, spare) = p.task().unwrap();
    assert_eq!(size, (80, 60));
    assert!(spare.is_none());
    // While it is made, the frame shown is of the old box: charged beside the new.
    p.engine = Some(e);
    assert!(p.charge() > before + 80 * 60 * 4);
    let mut e = p.engine.take().unwrap();
    let made = e.step(size, spare, &NO_CANCEL);
    p.took(e, made);
    p.shown = Some(Instant::now());
    p.swap();
    assert_eq!(p.front().map(|b| (b.width, b.height)), Some((80, 60)));
    // Kept frames are made again from the first.
    let bytes = sprites(4, None);
    let index = Index::new(&bytes).unwrap();
    let engine = Box::new(Engine::new(Arc::new(bytes), false, index));
    let mut p = Playing::new(2, engine, (8, 6), true);
    p.want = (9, 6);
    assert!(p.task().is_err());
}

/// Frames due within [`COALESCE`] of the earliest are shown by its tick, and ticks are
/// [`CAP`] apart at least: 40 animations of 20 ms frames, each started half a millisecond after
/// the one before, are shown in at most 50 repaints a second, and each still plays (none waits
/// more than a tick).
#[test]
fn frames_due_together_share_a_tick_and_ticks_are_capped() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    assert_eq!(next_tick(None, Some(t0)), None, "nothing due: no timer");
    assert_eq!(next_tick(Some(t0), None), Some(t0));
    assert_eq!(next_tick(Some(t0 + ms(5)), Some(t0)), Some(t0 + CAP));
    assert_eq!(next_tick(Some(t0 + ms(50)), Some(t0)), Some(t0 + ms(50)));
    assert!(shows_at(t0 + COALESCE, t0) && !shows_at(t0 + COALESCE + ms(1), t0));

    let frame = delay(20);
    let mut due: Vec<Instant> = (0..40).map(|i| t0 + frame + ms(1) * i / 2).collect();
    let mut shown = [0u32; 40];
    let (mut last, mut ticks) = (None, 0);
    let end = t0 + Duration::from_secs(1);
    while let Some(now) = next_tick(due.iter().min().copied(), last)
        && now < end
    {
        ticks += 1;
        last = Some(now);
        for (d, n) in due.iter_mut().zip(&mut shown) {
            if shows_at(*d, now) {
                // The next frame is due its delay after this one is shown (pull pacing).
                *d = now + frame;
                *n += 1;
            }
        }
    }
    assert!(ticks <= 50, "{ticks} repaints in a second");
    assert!(ticks >= 30, "{ticks}");
    assert!(shown.iter().all(|&n| n >= 30), "{shown:?}");
}

/// An animated WebP turned by its EXIF orientation is shown as its still (turned), not played
/// unturned.
#[test]
fn a_turned_animated_webp_is_shown_as_its_still() {
    let plain = fixture("anim.webp");
    assert!(detect(&plain).is_some());
    let mut turned = plain.clone();
    turned[20] |= 0x08;
    let tiff = &super::super::exif::app1(6)[10..];
    turned.extend_from_slice(b"EXIF");
    turned.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
    turned.extend_from_slice(tiff);
    let riff = (turned.len() - 8) as u32;
    turned[4..8].copy_from_slice(&riff.to_le_bytes());
    assert_eq!(super::super::info_of(&turned).unwrap().orientation, 6);
    assert_eq!(detect(&turned), None);
}

/// The allocation lets each animation in turn play that fits beside those before it, within the
/// room and [`ANIM_ALL_MAX`]; one that doesn't waits, and doesn't keep a smaller one after it
/// from playing. The same bids and room give the same answer.
#[test]
fn the_allocation_plays_those_that_fit_in_their_order() {
    let bid = |id: u32, cost: usize| Bid {
        key: Key::Local(id),
        size: (10, 10),
        cost,
        body: 0,
    };
    let keys = |ids: &[u32]| ids.iter().map(|&id| Key::Local(id)).collect::<Vec<_>>();
    let mb = 1 << 20;
    let bids = vec![
        bid(1, 9 * mb),
        bid(2, 9 * mb),
        bid(3, 2 * mb),
        bid(4, 6 * mb),
    ];
    let a = allocate(bids.clone(), usize::MAX);
    assert_eq!(a.play, keys(&[1, 3]));
    assert_eq!(a.waiting, keys(&[2, 4]));
    // The room the cache has decides too, below the limit for all.
    let a = allocate(bids.clone(), 10 * mb);
    assert_eq!(a.play, keys(&[1]));
    assert_eq!(a.waiting, keys(&[2, 3, 4]));
    let b = allocate(bids.clone(), 10 * mb);
    assert_eq!((a.play, a.waiting), (b.play, b.waiting));
    assert_eq!(allocate(Vec::new(), 0).play, Vec::new());
    // A remote one's body is weighed against the room only, not the limit for all.
    let body = Bid {
        body: 9 * mb,
        ..bid(5, 9 * mb)
    };
    assert_eq!(allocate(vec![body.clone()], usize::MAX).play, keys(&[5]));
    assert_eq!(allocate(vec![body], 17 * mb).waiting, keys(&[5]));
}
