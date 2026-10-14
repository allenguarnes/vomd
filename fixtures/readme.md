<img src="assets/icon@2x.png" width="128" alt="VOMD icon">

# VOMD: View-Only Markdown

A tiny, fast Markdown viewer for the Linux desktop. Double-click a `.md` file and it appears,
rendered the way GitHub shows it, in well under a tenth of a second. It is one 3.7 MB binary
with its fonts built in. It doesn't use a web engine or a GUI toolkit, and it never touches
the network unless you ask it to.

- **CommonMark and GitHub Flavored Markdown**: tables, task lists, strikethrough, footnotes,
  alerts (`> [!NOTE]`), autolinks, and YAML front matter (hidden).
- **Light and dark themes** that follow the system. Zoom runs from 50% to 300%, and HiDPI
  scaling follows the system scale factor.
- **Links**: `#anchors` scroll, relative `.md` links open in the same window (Back returns),
  and web links open in your browser.
- **Images**: local PNG and JPEG, decoded lazily. Remote images load only after you press `I`.
- **Live reload**: when the window gets focus, a document that changed on disk is shown again
  at the same place.
- **Large files**: a 5 MB document opens in about 60 ms. Files over 10 MB still render, with
  a notice.

Raw HTML is never run. Tags are stripped and their text kept, and a few tags (`<b>`, `<img>`,
`<a>`, `<br>`, …) are rendered the way GitHub renders them.

## Install

**Arch Linux (AUR):** `vomd` builds from source and `vomd-bin` is the release binary:

```sh
paru -S vomd        # or: yay -S vomd-bin
```

`vomd` honors `makepkg.conf`: its `RUSTFLAGS` (CachyOS sets `-C opt-level=3 -C target-cpu=native`)
replace the size-tuned `opt-level`, so that binary can be larger (about 4.9 MB) and faster than the
release one, and runs only on the CPU it was built on. `vomd-bin` is the small, portable build.

**Release tarball:** each [release](https://github.com/allenguarnes/vomd/releases) has
`vomd-<version>-x86_64-linux.tar.gz` (glibc 2.36 or newer, any x86-64 CPU):

```sh
tar xf vomd-*-x86_64-linux.tar.gz && cd vomd-*-x86_64-linux
install -Dm755 vomd ~/.local/bin/vomd
install -Dm644 vomd.desktop ~/.local/share/applications/vomd.desktop
install -Dm644 vomd.svg ~/.local/share/icons/hicolor/scalable/apps/vomd.svg
xdg-mime default vomd.desktop text/markdown     # make it the default for .md files
```

**From source:** you need a stable Rust toolchain.

```sh
cargo build --release          # target/release/vomd
cargo build --release --no-default-features   # smallest: no images, system fonts
```

The Cargo features are `images` (PNG/JPEG), `remote-images` (via the system `curl`) and
`embed-fonts`. All three are on by default.

## Usage

```
vomd [FILE]          Open FILE (any extension is read as Markdown)
vomd -               Read Markdown from standard input
vomd                 An empty window: drop a file on it (X11)
vomd --no-settings   Ignore the settings file
vomd --help | --version
```

Each window is its own process, so opening another file starts another window.

## Keys

| Key | Action |
|---|---|
| Wheel, `↑` `↓`, `j` `k` | Scroll |
| `PgUp` `PgDn`, `Space` `Shift+Space` | Scroll a page |
| `Home` `End`, `g` `G` | Top / bottom |
| `[` `]` | Previous / next heading |
| `Ctrl` + `=` `-` `0`, `Ctrl` + wheel | Zoom in / out / reset |
| `T` | Theme: system → light → dark |
| `B` | Show / hide the toolbar |
| `W` | Wide layout on / off: the text spans the window instead of stopping at `line_width` |
| `I` | Load remote images for this document (press again to hide them) |
| `R`, `F5` | Reload |
| Click a link | Follow it; hover shows where it goes |
| `Backspace`, `Alt+←`, `Ctrl+[` | Back |
| `Esc`, `Q`, `Ctrl+W` | Quit |

The toolbar has buttons for these, and its ⚙ button opens the settings file.

## Settings

`$XDG_CONFIG_HOME/vomd/settings.ini` (usually `~/.config/vomd/settings.ini`). It is optional,
holds plain `key = value` lines, and is applied as soon as the window gets focus. ⚙ creates
it with every key listed and commented out:

| Key | Values (default first) |
|---|---|
| `theme` | `system`, `light`, `dark` |
| `zoom` | `100` (50–300) |
| `toolbar` | `true`, `false` |
| `window_width`, `window_height` | `900`, `1000` (logical pixels; saved on close) |
| `remote_images` | `ask`, `never`, `always` |
| `font_size` | `16` (10–32) |
| `line_width` | `80` (40–200 characters) |
| `wide` | `false`, `true` (the text spans the window; `line_width` is ignored) |
| `font_sans`, `font_mono` | `default` (built in), or an installed family such as `Noto Sans` |

Theme, zoom, toolbar, wide layout and window size are saved automatically when you change them.

## Privacy

VOMD makes no network requests of its own. Remote (`http`/`https`) images show as
placeholders until you press `I` (or click the toolbar's image button) for that document.
They are then fetched with the system `curl`, at most 4 at a time, limited to 12 MiB and
15 seconds each, and kept in memory only. `remote_images = always` loads them for every
document without asking, which lets any document you open contact the servers its images are
on. `never` removes the option.

## Limitations

- **Linux only** for now. The code is portable and compiles for Windows and macOS, but those
  are untested and unsupported until a later release.
- No text selection or copy yet (v1.1), and no table of contents panel (v1.2).
- No screen-reader support (AccessKit would add too much to the binary). Text scaling comes
  from the system scale factor that winit reports (`Xft.dpi` on X11, the output scale on
  Wayland).
- Drag-and-drop works on X11 only, because winit doesn't support it on Wayland.
- SVG, GIF and WebP images show as placeholders. EXIF orientation isn't applied. Math and
  Mermaid show as text and code.

## Development

`just ci` runs formatting, clippy (every feature set), the tests, and Windows/macOS compile
checks. `cargo xtask budgets` measures every performance budget against a release build (it
needs a display). `just fuzz <doc|layout|settings> [seconds]` fuzzes on the stable toolchain.
`scripts/aur-check.sh` builds and checks both AUR packages offline.

## License

MIT. The embedded fonts are Inter and JetBrains Mono (SIL OFL 1.1) and Lucide icons (ISC);
their licenses are in `assets/fonts/`.

---

# Changelog

All notable changes to VOMD. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [1.0.0] - Unreleased

The first release, for Linux (x86-64; Windows and macOS compile but are unsupported).

### Added
- CommonMark and GitHub Flavored Markdown: tables, task lists, strikethrough, footnotes,
  alerts, extended autolinks, hidden YAML front matter. Raw HTML is stripped, never run; a
  few tags are styled as on GitHub.
- Our own text layout and renderer on `swash`, `winit` and `softbuffer`, with embedded Inter
  and JetBrains Mono subsets and a lazy system-font fallback for other scripts and emoji.
- Light and dark themes following the system, zoom 50–300%, HiDPI.
- Links: anchors, relative Markdown files in the same window with Back, web links in the
  browser; a URL preview on hover.
- Reload on `R`/`F5` and when the window regains focus, keeping the reading position.
- A slim toolbar (hidden with `B`) with an overflow menu on narrow windows.
- `settings.ini` with live apply, written back without losing comments.
- Local PNG and JPEG images, decoded lazily at their displayed size under a 32 MB cache.
- Remote images, off by default: loaded per document with `I`, through the system `curl`.
- A one-line notice for files over 10 MB, which still render.
- Packaging: AUR `vomd` and `vomd-bin`, a release tarball with a desktop entry, icon and the
  README's image.
- The app icon (an eye over the Markdown mark). The window's Wayland app ID and X11 `WM_CLASS` are
  `vomd`, so the desktop shows it for the window.
- The system theme comes from the desktop portal's `color-scheme` (Plasma's and GNOME's dark
  mode, followed live, and back to winit and `GTK_THEME` when the preference is withdrawn), which
  say nothing on Wayland. Only the portal's own broadcasts count (not a signal another client addresses to VOMD), none
  is missed while starting (the first read is reported at once; signals are followed in
  order, and one that races a read triggers a backed-off re-read, 50 ms doubling to 5 s, until a
  read completes undisturbed), and the bus going away also returns to winit and `GTK_THEME`.
- Wide layout: `W`, the toolbar's button (next to the theme's) or `wide = true` in
  `settings.ini` lets the text span the window instead of stopping at `line_width`; it is saved.
  A change of the column's width (this, or resizing the window) rescales every height, until
  its block is measured again, by how its estimate changed, so the scrollbar is right while
  measuring goes on; the estimates read what the prepass stored (20 bytes per block), not the
  text, so this costs two to three times the plain remeasure (0.5 ms on a 42,000-block
  document).
- The scrollbar also shows while the pointer is over its column.
- Budgets checked in CI (`cargo xtask budgets`): binary size, cold and warm start, idle RSS,
  idle CPU, 4K scroll frame time. Fuzz targets for the parser, layout and settings.

[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0

---

<img src="assets/icon@2x.png" width="128" alt="VOMD icon">

# VOMD: View-Only Markdown

A tiny, fast Markdown viewer for the Linux desktop. Double-click a `.md` file and it appears,
rendered the way GitHub shows it, in well under a tenth of a second. It is one 3.7 MB binary
with its fonts built in. It doesn't use a web engine or a GUI toolkit, and it never touches
the network unless you ask it to.

- **CommonMark and GitHub Flavored Markdown**: tables, task lists, strikethrough, footnotes,
  alerts (`> [!NOTE]`), autolinks, and YAML front matter (hidden).
- **Light and dark themes** that follow the system. Zoom runs from 50% to 300%, and HiDPI
  scaling follows the system scale factor.
- **Links**: `#anchors` scroll, relative `.md` links open in the same window (Back returns),
  and web links open in your browser.
- **Images**: local PNG and JPEG, decoded lazily. Remote images load only after you press `I`.
- **Live reload**: when the window gets focus, a document that changed on disk is shown again
  at the same place.
- **Large files**: a 5 MB document opens in about 60 ms. Files over 10 MB still render, with
  a notice.

Raw HTML is never run. Tags are stripped and their text kept, and a few tags (`<b>`, `<img>`,
`<a>`, `<br>`, …) are rendered the way GitHub renders them.

## Install

**Arch Linux (AUR):** `vomd` builds from source and `vomd-bin` is the release binary:

```sh
paru -S vomd        # or: yay -S vomd-bin
```

`vomd` honors `makepkg.conf`: its `RUSTFLAGS` (CachyOS sets `-C opt-level=3 -C target-cpu=native`)
replace the size-tuned `opt-level`, so that binary can be larger (about 4.9 MB) and faster than the
release one, and runs only on the CPU it was built on. `vomd-bin` is the small, portable build.

**Release tarball:** each [release](https://github.com/allenguarnes/vomd/releases) has
`vomd-<version>-x86_64-linux.tar.gz` (glibc 2.36 or newer, any x86-64 CPU):

```sh
tar xf vomd-*-x86_64-linux.tar.gz && cd vomd-*-x86_64-linux
install -Dm755 vomd ~/.local/bin/vomd
install -Dm644 vomd.desktop ~/.local/share/applications/vomd.desktop
install -Dm644 vomd.svg ~/.local/share/icons/hicolor/scalable/apps/vomd.svg
xdg-mime default vomd.desktop text/markdown     # make it the default for .md files
```

**From source:** you need a stable Rust toolchain.

```sh
cargo build --release          # target/release/vomd
cargo build --release --no-default-features   # smallest: no images, system fonts
```

The Cargo features are `images` (PNG/JPEG), `remote-images` (via the system `curl`) and
`embed-fonts`. All three are on by default.

## Usage

```
vomd [FILE]          Open FILE (any extension is read as Markdown)
vomd -               Read Markdown from standard input
vomd                 An empty window: drop a file on it (X11)
vomd --no-settings   Ignore the settings file
vomd --help | --version
```

Each window is its own process, so opening another file starts another window.

## Keys

| Key | Action |
|---|---|
| Wheel, `↑` `↓`, `j` `k` | Scroll |
| `PgUp` `PgDn`, `Space` `Shift+Space` | Scroll a page |
| `Home` `End`, `g` `G` | Top / bottom |
| `[` `]` | Previous / next heading |
| `Ctrl` + `=` `-` `0`, `Ctrl` + wheel | Zoom in / out / reset |
| `T` | Theme: system → light → dark |
| `B` | Show / hide the toolbar |
| `W` | Wide layout on / off: the text spans the window instead of stopping at `line_width` |
| `I` | Load remote images for this document (press again to hide them) |
| `R`, `F5` | Reload |
| Click a link | Follow it; hover shows where it goes |
| `Backspace`, `Alt+←`, `Ctrl+[` | Back |
| `Esc`, `Q`, `Ctrl+W` | Quit |

The toolbar has buttons for these, and its ⚙ button opens the settings file.

## Settings

`$XDG_CONFIG_HOME/vomd/settings.ini` (usually `~/.config/vomd/settings.ini`). It is optional,
holds plain `key = value` lines, and is applied as soon as the window gets focus. ⚙ creates
it with every key listed and commented out:

| Key | Values (default first) |
|---|---|
| `theme` | `system`, `light`, `dark` |
| `zoom` | `100` (50–300) |
| `toolbar` | `true`, `false` |
| `window_width`, `window_height` | `900`, `1000` (logical pixels; saved on close) |
| `remote_images` | `ask`, `never`, `always` |
| `font_size` | `16` (10–32) |
| `line_width` | `80` (40–200 characters) |
| `wide` | `false`, `true` (the text spans the window; `line_width` is ignored) |
| `font_sans`, `font_mono` | `default` (built in), or an installed family such as `Noto Sans` |

Theme, zoom, toolbar, wide layout and window size are saved automatically when you change them.

## Privacy

VOMD makes no network requests of its own. Remote (`http`/`https`) images show as
placeholders until you press `I` (or click the toolbar's image button) for that document.
They are then fetched with the system `curl`, at most 4 at a time, limited to 12 MiB and
15 seconds each, and kept in memory only. `remote_images = always` loads them for every
document without asking, which lets any document you open contact the servers its images are
on. `never` removes the option.

## Limitations

- **Linux only** for now. The code is portable and compiles for Windows and macOS, but those
  are untested and unsupported until a later release.
- No text selection or copy yet (v1.1), and no table of contents panel (v1.2).
- No screen-reader support (AccessKit would add too much to the binary). Text scaling comes
  from the system scale factor that winit reports (`Xft.dpi` on X11, the output scale on
  Wayland).
- Drag-and-drop works on X11 only, because winit doesn't support it on Wayland.
- SVG, GIF and WebP images show as placeholders. EXIF orientation isn't applied. Math and
  Mermaid show as text and code.

## Development

`just ci` runs formatting, clippy (every feature set), the tests, and Windows/macOS compile
checks. `cargo xtask budgets` measures every performance budget against a release build (it
needs a display). `just fuzz <doc|layout|settings> [seconds]` fuzzes on the stable toolchain.
`scripts/aur-check.sh` builds and checks both AUR packages offline.

## License

MIT. The embedded fonts are Inter and JetBrains Mono (SIL OFL 1.1) and Lucide icons (ISC);
their licenses are in `assets/fonts/`.

---

# Changelog

All notable changes to VOMD. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [1.0.0] - Unreleased

The first release, for Linux (x86-64; Windows and macOS compile but are unsupported).

### Added
- CommonMark and GitHub Flavored Markdown: tables, task lists, strikethrough, footnotes,
  alerts, extended autolinks, hidden YAML front matter. Raw HTML is stripped, never run; a
  few tags are styled as on GitHub.
- Our own text layout and renderer on `swash`, `winit` and `softbuffer`, with embedded Inter
  and JetBrains Mono subsets and a lazy system-font fallback for other scripts and emoji.
- Light and dark themes following the system, zoom 50–300%, HiDPI.
- Links: anchors, relative Markdown files in the same window with Back, web links in the
  browser; a URL preview on hover.
- Reload on `R`/`F5` and when the window regains focus, keeping the reading position.
- A slim toolbar (hidden with `B`) with an overflow menu on narrow windows.
- `settings.ini` with live apply, written back without losing comments.
- Local PNG and JPEG images, decoded lazily at their displayed size under a 32 MB cache.
- Remote images, off by default: loaded per document with `I`, through the system `curl`.
- A one-line notice for files over 10 MB, which still render.
- Packaging: AUR `vomd` and `vomd-bin`, a release tarball with a desktop entry, icon and the
  README's image.
- The app icon (an eye over the Markdown mark). The window's Wayland app ID and X11 `WM_CLASS` are
  `vomd`, so the desktop shows it for the window.
- The system theme comes from the desktop portal's `color-scheme` (Plasma's and GNOME's dark
  mode, followed live, and back to winit and `GTK_THEME` when the preference is withdrawn), which
  say nothing on Wayland. Only the portal's own broadcasts count (not a signal another client addresses to VOMD), none
  is missed while starting (the first read is reported at once; signals are followed in
  order, and one that races a read triggers a backed-off re-read, 50 ms doubling to 5 s, until a
  read completes undisturbed), and the bus going away also returns to winit and `GTK_THEME`.
- Wide layout: `W`, the toolbar's button (next to the theme's) or `wide = true` in
  `settings.ini` lets the text span the window instead of stopping at `line_width`; it is saved.
  A change of the column's width (this, or resizing the window) rescales every height, until
  its block is measured again, by how its estimate changed, so the scrollbar is right while
  measuring goes on; the estimates read what the prepass stored (20 bytes per block), not the
  text, so this costs two to three times the plain remeasure (0.5 ms on a 42,000-block
  document).
- The scrollbar also shows while the pointer is over its column.
- Budgets checked in CI (`cargo xtask budgets`): binary size, cold and warm start, idle RSS,
  idle CPU, 4K scroll frame time. Fuzz targets for the parser, layout and settings.

[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0

---

<img src="assets/icon@2x.png" width="128" alt="VOMD icon">

# VOMD: View-Only Markdown

A tiny, fast Markdown viewer for the Linux desktop. Double-click a `.md` file and it appears,
rendered the way GitHub shows it, in well under a tenth of a second. It is one 3.7 MB binary
with its fonts built in. It doesn't use a web engine or a GUI toolkit, and it never touches
the network unless you ask it to.

- **CommonMark and GitHub Flavored Markdown**: tables, task lists, strikethrough, footnotes,
  alerts (`> [!NOTE]`), autolinks, and YAML front matter (hidden).
- **Light and dark themes** that follow the system. Zoom runs from 50% to 300%, and HiDPI
  scaling follows the system scale factor.
- **Links**: `#anchors` scroll, relative `.md` links open in the same window (Back returns),
  and web links open in your browser.
- **Images**: local PNG and JPEG, decoded lazily. Remote images load only after you press `I`.
- **Live reload**: when the window gets focus, a document that changed on disk is shown again
  at the same place.
- **Large files**: a 5 MB document opens in about 60 ms. Files over 10 MB still render, with
  a notice.

Raw HTML is never run. Tags are stripped and their text kept, and a few tags (`<b>`, `<img>`,
`<a>`, `<br>`, …) are rendered the way GitHub renders them.

## Install

**Arch Linux (AUR):** `vomd` builds from source and `vomd-bin` is the release binary:

```sh
paru -S vomd        # or: yay -S vomd-bin
```

`vomd` honors `makepkg.conf`: its `RUSTFLAGS` (CachyOS sets `-C opt-level=3 -C target-cpu=native`)
replace the size-tuned `opt-level`, so that binary can be larger (about 4.9 MB) and faster than the
release one, and runs only on the CPU it was built on. `vomd-bin` is the small, portable build.

**Release tarball:** each [release](https://github.com/allenguarnes/vomd/releases) has
`vomd-<version>-x86_64-linux.tar.gz` (glibc 2.36 or newer, any x86-64 CPU):

```sh
tar xf vomd-*-x86_64-linux.tar.gz && cd vomd-*-x86_64-linux
install -Dm755 vomd ~/.local/bin/vomd
install -Dm644 vomd.desktop ~/.local/share/applications/vomd.desktop
install -Dm644 vomd.svg ~/.local/share/icons/hicolor/scalable/apps/vomd.svg
xdg-mime default vomd.desktop text/markdown     # make it the default for .md files
```

**From source:** you need a stable Rust toolchain.

```sh
cargo build --release          # target/release/vomd
cargo build --release --no-default-features   # smallest: no images, system fonts
```

The Cargo features are `images` (PNG/JPEG), `remote-images` (via the system `curl`) and
`embed-fonts`. All three are on by default.

## Usage

```
vomd [FILE]          Open FILE (any extension is read as Markdown)
vomd -               Read Markdown from standard input
vomd                 An empty window: drop a file on it (X11)
vomd --no-settings   Ignore the settings file
vomd --help | --version
```

Each window is its own process, so opening another file starts another window.

## Keys

| Key | Action |
|---|---|
| Wheel, `↑` `↓`, `j` `k` | Scroll |
| `PgUp` `PgDn`, `Space` `Shift+Space` | Scroll a page |
| `Home` `End`, `g` `G` | Top / bottom |
| `[` `]` | Previous / next heading |
| `Ctrl` + `=` `-` `0`, `Ctrl` + wheel | Zoom in / out / reset |
| `T` | Theme: system → light → dark |
| `B` | Show / hide the toolbar |
| `W` | Wide layout on / off: the text spans the window instead of stopping at `line_width` |
| `I` | Load remote images for this document (press again to hide them) |
| `R`, `F5` | Reload |
| Click a link | Follow it; hover shows where it goes |
| `Backspace`, `Alt+←`, `Ctrl+[` | Back |
| `Esc`, `Q`, `Ctrl+W` | Quit |

The toolbar has buttons for these, and its ⚙ button opens the settings file.

## Settings

`$XDG_CONFIG_HOME/vomd/settings.ini` (usually `~/.config/vomd/settings.ini`). It is optional,
holds plain `key = value` lines, and is applied as soon as the window gets focus. ⚙ creates
it with every key listed and commented out:

| Key | Values (default first) |
|---|---|
| `theme` | `system`, `light`, `dark` |
| `zoom` | `100` (50–300) |
| `toolbar` | `true`, `false` |
| `window_width`, `window_height` | `900`, `1000` (logical pixels; saved on close) |
| `remote_images` | `ask`, `never`, `always` |
| `font_size` | `16` (10–32) |
| `line_width` | `80` (40–200 characters) |
| `wide` | `false`, `true` (the text spans the window; `line_width` is ignored) |
| `font_sans`, `font_mono` | `default` (built in), or an installed family such as `Noto Sans` |

Theme, zoom, toolbar, wide layout and window size are saved automatically when you change them.

## Privacy

VOMD makes no network requests of its own. Remote (`http`/`https`) images show as
placeholders until you press `I` (or click the toolbar's image button) for that document.
They are then fetched with the system `curl`, at most 4 at a time, limited to 12 MiB and
15 seconds each, and kept in memory only. `remote_images = always` loads them for every
document without asking, which lets any document you open contact the servers its images are
on. `never` removes the option.

## Limitations

- **Linux only** for now. The code is portable and compiles for Windows and macOS, but those
  are untested and unsupported until a later release.
- No text selection or copy yet (v1.1), and no table of contents panel (v1.2).
- No screen-reader support (AccessKit would add too much to the binary). Text scaling comes
  from the system scale factor that winit reports (`Xft.dpi` on X11, the output scale on
  Wayland).
- Drag-and-drop works on X11 only, because winit doesn't support it on Wayland.
- SVG, GIF and WebP images show as placeholders. EXIF orientation isn't applied. Math and
  Mermaid show as text and code.

## Development

`just ci` runs formatting, clippy (every feature set), the tests, and Windows/macOS compile
checks. `cargo xtask budgets` measures every performance budget against a release build (it
needs a display). `just fuzz <doc|layout|settings> [seconds]` fuzzes on the stable toolchain.
`scripts/aur-check.sh` builds and checks both AUR packages offline.

## License

MIT. The embedded fonts are Inter and JetBrains Mono (SIL OFL 1.1) and Lucide icons (ISC);
their licenses are in `assets/fonts/`.

---

# Changelog

All notable changes to VOMD. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [1.0.0] - Unreleased

The first release, for Linux (x86-64; Windows and macOS compile but are unsupported).

### Added
- CommonMark and GitHub Flavored Markdown: tables, task lists, strikethrough, footnotes,
  alerts, extended autolinks, hidden YAML front matter. Raw HTML is stripped, never run; a
  few tags are styled as on GitHub.
- Our own text layout and renderer on `swash`, `winit` and `softbuffer`, with embedded Inter
  and JetBrains Mono subsets and a lazy system-font fallback for other scripts and emoji.
- Light and dark themes following the system, zoom 50–300%, HiDPI.
- Links: anchors, relative Markdown files in the same window with Back, web links in the
  browser; a URL preview on hover.
- Reload on `R`/`F5` and when the window regains focus, keeping the reading position.
- A slim toolbar (hidden with `B`) with an overflow menu on narrow windows.
- `settings.ini` with live apply, written back without losing comments.
- Local PNG and JPEG images, decoded lazily at their displayed size under a 32 MB cache.
- Remote images, off by default: loaded per document with `I`, through the system `curl`.
- A one-line notice for files over 10 MB, which still render.
- Packaging: AUR `vomd` and `vomd-bin`, a release tarball with a desktop entry, icon and the
  README's image.
- The app icon (an eye over the Markdown mark). The window's Wayland app ID and X11 `WM_CLASS` are
  `vomd`, so the desktop shows it for the window.
- The system theme comes from the desktop portal's `color-scheme` (Plasma's and GNOME's dark
  mode, followed live, and back to winit and `GTK_THEME` when the preference is withdrawn), which
  say nothing on Wayland. Only the portal's own broadcasts count (not a signal another client addresses to VOMD), none
  is missed while starting (the first read is reported at once; signals are followed in
  order, and one that races a read triggers a backed-off re-read, 50 ms doubling to 5 s, until a
  read completes undisturbed), and the bus going away also returns to winit and `GTK_THEME`.
- Wide layout: `W`, the toolbar's button (next to the theme's) or `wide = true` in
  `settings.ini` lets the text span the window instead of stopping at `line_width`; it is saved.
  A change of the column's width (this, or resizing the window) rescales every height, until
  its block is measured again, by how its estimate changed, so the scrollbar is right while
  measuring goes on; the estimates read what the prepass stored (20 bytes per block), not the
  text, so this costs two to three times the plain remeasure (0.5 ms on a 42,000-block
  document).
- The scrollbar also shows while the pointer is over its column.
- Budgets checked in CI (`cargo xtask budgets`): binary size, cold and warm start, idle RSS,
  idle CPU, 4K scroll frame time. Fuzz targets for the parser, layout and settings.

[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0
