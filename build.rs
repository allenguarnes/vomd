//! For Windows (MSVC) only: the executable's resources, written as a `.res` file the linker
//! takes as it is, so no resource compiler or crate is needed. They are the icon (from
//! `assets/icon.ico`, also the window's and the taskbar's, see `main.rs`), the manifest
//! (per-monitor DPI awareness v2, the common controls 6 of the Open dialog, long paths) and the
//! version information Explorer shows. Elsewhere this does nothing.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" || var("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    let ico = std::fs::read("assets/icon.ico").expect("assets/icon.ico");
    let res = resources(&ico, &var("CARGO_PKG_VERSION"));
    let out = Path::new(&var("OUT_DIR")).join("vomd.res");
    std::fs::write(&out, res).expect("writing vomd.res");
    println!("cargo:rustc-link-arg-bins={}", out.display());
    // The manifest is ours: the linker must not make one of its own.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
/// English (United States), as the version information's translation says.
const LANG: u16 = 0x0409;

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="vomd" version="1.0.0.0"/>
  <dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
    <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
  </windowsSettings></application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
</assembly>
"#;

/// The `.res` file: an empty first entry, as the format begins, then every resource.
fn resources(ico: &[u8], version: &str) -> Vec<u8> {
    let mut res = Vec::new();
    entry(&mut res, 0, 0, 0, 0, &[]);
    // An .ico is a directory of images followed by them; a resource file keeps each image as an
    // icon of its own and the directory as a group naming them by ID.
    let u16_at = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(ico[at..at + 4].try_into().unwrap());
    assert!(
        u16_at(0) == 0 && u16_at(2) == 1,
        "assets/icon.ico is not an icon"
    );
    let count = u16_at(4);
    let mut group = Vec::new();
    group.extend([0, 0, 1, 0]);
    group.extend(count.to_le_bytes());
    for i in 0..count {
        let dir = 6 + 16 * usize::from(i);
        let (len, offset) = (u32_at(dir + 8), u32_at(dir + 12) as usize);
        let id = i + 1;
        entry(
            &mut res,
            RT_ICON,
            id,
            LANG,
            0x1010,
            &ico[offset..offset + len as usize],
        );
        // The same 16 bytes but the last 4: a two-byte ID instead of the file offset.
        group.extend(&ico[dir..dir + 12]);
        group.extend(id.to_le_bytes());
    }
    entry(&mut res, RT_GROUP_ICON, 1, LANG, 0x1030, &group);
    entry(&mut res, RT_MANIFEST, 1, LANG, 0x0030, MANIFEST.as_bytes());
    entry(
        &mut res,
        RT_VERSION,
        1,
        LANG,
        0x0030,
        &version_info(version),
    );
    res
}

/// One resource of type and name `kind` and `id` (both numeric), with its data, each padded to
/// four bytes.
fn entry(res: &mut Vec<u8>, kind: u16, id: u16, lang: u16, flags: u16, data: &[u8]) {
    let mut header = Vec::new();
    header.extend((data.len() as u32).to_le_bytes());
    header.extend(32u32.to_le_bytes()); // header size: the fields below make 32 bytes
    header.extend([0xff, 0xff]);
    header.extend(kind.to_le_bytes());
    header.extend([0xff, 0xff]);
    header.extend(id.to_le_bytes());
    header.extend(0u32.to_le_bytes()); // data version
    header.extend(flags.to_le_bytes());
    header.extend(lang.to_le_bytes());
    header.extend(0u32.to_le_bytes()); // version
    header.extend(0u32.to_le_bytes()); // characteristics
    res.extend(header);
    res.extend(data);
    pad(res);
}

fn pad(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A node of the version information: its length, its value's length (`value_len`, in bytes or,
/// for text, in characters), its type (1 for text), its key, its value and its children, each
/// part starting on four bytes.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(value_len.to_le_bytes());
    v.extend(u16::from(text).to_le_bytes());
    v.extend(utf16z(key));
    pad(&mut v);
    v.extend(value);
    for child in children {
        pad(&mut v);
        v.extend(child);
    }
    let len = v.len() as u16;
    v[..2].copy_from_slice(&len.to_le_bytes());
    v
}

/// `VS_VERSIONINFO` for `version` (`major.minor.patch`).
fn version_info(version: &str) -> Vec<u8> {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u16>().unwrap_or(0));
    let [major, minor, patch] = [(); 3].map(|_| parts.next().unwrap_or(0));
    let ms = (u32::from(major) << 16) | u32::from(minor);
    let ls = u32::from(patch) << 16;
    let mut fixed = Vec::new();
    for field in [
        0xFEEF_04BD, // signature
        0x0001_0000, // structure version
        ms,
        ls, // file version
        ms,
        ls,          // product version
        0x3f,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,           // subtype
        0,
        0, // date
    ] {
        fixed.extend(u32::to_le_bytes(field));
    }
    let string = |key: &str, value: &str| {
        let chars = value.encode_utf16().count() as u16 + 1;
        node(key, &utf16z(value), chars, true, &[])
    };
    let strings = [
        string("CompanyName", "Allen Guarnes"),
        string("FileDescription", "VOMD"),
        string("FileVersion", version),
        string("InternalName", "vomd"),
        string(
            "LegalCopyright",
            "Copyright (c) 2026 Allen Guarnes, MIT License",
        ),
        string("OriginalFilename", "vomd.exe"),
        string("ProductName", "VOMD (View-Only Markdown)"),
        string("ProductVersion", version),
    ];
    let table = node("040904B0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    // English (United States), Unicode.
    let translation = node("Translation", &0x04B0_0409u32.to_le_bytes(), 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[translation]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        52,
        false,
        &[string_info, var_info],
    )
}
