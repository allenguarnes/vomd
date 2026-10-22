<img src="assets/icon@2x.png" width="128" alt="VOMD icon">

# VOMD: View-Only Markdown

A tiny, fast Markdown viewer for Linux and Windows. Double-click a `.md` file and it appears,
rendered the way GitHub shows it, in well under a tenth of a second. It is one binary with its
fonts built in, 5.5 MB on Linux and 4.3 MB on Windows. It doesn't use a web engine or a GUI
toolkit, and it never touches the network unless you ask it to.

- **CommonMark and GitHub Flavored Markdown**: tables, task lists, strikethrough, footnotes,
  alerts (`> [!NOTE]`), autolinks, and YAML front matter (hidden).
- **Light and dark themes** that follow the system. Zoom runs from 50% to 300%, and HiDPI
  scaling follows the system scale factor.
- **Links**: `#anchors` scroll, relative `.md` links open in the same window (Back returns),
  and web links open in your browser.
- **Images**: PNG, JPEG, GIF, WebP and SVG, local or remote, decoded lazily and drawn at the
  screen's resolution (SVGs stay sharp at any zoom); photos are turned upright by their EXIF
  orientation, and `<img width height>` is honored. Remote images load only after you press `I`.
  An image too large or complex for VOMD's limits shows its alt text with *click to load*;
  clicking loads it anyway (Ctrl+click when the image is a link). Loading it can use a lot of
  memory and time (an SVG stops after 3 seconds, but a clicked one runs until it is done or has
  used 120 seconds of CPU), and it's abandoned if you leave the page.
- **Selecting and copying text**: drag, double-click a word, triple-click a block, or press
  `Ctrl+A`; `Ctrl+C` copies it as plain text, and on X11 and Wayland what you select is also
  pasted by a middle click. A code block shows a copy button, as on GitHub, when the pointer is
  over it.
- **Contents panel**: `C` shows the headings in a panel at the left, with the current one
  marked; a click jumps to a heading.
- **Find**: `Ctrl+F` opens a bar below the toolbar, and every match of what you type is
  marked in the text as you type it, with a count and `Enter` to step through them.
- **Live reload**: when the window gets focus, a document that changed on disk is shown again
  at the same place.
- **Large files**: a 5 MB document opens in under 100 ms. Files over 10 MB still render, with
  a notice.

Raw HTML is never run. Tags are stripped and their text kept, and a few tags (`<b>`, `<img>`,
`<a>`, `<br>`, …) are rendered the way GitHub renders them.

## Install

**Arch Linux:** the packages aren't on the AUR yet, so build one from a clone with `makepkg`:

```sh
git clone https://github.com/allenguarnes/vomd
cd vomd/packaging/aur/vomd-bin
makepkg -si
```

`vomd-bin` installs the release binary, which is small and runs on any x86-64 CPU. To build from
source instead, use `packaging/aur/vomd`. It follows your `makepkg.conf`, so with CachyOS's
`RUSTFLAGS` (`-C opt-level=3 -C target-cpu=native`) the binary is larger (about 7.4 MB) and
faster, and runs only on the CPU it was built on.

**Debian, Ubuntu:** from v1.1.0, each [release](https://github.com/allenguarnes/vomd/releases)
has a `.deb` for Debian 12 or newer and Ubuntu 24.04 or newer (x86-64):

```sh
sudo apt install ./vomd_*_amd64.deb
```

**AppImage:** from v1.1.0, each release also has `vomd-<version>-x86_64.AppImage`, for any
x86-64 distribution with glibc 2.36 or newer. It bundles no libraries: it loads the system's
Wayland or X11 ones when it starts (see the run-time libraries below):

```sh
chmod +x vomd-*-x86_64.AppImage
./vomd-*-x86_64.AppImage README.md
```

It mounts itself with FUSE, which most desktops have (`fuse3`). Without it, run it with
`--appimage-extract-and-run`. An AppImage doesn't register itself as the app for `.md`
files; an integration tool such as Gear Lever or AppImageLauncher can.

**Windows:** from v1.6.0, each [release](https://github.com/allenguarnes/vomd/releases) has
`vomd-<version>-x86_64-windows-portable.zip`, for Windows 10 (1803) or newer and Windows 11 on
x86-64. It holds `vomd.exe`, which needs nothing else installed.

> [!IMPORTANT]
> **`vomd.exe` isn't code-signed yet, so Windows will probably warn you the first time you run
> it.** This is expected and doesn't mean the file is harmful. Microsoft Defender SmartScreen
> shows "Windows protected your PC" for programs from a publisher it doesn't know yet, and every
> unsigned program starts out that way. To run VOMD, click **More info**, then **Run anyway**.
> Windows remembers the choice, so you see this only once.
>
> The zip is built and uploaded by the public
> [release workflow](https://github.com/allenguarnes/vomd/actions/workflows/release.yml) from the
> tagged source, and each one has a `.sha256` file next to it. To check that your download
> matches, run `certutil -hashfile vomd-<version>-x86_64-windows-portable.zip SHA256` in a
> terminal and compare the result with that file. Signed builds and an installer are planned.

1. Unzip it anywhere (your user folder, for example).
2. Run `vomd.exe`, or drop a `.md` file on it.
3. To make VOMD the app for `.md` files: right-click a `.md` file, choose **Open with**, then
   **Choose another app**, find `vomd.exe` with **Choose an app on your PC**, and select
   **Always**.

VOMD itself never writes to the registry, so to remove it, delete the folder (and
`%APPDATA%\vomd` if you made a settings file).

**Release tarball:** each [release](https://github.com/allenguarnes/vomd/releases) has
`vomd-<version>-x86_64-linux.tar.gz` (glibc 2.36 or newer, any x86-64 CPU). It needs the same
run-time libraries as the AppImage (see below):

```sh
tar xf vomd-*-x86_64-linux.tar.gz
cd vomd-*-x86_64-linux
install -Dm755 vomd ~/.local/bin/vomd
install -Dm644 vomd.desktop ~/.local/share/applications/vomd.desktop
install -Dm644 vomd.svg ~/.local/share/icons/hicolor/scalable/apps/vomd.svg
# Make it the default for .md files:
xdg-mime default vomd.desktop text/markdown
```

**Run-time libraries:** the AppImage and the tarball binary load these system libraries when
they start: libwayland-client and libxkbcommon on Wayland, and libX11, libX11-xcb, libxcb,
libXcursor, libXi, libxkbcommon and libxkbcommon-x11 on X11. Most desktops already have them,
but a minimal one may not. `libxkbcommon-x11` is the one most often missing, and without it
VOMD stops at once with "Library libxkbcommon-x11.so could not be loaded". On Debian and
Ubuntu, install them all with (the `.deb` already depends on them):

```sh
sudo apt install libwayland-client0 libxkbcommon0 \
  libxkbcommon-x11-0 libx11-6 libx11-xcb1 libxcb1 \
  libxcursor1 libxi6
```

**From source:** you need a stable Rust toolchain, and on Windows the Visual Studio Build Tools
(the "Desktop development with C++" workload) for the MSVC linker.

```sh
# Builds target/release/vomd (vomd.exe on Windows):
cargo build --release
# Without SVG (SVGs show their alt text):
cargo build --release --no-default-features --features images,remote-images,embed-fonts,clipboard,toc,find
# The smallest build, with no images and system fonts only:
cargo build --release --no-default-features
```

The Cargo features are `images` (PNG, JPEG, GIF, WebP), `svg` (SVG and svgz, with `images`),
`remote-images` (via the system `curl`), `embed-fonts`, `clipboard` (selecting and copying
text), `toc` (the contents panel) and `find` (find in page). All seven are on by default. On
Linux the build without SVG is 4.1 MB, and the smallest one 3.0 MB; on Windows the smallest one
is 1.9 MB.

## Usage

```
vomd [FILE]          Open FILE (any extension is read as Markdown)
vomd -               Read Markdown from standard input
vomd                 An empty window: Ctrl+O opens a file
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
| `C` | Show / hide the contents panel |
| `Ctrl+F` | Find in the document: open the find bar, or close it when it is open |
| `Enter` `Shift+Enter`, `F3` `Shift+F3`, `Ctrl+G` `Ctrl+Shift+G` | In the find bar: next / previous match (`Enter` only while the field has the keyboard) |
| `←` `→`, `Ctrl+←` `Ctrl+→`, `Home` `End` | In the find bar's field: move the caret by a character, by a word, to an end; `Shift` selects |
| `Ctrl+A`, `Ctrl+C` `Ctrl+X` `Ctrl+V` | In the find bar's field: select all, copy, cut, paste (`Ctrl+Insert`, `Shift+Insert` too) |
| `W` | Wide layout on / off: the text spans the window instead of stopping at `line_width` |
| `I` | Load remote images for this document (press again to hide them) |
| `R`, `F5` | Reload |
| Click a link | Follow it; hover shows where it goes |
| Drag | Select text (a drag that starts on a link selects, and doesn't follow it) |
| Double-click, triple-click | Select a word, a paragraph (or a heading, code block, table row) |
| `Shift` + click | Extend the selection |
| Click an image's placeholder marked *click to load* | Load it beyond VOMD's limits (`Ctrl` + click when the image is a link) |
| Click the copy button of a code block | Copy the block's code (the button shows at the block's upper right while the pointer is over it) |
| `Ctrl+A` | Select everything |
| `Ctrl+C`, `Ctrl+Insert` | Copy the selection |
| `Backspace`, `Alt+←`, `Ctrl+[` | Back |
| `Ctrl+O` | Open a file (the desktop's file dialog, or the Windows one) |
| `Esc` | Close the open menu, else the find bar if it is open, else the contents panel if it covers the document, else clear the selection, else quit |
| `Q`, `Ctrl+W` | Quit |

**Copying.** `Ctrl+C` copies the selected text to the clipboard as plain text: the text as it
shows, with `- ` before list items, `> ` before quoted lines, a tab between table cells, an image's
alt text, and a link's text (not its address). Text that is selected with the mouse (and
`Ctrl+A`) is also offered as the primary selection, which a middle click pastes in programs that
use it (see `primary_selection` below). A long selection is made only when something pastes it,
so copying all of a very large document costs nothing until then. Dragging past the top or bottom
of the window scrolls. On X11 a selection goes with the window that owns it, so as the window
closes VOMD asks the clipboard manager (the desktop's, if it runs one) to keep the clipboard; on
Wayland the compositor's own manager does that. Windows has no primary selection; there, a copy
is made for the clipboard to keep when the window closes, and its lines end in CR LF.

**Copying a code block.** While the pointer is over a code block, a copy button shows in its
upper right corner, as on GitHub, and a click copies the block's code to the clipboard (not to
the primary selection, and the selection is left as it was). The code is as written: with its
tabs, indentation and blank lines, without the language tag, and without the quote marks or list
indent of what holds the block, and without a line break after the last line. The button then
shows a check mark for two seconds. A block too small for the button has none, and a block
taller than the window keeps its button at its top, where it scrolls away with the block. If
there is no clipboard to copy to, a note in the window says so. The button needs the pointer;
`Ctrl+A` and `Ctrl+C` are the keyboard's way to copy.

**Contents.** `C` (or the toolbar's list button) shows a panel at the window's left with the
document's headings, h1 to h6, indented by level; the heading you are reading is marked, and the
panel follows it as you scroll. A click on a heading jumps to it, as a link to its `#anchor` does,
so Back returns. The wheel over the panel scrolls it and not the document. In a window at least
740 logical pixels wide the panel sits beside the document, which is laid out in the room that
is left, keeping your place; in a narrower one it covers the document's edge, and a click on a
heading, a click on the document or `Esc` closes it. A document without headings says so.
Headings inside footnotes, and headings with no text, are not listed. The panel's size
follows the display's scale and not the zoom.

**Find.** `Ctrl+F` (or the toolbar's search button) opens a bar below the toolbar, with the text
you had selected as what to look for if that was one short line, and `Ctrl+F` or the button again
closes it. Every match in the text shows, in yellow, and the current one in orange, with its place
among them in the bar ("3 / 17", or "No matches"); the first one from where you are reading is
the current one, and the document scrolls to put it about a third of the way down unless it is in
view already. `Enter` (or `F3`, `Ctrl+G`, or the bar's ˅ button) goes to the next match and
`Shift+Enter` (the ˄ button) to the previous one, around the ends; the wheel and the scroll keys
move the page and leave the current match as it is. Case does not matter, and only the text you
read is searched, never a link's address or the markup; a match never runs from one paragraph,
heading or table cell into another. Every match is counted, but in a text with more than 10,000
only the 10,000 around the current one are marked.

What to look for is typed into a one-line text field, which has the keyboard while the bar is
open, so `Q`, `C` and the other letter keys are text, and the up and down arrows and the page keys
still scroll the document. The caret goes anywhere, by the arrows, by words with `Ctrl`, to the
ends with `Home` and `End`, or with a click; `Shift` with these, a drag, a double click (a word)
or a triple click selects, and `Ctrl+A` selects all. What is typed, committed by an input method
or pasted replaces the selection; `Backspace` and `Delete` delete it or a character, and with
`Ctrl` a word. The text scrolls to keep the caret in view, and a drag past either end of the field
scrolls it. `Ctrl+C` (or `Ctrl+Insert`) copies the selected text, `Ctrl+X` cuts it, and `Ctrl+V`
(or `Shift+Insert`) pastes the clipboard at the caret; a selection made in the field is also
offered as the primary selection (see `primary_selection` below), which a middle click on the field
pastes at the click; that reads the primary selection whatever the setting says, for it governs
only what VOMD offers. A paste is its first line only, with tabs as spaces and no other control
character, and what the query holds is at most 256 characters. The paste is read in the
background, so a program that is slow to answer, or none at all, never holds the window up; it is
given up on after about two seconds and pastes nothing. A click on the document gives the keyboard
back to the document: the bar stays open and the matches stay marked, the letter keys, `Ctrl+C`
and `Ctrl+A` are the document's again, and `F3` and `Ctrl+G` (with `Shift`, back) still step, while
`Enter` is left to the document. A click on the field, or closing the bar and opening it again,
gives the field the keyboard. `Esc` or the bar's ✕ closes the bar, whoever has the keyboard, and
clears the marks; the query is kept, shown selected, for the next `Ctrl+F`. A reload keeps the bar
open and searches the new text again. The bar is as high as the toolbar and follows the display's
scale and not the zoom.

The toolbar has buttons for these, and its ⚙ button opens the settings file. Open uses the
desktop portal's file chooser, or zenity or kdialog when there is none; the `.deb` recommends
one of them, the Arch packages list them as optional dependencies, and the AppImage needs one
on a desktop without a portal chooser (a bare window manager). On Windows it uses the system's
Open dialog, and ⚙ opens the file in the app set for `.ini` files (Notepad unless you changed
it).

## Settings

`$XDG_CONFIG_HOME/vomd/settings.ini` (usually `~/.config/vomd/settings.ini`), or
`%APPDATA%\vomd\settings.ini` on Windows. It is optional,
holds plain `key = value` lines, and is applied as soon as the window gets focus. ⚙ creates
it with every key listed and commented out:

| Key | Values (default first) |
|---|---|
| `theme` | `system`, `light`, `dark` |
| `zoom` | `100` (50–300) |
| `toolbar` | `true`, `false` |
| `toc` | `false`, `true` (the contents panel; `C` toggles it) |
| `window_width`, `window_height` | `900`, `1000` (logical pixels; saved on close) |
| `remote_images` | `ask`, `never`, `always` |
| `font_size` | `16` (10–32) |
| `line_width` | `80` (40–200 characters) |
| `wide` | `false`, `true` (the text spans the window; `line_width` is ignored) |
| `primary_selection` | `on`, `off` (X11 and Wayland: selecting text offers it for a middle click; `Ctrl+C` is not affected, and neither is pasting with a middle click in the find bar's field) |
| `font_sans`, `font_mono` | `default` (built in), or an installed family such as `Noto Sans` |

Theme, zoom, toolbar, contents panel, wide layout and window size are saved automatically when you
change them.

## Privacy

VOMD makes no network requests of its own. Remote (`http`/`https`) images show as
placeholders until you press `I` (or click the toolbar's image button) for that document.
They are then fetched with the system `curl` (on Windows, the `curl.exe` Windows comes with), at
most 4 at a time, limited to 12 MiB and
15 seconds each, and kept in memory only. `remote_images = always` loads them for every
document without asking, which lets any document you open contact the servers its images are
on. `never` removes the option. An SVG never loads anything: references to other files or URLs
inside it are ignored, even when you click to load it. Each SVG is drawn in a separate,
short-lived `vomd` process, which reads only the SVG and VOMD's fonts, so a slow or broken one
can be stopped: after 3 seconds, or after 120 seconds of CPU when you clicked to load it.

## Limitations

- **Linux and Windows.** The code compiles for macOS, but macOS is untested and unsupported.
- Selecting and copying, and the code blocks' copy button, work on X11, Wayland and Windows (not
  macOS); there is no keyboard selection (Shift and the arrows), the button can't be reached from the
  keyboard, and a word is a run of letters, digits and marks (a run of Chinese or Japanese
  characters counts as one word).
- No screen-reader support (AccessKit would add too much to the binary). Text scaling comes
  from the system scale factor that winit reports (`Xft.dpi` on X11, the output scale on
  Wayland, the display's scale on Windows).
- Drag-and-drop works on X11 and Windows, because winit doesn't support it on Wayland; there,
  open files with Ctrl+O or the toolbar's folder button.
- On Windows, emoji come from Segoe UI Emoji, which has no flags: a flag shows as its two
  letters, as in other Windows apps.
- Math and Mermaid show as text and code.
- Animated GIFs and WebPs show their first frame.
- SVGs: text uses VOMD's fonts (Inter and JetBrains Mono, or your `font_sans`/`font_mono`), so
  it can look different from a browser's. Images embedded in an SVG are drawn; references to
  other files or URLs are never followed. There is no animation or scripting.
- AVIF, HEIC, BMP, TIFF and ICO images show their alt text.
- `<img>` sizes in `em`, `vw` and other units than px and % are ignored.

## Development

`just ci` runs formatting, clippy (every feature set), the tests, and Windows/macOS compile
checks; CI also runs clippy, the tests and the size budget on Windows. `cargo xtask budgets` measures every performance budget against a release build (it
needs a display). `cargo xtask images` runs the image memory tests, each in its own process and
as many at once as there are CPUs (`-j N`, `--runs K` to repeat), much faster than
`cargo test` on a machine with several cores. `just fuzz <doc|image|layout|settings|svg>
[seconds]` fuzzes on the stable toolchain.
`vendor/` holds three crates VOMD patches, used through `[patch.crates-io]`: resvg 0.46 (its
raster images go to VOMD's decoders, and drawing can be cancelled), image-webp 0.2.4 (a
memory limit on a lossless image's Huffman codes) and simplecss 0.2.2 (an error's position made
in constant time, as a style sheet of many rules took the square of its length). Each one's
`VOMD-PATCH.md` lists the changes and how to upgrade.
`scripts/aur-check.sh` builds and checks both AUR packages offline. `scripts/clipboard-check.sh`
selects, copies and pastes against Xvfb and a headless sway in containers, with the real `xclip`,
`xsel` and `wl-paste` (it needs docker); the copy button is clicked on X11 only, because a
headless sway has no pointer. `scripts/pkg-build.sh` builds
the release files (tarball, `.deb`, AppImage) in a Debian 12 container as CI does, and
`scripts/pkg-check.sh` installs and runs the `.deb` and AppImage on Debian 12, Debian 13 and
Ubuntu 24.04 (both need docker).

## License

MIT. The embedded fonts are Inter and JetBrains Mono (SIL OFL 1.1) and Lucide icons (ISC);
their licenses are in `assets/fonts/`.
