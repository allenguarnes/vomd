//! `cargo run --release --example remote_rss -- BASE [N]`
//! Peak RSS with many large remote images: a document of N images
//! `BASE/big/0.png` … (default 16; the server must give each as a PNG with ~4 MiB after it, see
//! `scripts/remote-rss.sh`), the `always` policy, in a 1920 × 1080 view. The first 3 s nothing
//! is polled (the window is busy): what the fetchers hold then is the first figure. Then the
//! results are taken (and the images near the viewport decoded) until none came for 2 s; the
//! peak RSS is the second.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::platform;
use vomd::settings::RemoteImages;
use vomd::view::View;

fn main() {
    let mut args = std::env::args().skip(1);
    let base = args.next().expect("BASE url");
    let n: usize = args.next().map_or(16, |n| n.parse().expect("N"));
    let src: String = (0..n)
        .map(|i| format!("Image {i}:\n\n![{i}]({base}/big/{i}.png)\n\n"))
        .collect();
    let (w, h) = (1920u32, 1080u32);
    let fonts = Fonts::new().unwrap();
    let mut v = View::with_base(fonts, Doc::parse(&src), None, (w, h), 1.0, None);
    v.set_remote_policy(RemoteImages::Always);
    let mut buf = vec![0u32; (w * h) as usize];
    // The first frame turns remote images on and starts the fetchers; then the window is busy.
    v.paint(&mut buf, w as usize, Instant::now());
    v.tick(Instant::now(), Some(Instant::now()));
    std::thread::sleep(Duration::from_secs(3));
    println!("busy for 3 s: peak RSS {} KB", platform::peak_rss_kb());
    let start = Instant::now();
    let mut quiet = Instant::now();
    while quiet.elapsed() < Duration::from_secs(2) {
        v.paint(&mut buf, w as usize, Instant::now());
        v.tick(Instant::now(), Some(Instant::now()));
        if v.poll_images(Instant::now()) {
            quiet = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "{n} images, results taken in {:.1} s: peak RSS {} KB",
        start.elapsed().as_secs_f64(),
        platform::peak_rss_kb()
    );
}
