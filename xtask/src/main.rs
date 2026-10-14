//! Dev tooling: `cargo xtask <size|bench|fixtures>`. Never shipped.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs};

// Budgets (Linux x86_64).
const SIZE_BUDGET_DEFAULT: u64 = 4_000_000;
const SIZE_BUDGET_MIN: u64 = 3_000_000;
const FIRST_FRAME_BUDGET_MS: f64 = 100.0;
const RSS_BUDGET_KB: u64 = 30_000;

const BENCH_FIXTURES: &[&str] = &["tiny.md", "readme.md", "spec.md", "huge.md"];
const BENCH_RUNS: usize = 20;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let ok = match args.first().map(String::as_str) {
        Some("size") => size(),
        Some("bench") => bench(args.get(1).map(String::as_str)),
        Some("fixtures") => fixtures(),
        _ => {
            eprintln!("usage: cargo xtask <size | bench [fixture] | fixtures>");
            false
        }
    };
    std::process::exit(if ok { 0 } else { 1 });
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn cargo_build(extra: &[&str], target_dir: &str) -> PathBuf {
    if env::var("RUSTFLAGS").is_ok_and(|f| f.contains("target-cpu")) {
        eprintln!("warning: RUSTFLAGS sets target-cpu; numbers won't match release builds");
    }
    let status = Command::new(env!("CARGO"))
        .current_dir(root())
        .args([
            "build",
            "--release",
            "--package",
            "vomd",
            "--target-dir",
            target_dir,
        ])
        .args(extra)
        .status()
        .expect("run cargo");
    assert!(status.success(), "cargo build failed");
    root().join(target_dir).join("release/vomd")
}

fn size() -> bool {
    let mut ok = true;
    for (label, extra, dir, budget) in [
        ("default", &[][..], "target", SIZE_BUDGET_DEFAULT),
        (
            "--no-default-features",
            &["--no-default-features"][..],
            "target/min",
            SIZE_BUDGET_MIN,
        ),
    ] {
        let bin = cargo_build(extra, dir);
        let len = fs::metadata(&bin).expect("binary").len();
        let pass = len <= budget;
        ok &= pass;
        println!(
            "{} {label:<24} {:>9} bytes ({:.2} MB)  budget {:.1} MB",
            if pass { "PASS" } else { "FAIL" },
            len,
            len as f64 / 1e6,
            budget as f64 / 1e6
        );
    }
    ok
}

/// Drop the binary from the page cache so the first run approximates a cold start.
/// (Shared libraries stay cached; a true cold start needs root to drop all caches.)
fn evict(path: &Path) {
    use std::os::fd::AsRawFd;
    if let Ok(f) = fs::File::open(path) {
        unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    }
}

struct Run {
    wall_ms: f64,
    rss_kb: u64,
}

/// Spawn vomd with VOMD_BENCH=1 and time from spawn until it reports its first frame.
/// The child prints `vomd-bench first_frame_ms=<f> rss_kb=<n>` to stderr and exits.
fn run_once(bin: &Path, fixture: &Path) -> Option<Run> {
    let start = Instant::now();
    let mut child = Command::new(bin)
        .arg("--no-settings")
        .arg(fixture)
        .env("VOMD_BENCH", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut line = String::new();
    let mut reader = BufReader::new(child.stderr.take()?);
    let mut found = None;
    while reader.read_line(&mut line).ok()? > 0 {
        if let Some(rest) = line.trim().strip_prefix("vomd-bench ") {
            let wall_ms = start.elapsed().as_secs_f64() * 1e3;
            let rss_kb = rest
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("rss_kb="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            found = Some(Run { wall_ms, rss_kb });
            break;
        }
        line.clear();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    found
}

fn bench(only: Option<&str>) -> bool {
    if !root().join("fixtures/huge.md").exists() {
        fixtures();
    }
    let bin = cargo_build(&[], "target");
    let mut ok = true;
    println!(
        "{:<12} {:>9} {:>9} {:>9} {:>10}",
        "fixture", "cold ms", "median", "p90", "max rss"
    );
    for name in BENCH_FIXTURES
        .iter()
        .filter(|n| only.is_none_or(|o| o == **n))
    {
        let fixture = root().join("fixtures").join(name);
        evict(&bin);
        let mut runs: Vec<Run> = (0..BENCH_RUNS)
            .filter_map(|_| run_once(&bin, &fixture))
            .collect();
        if runs.is_empty() {
            println!("{name:<12} no bench output (is VOMD_BENCH implemented?)");
            ok = false;
            continue;
        }
        let cold = runs[0].wall_ms;
        let max_rss = runs.iter().map(|r| r.rss_kb).max().unwrap_or(0);
        runs.sort_by(|a, b| a.wall_ms.total_cmp(&b.wall_ms));
        let median = runs[runs.len() / 2].wall_ms;
        let p90 = runs[(runs.len() * 9 / 10).min(runs.len() - 1)].wall_ms;
        let pass = cold <= FIRST_FRAME_BUDGET_MS && max_rss <= RSS_BUDGET_KB;
        // The 5 MB fixture has its own RSS budget; report it but don't gate on 30 MB.
        if *name != "huge.md" {
            ok &= pass;
        }
        println!(
            "{name:<12} {cold:>9.1} {median:>9.1} {p90:>9.1} {:>7.1} MB",
            max_rss as f64 / 1024.0
        );
    }
    println!(
        "budgets: first frame ≤ {FIRST_FRAME_BUDGET_MS} ms, RSS ≤ {} MB (50 KB doc @1080p)",
        RSS_BUDGET_KB / 1000
    );
    ok
}

/// Generate large fixtures that aren't committed.
fn fixtures() -> bool {
    let dir = root().join("fixtures");
    let mut seed = fs::read_to_string(dir.join("readme.md")).expect("fixtures/readme.md");
    seed.push_str(&fs::read_to_string(dir.join("tables.md")).expect("fixtures/tables.md"));
    let mut out = String::with_capacity(5_300_000);
    let mut i = 0;
    while out.len() < 5_000_000 {
        i += 1;
        out.push_str(&format!("\n# Section {i}\n\n"));
        out.push_str(&seed);
    }
    fs::write(dir.join("huge.md"), &out).expect("write huge.md");
    println!("wrote fixtures/huge.md ({} bytes)", out.len());
    true
}
