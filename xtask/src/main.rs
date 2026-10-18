//! Dev tooling: `cargo xtask <size|bench|budgets|dist|fixtures|bless>`. Never shipped.
//! `budgets [--timings-advisory] [--report <file.md>]`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{env, fs};

// Budgets (Linux x86_64): (default build, --no-default-features).
const SIZE_BUDGET: [u64; 2] = [4_000_000, 3_000_000];
const COLD_BUDGET_MS: f64 = 100.0;
const WARM_BUDGET_MS: f64 = 50.0;
/// Idle RSS, 50 KB document, 1080p window.
const RSS_SMALL_KB: [u64; 2] = [30_000, 25_000];
/// Idle RSS, 5 MB document, 1080p window.
const RSS_HUGE_KB: [u64; 2] = [60_000, 55_000];
/// Idle CPU is 0%: clock ticks (1/100 s) allowed over `IDLE_WATCH`, for a stray event from
/// the window system; a timer or a busy loop takes many more.
const IDLE_TICKS: u64 = 1;
const IDLE_WATCH: Duration = Duration::from_secs(3);
const SCROLL_BUDGET_MS: f64 = 8.0;
/// Peak RSS with 50 images, local or remote: 30,000 KB idle and the image budget
/// of 32 MiB. (The examples have no window, so no Wayland allowance.)
const IMAGE_RSS_KB: u64 = 30_000 + 32_768;

/// The window the RSS budgets are specified for (logical pixels; the CI display is 1x).
const BENCH_SIZE: &str = "1920x1080";
const STARTUP_FIXTURES: &[&str] = &["tiny.md", "readme.md", "spec.md", "huge.md"];
/// The warm start budget is for documents up to the CommonMark spec (200 KB): the 5 MB one is
/// parsed whole before its first frame (≈ 37 ms of it), so it is held to the cold budget only
/// and its warm start reported.
const WARM_UNGATED: &[&str] = &["huge.md"];
/// On Wayland softbuffer keeps a second frame of the window's size (1920 × 1080 × 4 bytes),
/// which X11 (CI's Xvfb) doesn't: the RSS budgets allow for it there.
const WAYLAND_FRAME_KB: u64 = 1920 * 1080 * 4 / 1024;
const BENCH_RUNS: usize = 20;

const BUILDS: [(&str, &[&str], &str); 2] = [
    ("default", &[], "target"),
    (
        "--no-default-features",
        &["--no-default-features"],
        "target/min",
    ),
];

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let ok = match arg(0) {
        Some("size") => size(),
        Some("bench") => bench(arg(1)),
        Some("budgets") => budgets(
            args.iter()
                .position(|a| a == "--report")
                .and_then(|i| args.get(i + 1))
                .map(Path::new),
            !args.iter().any(|a| a == "--timings-advisory"),
        ),
        Some("dist") => dist(arg(1) == Some("--release")),
        Some("fixtures") => fixtures(),
        Some("bless") => bless(),
        _ => {
            eprintln!(
                "usage: cargo xtask <size | bench [fixture] | budgets [--report FILE] | \
                 dist [--release] | \
                 fixtures | bless>"
            );
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

/// Release binaries target baseline x86-64: a `target-cpu` flag would make them
/// crash on older CPUs, so measuring or packaging a build whose environment sets one is
/// refused. (A `target-cpu` in a cargo config file is overridden by [`cargo_build`].)
fn baseline_cpu() -> bool {
    let set = |v: &str| env::var(v).is_ok_and(|f| f.contains("target-cpu"));
    if set("RUSTFLAGS") || set("CARGO_ENCODED_RUSTFLAGS") || set("CARGO_BUILD_RUSTFLAGS") {
        eprintln!("error: the environment sets target-cpu; release builds must target baseline");
        return false;
    }
    true
}

/// The rustflags of every release build: nothing but RELR, set through
/// `CARGO_ENCODED_RUSTFLAGS` (unit-separator-joined), which overrides `[build] rustflags` and
/// `[target] rustflags` in every config file, so no `target-cpu` from `~/.cargo/config.toml`
/// or a parent directory's reaches the binary.
const RELR_FLAGS: &str = "-C\x1flink-arg=-Wl,-z,pack-relative-relocs";

/// A cargo command in the repository with [`RELR_FLAGS`] for what it builds.
fn release_cargo() -> Command {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(root());
    if cfg!(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )) {
        cmd.env("CARGO_ENCODED_RUSTFLAGS", RELR_FLAGS);
    }
    cmd
}

fn cargo_build(extra: &[&str], target_dir: &str) -> PathBuf {
    let status = release_cargo()
        .args([
            "build",
            "--release",
            "--locked",
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
    if !baseline_cpu() {
        return false;
    }
    let mut ok = true;
    for ((label, extra, dir), budget) in BUILDS.into_iter().zip(SIZE_BUDGET) {
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

/// A `vomd-bench` line's `key=value` fields.
fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .and_then(|v| v.parse().ok())
}

/// Start vomd on `fixture` with `VOMD_BENCH=mode`; its stderr lines come through the channel.
fn spawn(bin: &Path, fixture: &Path, mode: &str) -> Option<(Child, mpsc::Receiver<String>)> {
    let mut child = Command::new(bin)
        .arg("--no-settings")
        .arg(fixture)
        .env("VOMD_BENCH", mode)
        .env("VOMD_BENCH_SIZE", BENCH_SIZE)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let reader = BufReader::new(child.stderr.take()?);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    Some((child, rx))
}

/// Wait for the first `vomd-bench` line (at most `limit`).
fn bench_line(rx: &mpsc::Receiver<String>, limit: Duration) -> Option<String> {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let line = rx.recv_timeout(left).ok()?;
        if let Some(rest) = line.trim().strip_prefix("vomd-bench ") {
            return Some(rest.to_owned());
        }
    }
}

fn reap(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Run {
    wall_ms: f64,
    rss_kb: u64,
}

/// Time from spawn until vomd reports its first frame (it prints
/// `vomd-bench first_frame_ms=<f> rss_kb=<n>` and exits).
fn run_once(bin: &Path, fixture: &Path) -> Option<Run> {
    let start = Instant::now();
    let (child, rx) = spawn(bin, fixture, "1")?;
    let line = bench_line(&rx, Duration::from_secs(10));
    let wall_ms = start.elapsed().as_secs_f64() * 1e3;
    reap(child);
    let line = line?;
    Some(Run {
        wall_ms,
        rss_kb: field(&line, "rss_kb").unwrap_or(0.0) as u64,
    })
}

/// Cold (first run after evicting the binary) and warm (median) start of `fixture`, and the
/// peak RSS at the first frame. `None` unless all [`BENCH_RUNS`] runs gave a first frame: a
/// failed cold run would otherwise make a warm one count as cold.
fn startup(bin: &Path, fixture: &Path) -> Option<(f64, f64, f64, u64)> {
    evict(bin);
    let mut runs: Vec<Run> = Vec::with_capacity(BENCH_RUNS);
    for i in 0..BENCH_RUNS {
        let Some(run) = run_once(bin, fixture) else {
            eprintln!("run {} of {BENCH_RUNS} gave no first frame", i + 1);
            return None;
        };
        runs.push(run);
    }
    let cold = runs[0].wall_ms;
    let rss = runs.iter().map(|r| r.rss_kb).max().unwrap_or(0);
    runs.remove(0);
    runs.sort_by(|a, b| a.wall_ms.total_cmp(&b.wall_ms));
    let warm = runs.get(runs.len() / 2).map_or(cold, |r| r.wall_ms);
    let p90 = runs
        .get((runs.len() * 9 / 10).min(runs.len().saturating_sub(1)))
        .map_or(cold, |r| r.wall_ms);
    Some((cold, warm, p90, rss))
}

fn ensure_fixtures() {
    let dir = root().join("fixtures");
    if !dir.join("huge.md").exists() || !dir.join("medium.md").exists() {
        fixtures();
    }
}

/// The startup table, not gated.
fn bench(only: Option<&str>) -> bool {
    ensure_fixtures();
    let bin = cargo_build(&[], "target");
    let mut ok = true;
    println!(
        "{:<12} {:>9} {:>9} {:>9} {:>10}",
        "fixture", "cold ms", "median", "p90", "peak rss"
    );
    for name in STARTUP_FIXTURES
        .iter()
        .filter(|n| only.is_none_or(|o| o == **n))
    {
        let Some((cold, warm, p90, rss)) = startup(&bin, &root().join("fixtures").join(name))
        else {
            println!("{name:<12} no bench output (is there a display?)");
            ok = false;
            continue;
        };
        println!(
            "{name:<12} {cold:>9.1} {warm:>9.1} {p90:>9.1} {:>7.1} MB",
            rss as f64 / 1024.0
        );
    }
    ok
}

/// CPU time of `pid` in clock ticks (user + system), from `/proc` (xtask is Linux-only).
fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name can hold spaces; the fields after it are space-separated.
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // utime and stime are fields 14 and 15 of stat(5), 12 and 13 after the name.
    Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
}

fn rss_now_kb(pid: u32) -> Option<u64> {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()
}

struct Idle {
    /// Time from spawn until the document was measured and the loop went to sleep.
    ready_ms: f64,
    rss_kb: u64,
    ticks: u64,
}

/// Run vomd until it is idle (`VOMD_BENCH=idle`), then watch it: its RSS and the CPU ticks it
/// takes while nothing happens.
fn idle(bin: &Path, fixture: &Path) -> Option<Idle> {
    let (mut child, rx) = spawn(bin, fixture, "idle")?;
    let pid = child.id();
    let result = (|| {
        let line = bench_line(&rx, Duration::from_secs(60))?;
        let ready_ms = field(&line, "idle_ms")?;
        // Let anything due right after (a scrollbar fading out) pass first.
        std::thread::sleep(Duration::from_secs(1));
        let before = cpu_ticks(pid)?;
        std::thread::sleep(IDLE_WATCH);
        let ticks = cpu_ticks(pid)?.checked_sub(before)?;
        let rss_kb = rss_now_kb(pid)?.max(field(&line, "rss_kb")? as u64);
        Some(Idle {
            ready_ms,
            rss_kb,
            ticks,
        })
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// The scroll frame time of a 4K window (`examples/paint_bench.rs`), built with `extra` features
/// into `target_dir` like the binary of that build: p95 in ms.
fn scroll_p95(fixture: &Path, extra: &[&str], target_dir: &str) -> Option<f64> {
    let out = release_cargo()
        .args(["run", "--release", "--locked", "--quiet", "--target-dir"])
        .arg(target_dir)
        .args(extra)
        .arg("--example")
        .arg("paint_bench")
        .arg("--")
        .arg(fixture)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    print!("{text}");
    let line = text.lines().find(|l| l.starts_with("paint-bench "))?;
    field(line, "p95_ms")
}

/// The peak RSS an RSS example reports in its `image-rss peak_rss_kb=N` line, in KB.
fn image_rss_line(output: &std::process::Output) -> Option<u64> {
    let text = String::from_utf8_lossy(&output.stdout);
    print!("{text}");
    let line = text.lines().find(|l| l.starts_with("image-rss "))?;
    output.status.success().then_some(())?;
    field(line, "peak_rss_kb").map(|v| v as u64)
}

/// Build the RSS examples like the default binary and give the directory they are in.
fn image_examples() -> Option<PathBuf> {
    let built = release_cargo()
        .args(["build", "--release", "--locked", "--package", "vomd"])
        .args(["--target-dir", "target"])
        .args(["--example", "image_rss", "--example", "remote_rss"])
        .status()
        .ok()?
        .success();
    built.then(|| root().join("target/release/examples"))
}

/// A scratch directory of our own for an RSS run, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("vomd-xtask-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// Peak RSS scrolling through 50 local PNGs of 2000 × 1500 (`examples/image_rss.rs`: the images
/// are written first, by a process of their own, and removed after).
fn local_image_rss(examples: &Path) -> Option<u64> {
    let dir = scratch("images");
    let config = scratch("images-config");
    let bin = examples.join("image_rss");
    let made = Command::new(&bin)
        .arg("--generate")
        .arg(dir.join("img"))
        .env("XDG_CONFIG_HOME", &config)
        .stdout(Stdio::null())
        .status()
        .ok()?
        .success();
    let peak = made
        .then(|| {
            Command::new(&bin)
                .arg(dir.join("img"))
                .env("XDG_CONFIG_HOME", &config)
                .output()
                .ok()
        })
        .flatten()
        .and_then(|out| image_rss_line(&out));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&config);
    peak
}

/// Peak RSS of `scripts/remote-rss.sh`'s `mode` run with `n` images (PNGs of 4 MiB, or JPEGs
/// that are 325 KB and 178 bytes without metadata), polled every `poll` ms, from a server on
/// 127.0.0.1 that it starts and stops (`python3`; `None` with `Err` if there is none, which skips
/// the row). A run that didn't fetch and decode what it was to fails, and so gives `None`.
fn remote_image_rss(examples: &Path, n: &str, mode: &str, poll: &str) -> Result<Option<u64>, ()> {
    let python = Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !python.is_ok_and(|s| s.success()) {
        return Err(());
    }
    let out = Command::new("sh")
        .arg(root().join("scripts/remote-rss.sh"))
        .args([n, mode, poll])
        .env("REMOTE_RSS_BIN", examples.join("remote_rss"))
        .output()
        .ok();
    Ok(out.and_then(|out| image_rss_line(&out)))
}

struct Row {
    metric: String,
    build: &'static str,
    value: String,
    budget: String,
    pass: bool,
    /// A miss fails the run (else the row is only reported).
    gated: bool,
}

/// Every budget row, measured and checked; a miss fails. Needs a display (CI runs Xvfb).
/// Without `timings_gated` (`--timings-advisory`, for shared CI runners, which are two to three
/// times slower than the machine the budgets are for) the start and scroll times are only
/// reported; size, memory, idle CPU and a run that gives no output still fail.
fn budgets(report: Option<&Path>, timings_gated: bool) -> bool {
    if !baseline_cpu() {
        return false;
    }
    ensure_fixtures();
    let fixture = |n: &str| root().join("fixtures").join(n);
    let wayland = env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let frame_kb = if wayland { WAYLAND_FRAME_KB } else { 0 };
    if wayland {
        println!("Wayland: the RSS budgets allow {frame_kb} KB for softbuffer's second frame");
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut gated_row =
        |metric: String, build, value: String, budget: String, pass: bool, gated: bool| {
            let verdict = match (pass, gated) {
                (true, _) => "PASS",
                (false, true) => "FAIL",
                (false, false) => "OVER",
            };
            println!("{verdict} {metric:<34} {build:<22} {value:>12}  budget {budget}");
            rows.push(Row {
                metric,
                build,
                value,
                budget,
                pass,
                gated,
            });
        };
    let mb = |kb: u64| format!("{kb} KB");
    let allowance = if wayland {
        format!(" + {frame_kb} KB (Wayland)")
    } else {
        String::new()
    };
    for (i, (label, extra, dir)) in BUILDS.into_iter().enumerate() {
        let bin = cargo_build(extra, dir);
        let len = fs::metadata(&bin).expect("binary").len();
        gated_row(
            "stripped binary".into(),
            label,
            format!("{:.2} MB", len as f64 / 1e6),
            format!("≤ {:.1} MB", SIZE_BUDGET[i] as f64 / 1e6),
            len <= SIZE_BUDGET[i],
            true,
        );
        for name in STARTUP_FIXTURES {
            match startup(&bin, &fixture(name)) {
                Some((cold, warm, _, _)) => {
                    gated_row(
                        format!("cold start, {name}"),
                        label,
                        format!("{cold:.1} ms"),
                        format!("≤ {COLD_BUDGET_MS} ms (first of {BENCH_RUNS} runs)"),
                        cold <= COLD_BUDGET_MS,
                        timings_gated,
                    );
                    gated_row(
                        format!("warm start, {name}"),
                        label,
                        format!("{warm:.1} ms"),
                        format!("≤ {WARM_BUDGET_MS} ms (median of {})", BENCH_RUNS - 1),
                        warm <= WARM_BUDGET_MS,
                        timings_gated && !WARM_UNGATED.contains(name),
                    );
                }
                None => gated_row(
                    format!("start, {name}"),
                    label,
                    "no output".into(),
                    "a first frame".into(),
                    false,
                    true,
                ),
            }
        }
        for (name, what, budget) in [
            ("medium.md", "50 KB", RSS_SMALL_KB[i]),
            ("huge.md", "5 MB", RSS_HUGE_KB[i]),
        ] {
            let Some(r) = idle(&bin, &fixture(name)) else {
                gated_row(
                    format!("idle, {what} doc"),
                    label,
                    "no output".into(),
                    "idle".into(),
                    false,
                    true,
                );
                continue;
            };
            gated_row(
                format!("idle RSS, {what} doc, 1080p"),
                label,
                mb(r.rss_kb),
                format!("≤ {}{allowance}", mb(budget)),
                r.rss_kb <= budget + frame_kb,
                true,
            );
            gated_row(
                format!("idle CPU, {what} doc (idle at {:.0} ms)", r.ready_ms),
                label,
                format!("{} ticks/{}s", r.ticks, IDLE_WATCH.as_secs()),
                format!("≤ {IDLE_TICKS} tick"),
                r.ticks <= IDLE_TICKS,
                true,
            );
        }
    }
    // Both builds have a scroll budget.
    for (label, extra, dir) in BUILDS {
        match scroll_p95(&fixture("readme.md"), extra, dir) {
            Some(p95) => gated_row(
                "scroll frame, 4K, p95".into(),
                label,
                format!("{p95:.2} ms"),
                format!("≤ {SCROLL_BUDGET_MS} ms"),
                p95 <= SCROLL_BUDGET_MS,
                timings_gated,
            ),
            None => gated_row(
                "scroll frame, 4K".into(),
                label,
                "no output".into(),
                "a measurement".into(),
                false,
                true,
            ),
        }
    }
    // Memory with 50 images, from files and from a local server.
    let label = BUILDS[0].0;
    let budget = format!("≤ {}", mb(IMAGE_RSS_KB));
    match image_examples() {
        None => gated_row(
            "image RSS examples".into(),
            label,
            "no build".into(),
            "built".into(),
            false,
            true,
        ),
        Some(examples) => {
            let mut row = |metric: &str, peak: Option<u64>| match peak {
                Some(kb) => gated_row(
                    metric.into(),
                    label,
                    mb(kb),
                    budget.clone(),
                    kb <= IMAGE_RSS_KB,
                    true,
                ),
                None => gated_row(
                    metric.into(),
                    label,
                    "no output".into(),
                    "a measurement".into(),
                    false,
                    true,
                ),
            };
            row("image RSS, 50 local PNGs", local_image_rss(&examples));
            // Every way the memory budget is to hold: far from the viewport, in view, fast or
            // slow to be polled, and for bodies that shrink once their metadata is cut out.
            for (metric, n, mode, poll) in [
                ("image RSS, 50 remote PNGs", "50", "big", "16"),
                (
                    "image RSS, 50 remote PNGs in view, polled every 16 ms",
                    "50",
                    "inline",
                    "16",
                ),
                (
                    "image RSS, 50 remote PNGs in view, polled every 50 ms",
                    "50",
                    "inline",
                    "50",
                ),
                (
                    "image RSS, 16,384 remote JPEGs of 325 KB cut to 178 bytes",
                    "16384",
                    "stripped",
                    "16",
                ),
            ] {
                match remote_image_rss(&examples, n, mode, poll) {
                    Ok(peak) => row(metric, peak),
                    Err(()) => println!("SKIP {metric}: python3 not found"),
                }
            }
        }
    }
    let ok = rows.iter().all(|r| r.pass || !r.gated);
    if let Some(path) = report {
        let mut md =
            String::from("| | Metric | Build | Measured | Budget |\n|---|---|---|---|---|\n");
        for r in &rows {
            md.push_str(&format!(
                "| {} | {} | `{}` | {} | {} |\n",
                match (r.pass, r.gated) {
                    (true, _) => "✅",
                    (false, true) => "❌",
                    (false, false) => "⚠️ not gated",
                },
                r.metric,
                r.build,
                r.value,
                r.budget
            ));
        }
        fs::write(path, md).expect("write the report");
    }
    println!(
        "{}",
        if ok {
            "all budgets met"
        } else {
            "BUDGET MISSED"
        }
    );
    ok
}

/// The release tarball: `dist/vomd-<version>-x86_64-linux.tar.gz` holding the
/// binary, the desktop entry, the icon, README, CHANGELOG and the licenses (ours and those of
/// the embedded fonts).
fn dist(release: bool) -> bool {
    if !baseline_cpu() {
        return false;
    }
    let bin = cargo_build(&[], "target");
    // The newest glibc the binary can't run without (weak symbols are optional).
    let floor = glibc_floor(&bin);
    match floor {
        Some((a, b)) if (a, b) <= GLIBC_FLOOR => println!("needs glibc {a}.{b}"),
        Some((a, b)) => {
            eprintln!(
                "{}: the binary needs glibc {a}.{b}, newer than the {}.{} promised: build \
                 releases on an older distribution (CI uses Ubuntu 24.04)",
                if release { "error" } else { "warning" },
                GLIBC_FLOOR.0,
                GLIBC_FLOOR.1
            );
            if release {
                return false;
            }
        }
        None => {
            eprintln!("cannot read the binary's glibc versions (objdump)");
            if release {
                return false;
            }
        }
    }
    let version = env!("CARGO_PKG_VERSION");
    let manifest = fs::read_to_string(root().join("Cargo.toml")).expect("Cargo.toml");
    let ours = manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = \""))
        .and_then(|v| v.strip_suffix('"'))
        .expect("version in Cargo.toml");
    if ours != version {
        eprintln!("note: xtask {version}, vomd {ours}");
    }
    let name = format!("vomd-{ours}-x86_64-linux");
    let dist = root().join("dist");
    let stage = dist.join(&name);
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).expect("create dist/");
    let files: [(&Path, &str); 10] = [
        (&bin, "vomd"),
        (Path::new("packaging/vomd.desktop"), "vomd.desktop"),
        (Path::new("assets/icon.svg"), "vomd.svg"),
        (Path::new("README.md"), "README.md"),
        // The README's image, at the path it names.
        (Path::new("assets/icon@2x.png"), "assets/icon@2x.png"),
        (Path::new("CHANGELOG.md"), "CHANGELOG.md"),
        (Path::new("LICENSE"), "LICENSE"),
        // The fonts and icons built into the binary.
        (
            Path::new("assets/fonts/LICENSE-Inter.txt"),
            "LICENSE-Inter.txt",
        ),
        (
            Path::new("assets/fonts/LICENSE-JetBrainsMono.txt"),
            "LICENSE-JetBrainsMono.txt",
        ),
        (
            Path::new("assets/fonts/LICENSE-Lucide.txt"),
            "LICENSE-Lucide.txt",
        ),
    ];
    for (from, to) in files {
        let to = stage.join(to);
        fs::create_dir_all(to.parent().unwrap()).expect("create dist/ subdirectory");
        fs::copy(root().join(from), to).unwrap_or_else(|e| panic!("copy {}: {e}", from.display()));
    }
    let tarball = dist.join(format!("{name}.tar.gz"));
    let ok = Command::new("tar")
        .current_dir(&dist)
        .args(["--owner=0", "--group=0", "--numeric-owner", "-czf"])
        .arg(&tarball)
        .arg(&name)
        .status()
        .is_ok_and(|s| s.success());
    let _ = fs::remove_dir_all(&stage);
    if !ok {
        eprintln!("tar failed");
        return false;
    }
    let sum = Command::new("sha256sum")
        .current_dir(&dist)
        .arg(tarball.file_name().unwrap())
        .output()
        .expect("sha256sum");
    fs::write(dist.join(format!("{name}.tar.gz.sha256")), &sum.stdout).expect("write sum");
    print!("{}", String::from_utf8_lossy(&sum.stdout));
    println!("wrote {}", tarball.display());
    true
}

/// The oldest glibc release binaries support: RELR relocations need 2.36.
const GLIBC_FLOOR: (u32, u32) = (2, 36);

/// The newest `GLIBC_x.y` version a strong undefined symbol of `bin` needs (`objdump -T`).
fn glibc_floor(bin: &Path) -> Option<(u32, u32)> {
    let out = Command::new("objdump").arg("-T").arg(bin).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut newest = (2, 0);
    for line in text.lines().filter(|l| l.contains("*UND*")) {
        // `0000… w   DF *UND*  0000… (GLIBC_2.39) name`: the flags come before `DF`.
        let weak = line.split_whitespace().nth(1) == Some("w");
        let Some(v) = line.split("GLIBC_").nth(1) else {
            continue;
        };
        let v: String = v
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let version = (parts.next()?, parts.next().unwrap_or(0));
        if !weak {
            newest = newest.max(version);
        }
    }
    Some(newest)
}

/// Regenerate the render goldens in `tests/golden/`. Review the images before
/// committing them.
fn bless() -> bool {
    Command::new(env!("CARGO"))
        .current_dir(root())
        .args([
            "test",
            "--package",
            "vomd",
            "--test",
            "golden",
            "--",
            "--nocapture",
        ])
        .env("VOMD_BLESS", "1")
        .status()
        .expect("run cargo")
        .success()
}

/// Generate the large fixtures that aren't committed: `huge.md` (5 MB) and `medium.md` (the
/// 50 KB document of the idle RSS budget).
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
    // Whole lines of the same text, up to 50 KB.
    let mut medium = String::with_capacity(52_000);
    for line in out.split_inclusive('\n') {
        if medium.len() + line.len() > 50_000 {
            break;
        }
        medium.push_str(line);
    }
    fs::write(dir.join("medium.md"), &medium).expect("write medium.md");
    println!("wrote fixtures/medium.md ({} bytes)", medium.len());
    true
}
