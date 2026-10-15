use super::*;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn have_curl() -> bool {
    Command::new("curl")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A 2 × 1 PNG.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 2, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.write_header()
        .unwrap()
        .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
        .unwrap();
    out
}

fn respond(mut s: TcpStream, status: &str, headers: &str, body: &[u8]) {
    let _ = write!(s, "HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n");
    let _ = s.write_all(body);
}

/// Serve one request on `s`.
fn serve(s: TcpStream, port: u16) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    // Skip the headers.
    let mut h = String::new();
    while r.read_line(&mut h).is_ok_and(|n| n > 2) {
        h.clear();
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let png = png();
    let len = |n: usize| format!("Content-Length: {n}\r\n");
    match path.as_str() {
        "/ok.png" | "/-o" => respond(s, "200 OK", &len(png.len()), &png),
        p if p.starts_with("/r/") => {
            // `/r/N` redirects N times before the image.
            let n: u32 = p[3..].parse().unwrap_or(0);
            if n == 0 {
                respond(s, "200 OK", &len(png.len()), &png);
            } else {
                let to = format!("Location: http://127.0.0.1:{port}/r/{}\r\n", n - 1);
                respond(s, "302 Found", &(to + &len(0)), b"");
            }
        }
        "/to-file" => respond(
            s,
            "302 Found",
            "Location: file:///etc/hostname\r\nContent-Length: 0\r\n",
            b"",
        ),
        "/to-ftp" => respond(
            s,
            "302 Found",
            &format!("Location: ftp://127.0.0.1:{port}/x.png\r\nContent-Length: 0\r\n"),
            b"",
        ),
        // Says how big it is: curl refuses it before reading.
        "/big-declared" => respond(s, "200 OK", &len(4096), &[0u8; 4096]),
        // Doesn't say (the connection's end ends it): the pipe is cut while reading.
        "/big-stream" => respond(s, "200 OK", "", &[0u8; 4096]),
        "/slow" => {
            std::thread::sleep(Duration::from_secs(4));
            respond(s, "200 OK", &len(png.len()), &png);
        }
        "/page.html" => respond(s, "200 OK", &len(6), b"<html>"),
        _ => respond(s, "404 Not Found", &len(0), b""),
    }
}

/// A server on 127.0.0.1 answering every request in its own thread; its port.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || serve(s, port));
        }
    });
    port
}

const SMALL: Limits = Limits {
    bytes: 1024,
    secs: 2,
    redirects: 5,
};

#[test]
fn only_http_urls_are_handed_to_curl() {
    assert!(fetchable("https://x.y/a.png"));
    assert!(fetchable("HTTP://x.y/a.png"));
    for url in [
        "-o/tmp/x",
        "--config=/etc/passwd",
        "-K/etc/passwd",
        "file:///etc/passwd",
        "ftp://x.y/a.png",
        "x.png",
        "https://x.y/a b.png",
        "https://x.y/\na.png",
        "",
    ] {
        assert!(!fetchable(url), "{url:?}");
        assert_eq!(
            fetch_with("/nonexistent/curl", url, SMALL),
            Err(Error::Refused)
        );
    }
}

/// The size cap holds while reading the pipe, whatever the program does: a `curl` that
/// writes forever is stopped.
#[cfg(unix)]
#[test]
fn reading_stops_at_the_size_cap() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vomd-remote-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("curl");
    std::fs::write(&fake, "#!/bin/sh\nexec yes\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let t = Instant::now();
    let got = fetch_with(fake.to_str().unwrap(), "https://x.y/a.png", SMALL);
    assert_eq!(got, Err(Error::TooLarge));
    assert!(t.elapsed() < Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_curl_is_told_apart() {
    assert_eq!(
        fetch_with("/nonexistent/vomd-curl", "https://x.y/a.png", SMALL),
        Err(Error::NoCurl)
    );
}

#[test]
fn fetching_follows_the_rules() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let url = |p: &str| format!("http://127.0.0.1:{port}{p}");
    assert_eq!(fetch(&url("/ok.png"), SMALL), Ok(png()));
    // A path that looks like an option stays a URL (it comes after `--`).
    assert_eq!(fetch(&url("/-o"), SMALL), Ok(png()));
    // At most 5 redirects.
    assert_eq!(fetch(&url("/r/5"), SMALL), Ok(png()));
    assert_eq!(fetch(&url("/r/6"), SMALL), Err(Error::Failed));
    // Redirects only to http(s).
    assert_eq!(fetch(&url("/to-file"), SMALL), Err(Error::Failed));
    assert_eq!(fetch(&url("/to-ftp"), SMALL), Err(Error::Failed));
    // The size cap, declared or not.
    assert_eq!(fetch(&url("/big-declared"), SMALL), Err(Error::Failed));
    // (Recent curl cuts an undeclared size off itself; the pipe is cut too, see below.)
    let streamed = fetch(&url("/big-stream"), SMALL);
    assert!(
        matches!(streamed, Err(Error::Failed | Error::TooLarge)),
        "{streamed:?}"
    );
    // HTTP errors fail.
    assert_eq!(fetch(&url("/missing"), SMALL), Err(Error::Failed));
    // The time cap.
    let t = Instant::now();
    assert_eq!(fetch(&url("/slow"), SMALL), Err(Error::Failed));
    assert!(
        t.elapsed() < Duration::from_millis(3500),
        "{:?}",
        t.elapsed()
    );
    // Only images are kept.
    assert!(fetch(&url("/page.html"), SMALL).is_ok());
    assert_eq!(fetch_image(&url("/page.html")), Err(Error::NotAnImage));
    let (bytes, w, h) = fetch_image(&url("/ok.png")).unwrap();
    assert_eq!((&bytes[..], w, h), (&png()[..], 2, 1));
}

#[test]
fn what_fetching_found_decides_the_layout() {
    let mut r = Remote::default();
    let url: Arc<str> = "https://x.y/a.png".into();
    assert_eq!(r.layout_size(&url), ImageSize::Alt, "off");
    r.on = true;
    assert_eq!(r.layout_size(&url), ImageSize::Loading);
    let bytes: Arc<[u8]> = png().into();
    assert!(r.fetched(&url, Ok((bytes, 2, 1))).is_some());
    assert_eq!(r.layout_size(&url), ImageSize::Pixels(2, 1));
    r.failed(&url);
    assert_eq!(r.layout_size(&url), ImageSize::Alt);
    let other: Arc<str> = "https://x.y/b.png".into();
    assert!(r.fetched(&other, Err(Error::NoCurl)).is_none());
    assert!(r.no_curl);
    assert_eq!(r.layout_size(&other), ImageSize::Alt, "nothing can load");
    // Off, nothing is fetched (and no fetcher started).
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut r = Remote::default();
    r.fetch(vec![url.clone()], tx, Arc::new(|| {}));
    assert!(!r.started());
}
