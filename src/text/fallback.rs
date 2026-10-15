//! System font fallback. Runs off the startup path, in a background thread, the
//! first time a character no loaded face covers is laid out.
//!
//! Strategy: a script-keyed preference list matched by file name (cheap: a directory walk),
//! then a scan of every font's cmap. Files are memory-mapped, so a coverage check touches only
//! the header and cmap pages, never whole multi-megabyte CJK files.

use std::collections::{BTreeSet, HashMap};
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

/// What each face of a font file covers of some characters: the face's index in the file and the
/// characters (faces that cover none are left out).
type Coverage = Vec<(u32, Vec<char>)>;

/// The faces in `path` and which of `chars` each covers; nothing if the file can't be read.
fn coverage(path: &Path, chars: &[char]) -> Coverage {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    // SAFETY: read-only mapping of a font file; see `Fonts::load`.
    let Ok(map) = (unsafe { memmap2::Mmap::map(&file) }) else {
        return Vec::new();
    };
    let Some(data) = FontDataRef::new(&map) else {
        return Vec::new();
    };
    let mut faces = Vec::new();
    for (i, font) in data.fonts().enumerate() {
        let charmap = font.charmap();
        let covered: Vec<char> = chars
            .iter()
            .copied()
            .filter(|&c| charmap.map(c) != 0)
            .collect();
        if !covered.is_empty() {
            faces.push((i as u32, covered));
        }
    }
    faces
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
    find_with(files, missing, skip, &mut coverage)
}

/// [`find_in`] with the reading of font files (`read`) in the caller's hands. A file is read at
/// most once, however many characters are missing.
fn find_with<'a>(
    files: &'a [PathBuf],
    missing: &[char],
    skip: &dyn Fn(&Path, u32) -> bool,
    read: &mut dyn FnMut(&Path, &[char]) -> Coverage,
) -> Vec<Found> {
    let mut all: Vec<char> = missing.to_vec();
    all.sort_unstable();
    all.dedup();
    let mut remaining: BTreeSet<char> = all.iter().copied().collect();
    let mut found: Vec<Found> = Vec::new();
    // What the files already read cover (of `all`); a file read in the scan below is not needed
    // again, and is kept as covering nothing.
    let mut read_files: HashMap<&'a Path, Coverage> = HashMap::new();
    let mut take = |path: &'a Path, remaining: &mut BTreeSet<char>, preferred: bool| {
        let faces = read_files.entry(path).or_insert_with(|| {
            if preferred {
                read(path, &all)
            } else {
                let chars: Vec<char> = remaining.iter().copied().collect();
                read(path, &chars)
            }
        });
        // The face covering the most of what is still missing, and then the next, until no
        // face of the file helps: the file is not read again, so this is its only chance.
        loop {
            let mut best: Option<(u32, usize)> = None;
            for (index, covered) in faces.iter() {
                let n = covered.iter().filter(|c| remaining.contains(c)).count();
                if n > 0
                    && best.is_none_or(|(_, b)| n > b)
                    && !skip(path, *index)
                    && !found.iter().any(|f| f.path == path && f.index == *index)
                {
                    best = Some((*index, n));
                }
            }
            let Some((index, _)) = best else { break };
            for c in faces
                .iter()
                .filter(|(i, _)| *i == index)
                .flat_map(|(_, c)| c)
            {
                remaining.remove(c);
            }
            found.push(Found {
                path: path.to_owned(),
                index,
            });
        }
        if !preferred {
            *faces = Vec::new();
        }
    };
    let by_name: HashMap<String, &PathBuf> = files
        .iter()
        .filter_map(|p| Some((file_key(p)?, p)))
        .collect();
    // Characters that share a preference list are looked up together, so that each preferred
    // file is read once for all of them.
    let mut groups: Vec<(&[&str], Vec<char>)> = Vec::new();
    for &c in &all {
        let list = preferred(c);
        match groups.iter_mut().find(|(l, _)| *l == list) {
            Some((_, chars)) => chars.push(c),
            None => groups.push((list, vec![c])),
        }
    }
    for (list, chars) in &groups {
        for name in *list {
            if !chars.iter().any(|c| remaining.contains(c)) {
                break;
            }
            if let Some(path) = by_name.get(*name) {
                take(path, &mut remaining, true);
            }
        }
    }
    if !remaining.is_empty() {
        let mut ordered: Vec<&PathBuf> = files.iter().collect();
        ordered.sort_by_key(|p| style_penalty(p));
        for path in ordered {
            if remaining.is_empty() {
                break;
            }
            take(path, &mut remaining, false);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

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
            assert!(!coverage(&f.path, &chars).is_empty());
        }
        // Skipping everything finds nothing.
        assert!(find_in(&files, &chars, &|_, _| true).is_empty());
    }

    /// Reading is counted per file, as a font file would be opened: with thousands of missing
    /// characters every file (preferred or not) is still read once.
    #[test]
    fn each_font_file_is_read_once() {
        let names = [
            "NotoSansCJK-Regular.ttc",
            "NotoSansCJKsc-Regular.otf",
            "DejaVuSans.ttf",
            "NotoSansArabic-Regular.ttf",
            "NotoColorEmoji.ttf",
        ];
        let mut files: Vec<PathBuf> = names.iter().map(|n| Path::new("/fonts").join(n)).collect();
        files.extend((0..300).map(|i| PathBuf::from(format!("/fonts/extra/Other{i}.ttf"))));
        // Characters of several scripts (each with its own preference list), the CJK ones
        // spread over many blocks, interleaved: more than any one file covers.
        let mut missing: Vec<char> = (0..4000u32)
            .filter_map(|i| char::from_u32(0x30000 + i))
            .collect();
        missing.extend((0..300u32).filter_map(|i| char::from_u32(0x4E00 + i * 3)));
        missing.extend(['Ж', 'م', '😀', '\u{10FFFF}']);
        // The CJK collection has two faces; the second covers more. Each of the other files
        // covers a share of the characters by their value.
        let reads: RefCell<HashMap<PathBuf, usize>> = RefCell::default();
        let mut read = |path: &Path, chars: &[char]| -> Coverage {
            *reads.borrow_mut().entry(path.to_owned()).or_default() += 1;
            let name = path.file_name().unwrap().to_str().unwrap();
            let face = |modulus: u32, rem: u32| -> Vec<char> {
                chars
                    .iter()
                    .copied()
                    .filter(|&c| c as u32 % modulus == rem)
                    .collect()
            };
            match name {
                "NotoSansCJK-Regular.ttc" => vec![(0, face(5, 0)), (1, face(2, 0))],
                "NotoSansArabic-Regular.ttf" => vec![(0, vec!['م'])],
                "DejaVuSans.ttf" => vec![(0, vec!['Ж'])],
                n if n.starts_with("Other") => vec![(0, face(2, 1))],
                _ => Vec::new(),
            }
            .into_iter()
            .filter(|(_, covered)| !covered.is_empty())
            .collect()
        };
        let found = find_with(&files, &missing, &|_, _| false, &mut read);
        for (path, n) in reads.borrow().iter() {
            assert_eq!(*n, 1, "{} was read {n} times", path.display());
        }
        // The CJK collection's second face (it covers more than the first) comes before the scan's
        // finds, which pick up the odd characters it left.
        let at = |name: &str| found.iter().position(|f| f.path.ends_with(name));
        assert_eq!(found[at("NotoSansCJK-Regular.ttc").unwrap()].index, 1);
        // The collection's first face still covers characters the second doesn't: both come.
        let cjk = found
            .iter()
            .filter(|f| f.path.ends_with("NotoSansCJK-Regular.ttc"));
        assert_eq!(cjk.map(|f| f.index).collect::<Vec<_>>(), [1, 0]);
        assert!(at("NotoSansCJK-Regular.ttc") < at("Other0.ttf"));
        assert!(found.iter().any(|f| f.path.ends_with("DejaVuSans.ttf")));
        assert!(
            found
                .iter()
                .any(|f| f.path.ends_with("NotoSansArabic-Regular.ttf"))
        );
        assert!(found.iter().any(|f| f.path.ends_with("Other0.ttf")));
        // Only the files that were needed were read in the scan: it stops once nothing is missing.
        assert!(reads.borrow().len() < files.len());

        // A face that is already loaded is passed over, not read again.
        reads.borrow_mut().clear();
        let found = find_with(
            &files,
            &missing,
            &|p, _| p.ends_with("DejaVuSans.ttf"),
            &mut read,
        );
        assert!(reads.borrow().values().all(|&n| n == 1));
        assert!(!found.iter().any(|f| f.path.ends_with("DejaVuSans.ttf")));
    }

    /// A collection whose faces each cover part of what is missing gives all of them, in one
    /// read: a preferred file and one found by the scan alike, skipping the faces already loaded.
    #[test]
    fn every_useful_face_of_a_collection_is_taken() {
        let files = vec![
            PathBuf::from("/fonts/NotoSansCJK-Regular.ttc"),
            PathBuf::from("/fonts/extra/Collection.ttc"),
        ];
        let reads: RefCell<HashMap<PathBuf, usize>> = RefCell::default();
        let mut read = |path: &Path, chars: &[char]| -> Coverage {
            *reads.borrow_mut().entry(path.to_owned()).or_default() += 1;
            let covers = |list: &[char]| -> Vec<char> {
                chars.iter().copied().filter(|c| list.contains(c)).collect()
            };
            let name = path.file_name().unwrap().to_str().unwrap();
            let faces = match name {
                // Three faces each covering one character, the first two also the same one.
                "NotoSansCJK-Regular.ttc" => vec![
                    (0, covers(&['中'])),
                    (1, covers(&['文'])),
                    (2, covers(&['字'])),
                ],
                _ => vec![
                    (0, covers(&['\u{20000}', '\u{20001}'])),
                    (1, covers(&['\u{20001}'])),
                    (2, covers(&['\u{20002}'])),
                    (3, covers(&['\u{20003}'])),
                ],
            };
            faces.into_iter().filter(|(_, c)| !c.is_empty()).collect()
        };
        let indexes = |found: &[Found], name: &str| -> Vec<u32> {
            let mut v: Vec<u32> = found
                .iter()
                .filter(|f| f.path.ends_with(name))
                .map(|f| f.index)
                .collect();
            v.sort_unstable();
            v
        };
        let missing = [
            '中',
            '文',
            '字',
            '\u{20000}',
            '\u{20001}',
            '\u{20002}',
            '\u{20003}',
        ];
        let found = find_with(&files, &missing, &|_, _| false, &mut read);
        assert_eq!(indexes(&found, "NotoSansCJK-Regular.ttc"), [0, 1, 2]);
        // Faces 0, 2 and 3 cover everything; face 1 adds nothing the others don't.
        assert_eq!(indexes(&found, "Collection.ttc"), [0, 2, 3]);
        assert!(reads.borrow().values().all(|&n| n == 1), "{reads:?}");
        // A face that is loaded already is passed over, and the rest of the file still counts.
        reads.borrow_mut().clear();
        let skip = |p: &Path, i: u32| p.ends_with("NotoSansCJK-Regular.ttc") && i == 1;
        let found = find_with(&files, &missing, &skip, &mut read);
        assert_eq!(indexes(&found, "NotoSansCJK-Regular.ttc"), [0, 2]);
        assert!(reads.borrow().values().all(|&n| n == 1), "{reads:?}");
        // Faces that add nothing new are not taken, whatever the order.
        let found = find_with(&files, &['中'], &|_, _| false, &mut read);
        assert_eq!(indexes(&found, "NotoSansCJK-Regular.ttc"), [0]);
    }
}
