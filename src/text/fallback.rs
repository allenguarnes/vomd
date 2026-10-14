//! System font fallback. Runs off the startup path, in a background thread, the
//! first time a character no loaded face covers is laid out.
//!
//! Strategy: a script-keyed preference list matched by file name (cheap: a directory walk),
//! then a scan of every font's cmap. Files are memory-mapped, so a coverage check touches only
//! the header and cmap pages, never whole multi-megabyte CJK files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use swash::FontDataRef;

/// A face to load: file and collection index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub index: u32,
}

/// Directories searched for fonts, most specific first.
pub fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        dirs.extend(data_home.map(|d| d.join("fonts")));
        dirs.extend(home.as_ref().map(|h| h.join(".fonts")));
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(
            data_dirs
                .split(':')
                .filter(|s| !s.is_empty())
                .map(|d| Path::new(d).join("fonts")),
        );
    }
    #[cfg(target_os = "macos")]
    {
        // TODO(P14): untested.
        dirs.extend(home.as_ref().map(|h| h.join("Library/Fonts")));
        dirs.push("/Library/Fonts".into());
        dirs.push("/System/Library/Fonts".into());
    }
    #[cfg(windows)]
    {
        // TODO(P14): untested.
        let _ = home;
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(Path::new(&local).join("Microsoft/Windows/Fonts"));
        }
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(Path::new(&windir).join("Fonts"));
    }
    dirs.dedup();
    dirs
}

fn is_font_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc" | "otc"
        )
    })
}

/// All font files under `dirs` (recursive, bounded depth so symlink loops terminate).
pub fn list_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if depth < 6 {
                    walk(&path, depth + 1, out);
                }
            } else if is_font_file(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        walk(dir, 0, &mut out);
    }
    out
}

fn file_key(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
}

/// The first of `names` (in preference order, case-insensitive) present in the font dirs.
pub fn find_file(names: &[&str]) -> Option<PathBuf> {
    let files = list_files(&font_dirs());
    let index: HashMap<String, &PathBuf> = files
        .iter()
        .filter_map(|p| Some((file_key(p)?, p)))
        .collect();
    names
        .iter()
        .find_map(|n| index.get(&n.to_ascii_lowercase()).map(|p| (*p).clone()))
}

/// Preferred fallback files for a character, by script/block.
fn preferred(c: char) -> &'static [&'static str] {
    const EMOJI: &[&str] = &[
        "notocoloremoji.ttf",
        "twemoji.ttf",
        "twemojimozilla.ttf",
        "joypixels.ttf",
        "seguiemj.ttf",
        "apple color emoji.ttc",
    ];
    const CJK: &[&str] = &[
        "notosanscjk-regular.ttc",
        "notosanscjksc-regular.otf",
        "notosanscjkjp-regular.otf",
        "sourcehansans-regular.ttc",
        "wqy-microhei.ttc",
        "wqy-zenhei.ttc",
        "droidsansfallbackfull.ttf",
        "msyh.ttc",
        "yugothr.ttc",
        "malgun.ttf",
        "pingfang.ttc",
    ];
    const ARABIC: &[&str] = &[
        "notosansarabic-regular.ttf",
        "notonaskharabic-regular.ttf",
        "dejavusans.ttf",
        "segoeui.ttf",
        "geezapro.ttc",
    ];
    const HEBREW: &[&str] = &[
        "notosanshebrew-regular.ttf",
        "dejavusans.ttf",
        "segoeui.ttf",
        "arialhb.ttc",
    ];
    const DEVANAGARI: &[&str] = &["notosansdevanagari-regular.ttf", "mangal.ttf"];
    const THAI: &[&str] = &["notosansthai-regular.ttf", "leelawui.ttf"];
    const GENERIC: &[&str] = &[
        "notosans-regular.ttf",
        "dejavusans.ttf",
        "liberationsans-regular.ttf",
        "segoeui.ttf",
        "arial.ttf",
        "notosanssymbols-regular.ttf",
        "notosanssymbols2-regular.ttf",
        "notosansmath-regular.ttf",
        "segoeuisymbol.ttf",
    ];
    match c as u32 {
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF => EMOJI,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000.. => CJK,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
            ARABIC
        }
        0x0590..=0x05FF => HEBREW,
        0x0900..=0x097F => DEVANAGARI,
        0x0E00..=0x0E7F => THAI,
        _ => GENERIC,
    }
}

/// The face in `path` covering the most of `chars`, with the characters it covers.
fn best_face(path: &Path, chars: &[char]) -> Option<(u32, Vec<char>)> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: read-only mapping of a font file; see `Fonts::load`.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let data = FontDataRef::new(&map)?;
    let mut best: Option<(u32, Vec<char>)> = None;
    for (i, font) in data.fonts().enumerate() {
        let charmap = font.charmap();
        let covered: Vec<char> = chars
            .iter()
            .copied()
            .filter(|&c| charmap.map(c) != 0)
            .collect();
        if !covered.is_empty() && best.as_ref().is_none_or(|(_, b)| covered.len() > b.len()) {
            best = Some((i as u32, covered));
        }
    }
    best
}

/// Style words that make a file a poor fallback (prefer upright regular weights).
fn style_penalty(path: &Path) -> u8 {
    let name = file_key(path).unwrap_or_default();
    const WORDS: &[&str] = &[
        "bold",
        "italic",
        "oblique",
        "light",
        "thin",
        "black",
        "heavy",
        "condensed",
        "medium",
        "semibold",
        "extra",
        "narrow",
        "mono",
    ];
    WORDS
        .iter()
        .filter(|w| name.contains(*w))
        .count()
        .min(u8::MAX as usize) as u8
}

/// Faces covering `missing`, in the order they should be added as fallbacks. `skip` reports
/// faces that are already loaded. Characters nothing covers are simply not returned.
pub fn find_fallbacks(missing: &[char], skip: &dyn Fn(&Path, u32) -> bool) -> Vec<Found> {
    find_in(&list_files(&font_dirs()), missing, skip)
}

pub fn find_in(
    files: &[PathBuf],
    missing: &[char],
    skip: &dyn Fn(&Path, u32) -> bool,
) -> Vec<Found> {
    let mut remaining: Vec<char> = missing.to_vec();
    remaining.sort_unstable();
    remaining.dedup();
    let mut found: Vec<Found> = Vec::new();
    let mut take = |path: &Path, remaining: &mut Vec<char>| {
        if let Some((index, covered)) = best_face(path, remaining)
            && !skip(path, index)
            && !found.iter().any(|f| f.path == path && f.index == index)
        {
            remaining.retain(|c| !covered.contains(c));
            found.push(Found {
                path: path.to_owned(),
                index,
            });
        }
    };
    let by_name: HashMap<String, &PathBuf> = files
        .iter()
        .filter_map(|p| Some((file_key(p)?, p)))
        .collect();
    let mut i = 0;
    while i < remaining.len() {
        let c = remaining[i];
        for name in preferred(c) {
            if let Some(path) = by_name.get(*name) {
                take(path, &mut remaining);
                if !remaining.contains(&c) {
                    break;
                }
            }
        }
        if remaining.get(i) == Some(&c) {
            i += 1;
        }
    }
    if !remaining.is_empty() {
        let mut ordered: Vec<&PathBuf> = files.iter().collect();
        ordered.sort_by_key(|p| style_penalty(p));
        for path in ordered {
            if remaining.is_empty() {
                break;
            }
            take(path, &mut remaining);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_by_script() {
        assert!(preferred('中')[0].contains("cjk"));
        assert!(preferred('😀')[0].contains("emoji"));
        assert!(preferred('م')[0].contains("arabic"));
        assert!(preferred('ש')[0].contains("hebrew"));
        assert!(preferred('Ж')[0].contains("notosans-regular"));
    }

    #[test]
    fn style_penalties() {
        assert_eq!(style_penalty(Path::new("/f/NotoSans-Regular.ttf")), 0);
        assert!(style_penalty(Path::new("/f/NotoSans-BoldItalic.ttf")) >= 2);
    }

    /// Uses the machine's installed fonts; passes vacuously where none exist (CI images).
    #[test]
    fn finds_system_fallbacks() {
        let files = list_files(&font_dirs());
        if files.is_empty() {
            return;
        }
        let chars = ['中', 'Ж', 'م'];
        let found = find_in(&files, &chars, &|_, _| false);
        for f in &found {
            assert!(f.path.exists());
        }
        // Every returned face covers at least one requested character.
        for f in &found {
            assert!(best_face(&f.path, &chars).is_some());
        }
        // Skipping everything finds nothing.
        assert!(find_in(&files, &chars, &|_, _| true).is_empty());
    }
}
