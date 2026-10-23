# Changelog

All notable changes to VOMD. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [1.6.1] - 2026-10-24

### Fixed
- Raw HTML shows the space between elements the way a browser does: the badges and links in a
  README's `<p align="center">` header no longer run together ("Latest releaseCILicense: MIT"),
  and a block of HTML has no space at its start or end.
- Holding `Q` (or `Ctrl`+`W`, or `Esc`) no longer goes on to close the other VOMD windows one after
  another. A key that is already down when a window gets the focus does nothing, however long you
  keep holding it, until you let go.
- A very large document (over 1 MB) with a footnote whose label ends in a backslash is laid out
  the same as when it is read whole.

## [1.6.0] - 2026-10-23

### Added
- Animated GIFs and WebPs play, on every platform. They are off by default, so they keep showing
  their first frame until you turn them on: the `animations = on` setting, the `A` key, or the
  toolbar's play button (shown when the document has an animated image), which are saved like `W`.
  - A paused animated image shows a ▶ badge at its lower left (on images of 48 × 48 pixels or
    more), and a click plays or pauses that image alone, whether animations are on or off.
    `Ctrl`+click does it on an image that is a link, whose click follows the link.
  - Timing is the browsers': a frame delay of 10 ms or less plays as 100 ms, and 20 ms (50 frames
    a second) is the fastest. Loop counts are honored, and the last frame stays, with the badge:
    a click (or `A` off and on) plays it again from the start.
  - An animation plays only while it is on screen, and waits while the window is minimized,
    covered whole (X11), or not drawn by the compositor (Wayland); it keeps playing in a window
    that is visible without the keyboard. With nothing playing VOMD uses no CPU, and only the
    animated images are painted again for each frame.
  - Frames are made on a thread of their own and use the 32 MiB image memory: small animations
    are kept whole, larger ones are decoded frame by frame, and at most half of that memory goes
    to animations. A README-sized GIF (640 × 360, or 800 × 600) plays by itself on a 2× HiDPI
    screen too. One over VOMD's limits shows its first frame and the badge, and a click plays
    it anyway, as *click to load* does for images.
- Windows 10 (1803) and newer, and Windows 11, on x86-64: every feature Linux has, from one
  `vomd.exe` that needs nothing installed (the C runtime is linked in). The release has
  `vomd-<version>-x86_64-windows-portable.zip`. `vomd.exe` isn't code-signed yet, so the first
  time it runs, SmartScreen shows "Windows protected your PC": click More info, then Run anyway
  (see the README's Install section).
  - Copy and paste use the Windows clipboard: a copy is made when a program pastes it, with CR LF
    line ends, and is kept on the clipboard when the window closes. Windows has no primary
    selection, so `primary_selection` doesn't apply.
  - Open uses the system's Open dialog; links open in the default browser and ⚙ opens the
    settings file (`%APPDATA%\vomd\settings.ini`) in the default app for `.ini` files.
  - Remote images are fetched with the `curl.exe` Windows comes with.
  - SVGs are drawn in separate `vomd.exe` processes, each in a job object that ends it with
    VOMD and limits its memory and CPU time, as on Linux.
  - The theme and the display's scale follow the system as they change, files can be dropped on
    the window, and the window fits the screen above the taskbar.
  - `vomd --help` and `--version` print in the console they were typed in; an error that stops
    VOMD when no console started it (a bad command line from a shortcut) shows in a message box.
  - The executable has its icon, version information and a manifest (per-monitor DPI awareness).

### Fixed
- An emoji that its font builds from several glyphs, one for each person joined by a zero-width
  joiner (Segoe UI Emoji's families and couples), shows every person, not only the first.
- Every invisible formatting character Unicode lists (also the Hangul fillers and the musical
  formatting characters) is left out of the text, as the others were, instead of showing as a box.

## [1.5.0] - 2026-10-22

### Added
- GIF images (the first frame of an animation).
- WebP images: lossy, lossless and with alpha (the first frame of an animation).
- SVG and svgz images, with their text (set in VOMD's fonts) and the images embedded in them,
  drawn at the screen's resolution so they stay sharp at any zoom. An SVG never reads another
  file or fetches anything. Each one is drawn in a separate, short-lived `vomd` process, so one
  that is slow or broken is stopped (after 3 seconds, or as soon as it isn't wanted) and never
  holds up the window or the other images; one you click to load has no 3 seconds, and runs
  until it is done, you leave the page, or it has used 120 seconds of CPU. The `svg` Cargo
  feature (on by default) builds them in; without it an SVG shows its alt text.
- `<img width height>`: a size in pixels or a percentage of the line's width, one side keeping
  the image's shape; Markdown images keep their own size.
- Click to load an image over VOMD's limits (too many pixels, an SVG too big, too complex or too
  slow to draw, a decode that needs more than the image budget): its placeholder says why and
  "click to load" (`Ctrl+click` in a link, where a click follows the link). It loads on a thread
  of its own, outside the budget, within half the memory available (1 GiB at most). It stops
  when the document goes (and starts again if the image is on the page you reload or come back
  to), and starts again at the new size when the image's size changes. The choice is kept until
  the window closes.

### Changed
- JPEG, PNG and WebP photos are turned upright by their EXIF orientation.
- The default binary is 5.5 MB, up from 3.9 MB (the build without SVG is 4.1 MB).
- A remote image whose server says it is over 12 MiB is refused as too large (click to load
  takes up to 256 MiB), not as a failed fetch.

## [1.4.0] - 2026-10-21

### Added
- Find in page. `Ctrl+F` or the toolbar's search button opens a bar below the toolbar, and every
  match of what you type is marked in the text as you type it: all in yellow (a muted amber in the
  dark theme), the current one in orange, with its place among them in the bar ("3 / 17", "No
  matches"). The query starts as the text you had selected, if that is one short line; the first
  match from where you are reading is the current one, and the document scrolls to put it a third of
  the way down unless it is in view already, however far it is. `Enter`, `F3` and `Ctrl+G` go to the
  next match and `Shift` with them to the previous, around the ends, as the bar's buttons do; the
  wheel over the bar does nothing, and `Esc` or its close button closes the bar and clears the marks
  (the query is kept, shown selected, for the next time); `Ctrl+F` and the button open and close it.
  What to look for is a one-line text field with the keyboard while the bar is open: the letter keys
  are typed into it, an input method's text too, the caret goes anywhere (the arrows, `Ctrl` for
  words, `Home` and `End`, a click), `Shift`, a drag, a double click (a word) and a triple click
  select, `Backspace` and `Delete` delete a character or, with `Ctrl`, a word, the text scrolls to
  keep the caret in view, and `Up`, `Down`, the page keys and the `Ctrl` shortcuts that aren't the
  field's still work on the document. `Ctrl+C`, `Ctrl+X` and `Ctrl+V` (and `Ctrl+Insert`,
  `Shift+Insert`) copy, cut and paste its text, a selection in it is offered as the primary
  selection, and a middle click on it pastes that; a paste is the first line, read in the
  background and never holding the window up. A click on the document gives the keyboard back to
  the document, the bar and its marks staying, and `F3` and `Ctrl+G` still step. Case
  is ignored for every script. Only the text shown is searched, never markup or an address, and no
  match runs from one block into another or from one table cell into the next; a very long
  paragraph's pieces count as one block. Every match is counted, but only the 10,000 around the
  current one are marked. A reload keeps the bar open and the query and searches the new text. The
  bar is chrome, so it scales with the display and not the zoom. Needs the new `find` feature, on by
  default; the smallest build leaves it out.

## [1.3.0] - 2026-10-21

### Added
- A table of contents panel. `C`, the toolbar's list button or `toc = true` in the settings shows
  the document's headings, h1 to h6 and indented by level, in a panel at the window's left, from
  below the toolbar down. The heading being read is marked and the panel scrolls to keep it in
  view; a heading you jump to stays marked until you scroll, even one too near the end to come up
  to the top (`[` and `]` then step on from it); a click on a heading jumps to it as a link to its
  `#anchor` does (Back returns), and the wheel over the panel scrolls the panel alone. In a window
  at least 740 logical pixels wide the panel pushes the document aside and the document is laid
  out in the rest, keeping your place; in a narrower one it covers the document, and a click on a
  heading, a click on the document or `Esc` closes it. Headings inside footnotes are left out, a
  document without headings says so, and the panel stays open (and keeps its place) through a
  reload. It is chrome, so it scales with the display and not the zoom. The `toc` setting is saved
  when you toggle the panel. Needs the new `toc` feature, on by default; the smallest build leaves
  it out.

### Fixed
- A heading of nothing but punctuation or emoji (`## !!!`, `## 🚀`) was skipped: `[` and `]`
  passed over it, it was missing from the contents panel, and a reload kept your place from the
  heading before it. Such headings are headings now, with the ids GitHub gives them (none for the
  first, which `#` can't name, then `-1`, `-2`, ...), so links to those ids written for GitHub
  work too. A heading with no text at all is still left out of the panel.
- Documents over 1 MB, which are parsed in pieces, render the same as when parsed whole when a
  piece that refers to a footnote defined in another piece has a control character such as U+0001
  between square brackets: it came out as a link.

## [1.2.0] - 2026-10-20

### Added
- Select and copy text. Drag to select (a drag that starts on a link selects, and a click
  still follows it), double-click for a word, triple-click for a paragraph, heading, code block
  or table row, `Shift`+click to extend, `Ctrl+A` for everything, and drag past the top or bottom
  of the window to scroll. `Ctrl+C` (also `Ctrl+Insert`, `Ctrl+Shift+C` and the Copy key) copies
  plain text: list items start with `- `, quoted lines with `> `, table cells are tab-separated,
  an image is its alt text and a link its text. The text is made only when a program pastes it,
  so copying a whole 5 MB document costs no memory until then (X11 INCR and Wayland pipes, in
  pieces of 64 KiB). On X11 and Wayland a selection made with the mouse (and `Ctrl+A`) is also the
  primary selection, pasted with a middle click; the new `primary_selection = on|off` setting
  turns that off. As the window closes on X11 the clipboard manager, if there is one, is asked to
  keep the clipboard.
  Needs the new `clipboard` feature, on by default; the smallest build leaves it out.
- `Esc` clears the selection before it quits (after closing the toolbar's menu, as before).
- A copy button on code blocks, as on GitHub: while the pointer is over a fenced or indented
  code block, a button shows in its upper right corner, and a click copies the block's code to
  the clipboard (not the primary selection, and a selection is left alone). The code is as
  written, with its tabs, indentation and blank lines, without the language tag, without the
  quote marks or list indent of what holds the block, and without a line break after the last
  line; a block of several MB is made only when something pastes it. The button turns into a
  check mark for two seconds, lights up and shows the hand under the pointer, scales with the
  zoom and the display, and is left off blocks too small for it. If there is no clipboard, a
  note says so. Part of the `clipboard` feature, so the smallest build has none.

### Fixed
- An image decode that started just as the images wanted changed is counted in the room the
  image cache keeps free. Before, the cache could briefly fill the memory that decode used.
- With remote images on, a picture near the window could be fetched again and again after a
  few arrived at once: each body let in was the first thing the next one's room was made from.

## [1.1.0] - 2026-10-19

### Added
- Open a file from the window: `Ctrl+O` or the toolbar's folder button shows the desktop's
  file dialog (the XDG portal's, so KDE's, GNOME's or the GTK one, else `zenity` or
  `kdialog`), starting in the document's folder. The empty window says so, and offers
  dropping a file only on X11, where it works.
- A Debian package (`vomd_<version>-1_amd64.deb`, Debian 12+ and Ubuntu 24.04+) with a man
  page, and an AppImage for any x86-64 distribution with glibc 2.36 or newer, both attached to
  each release and checked by CI on Debian 12, Debian 13 and Ubuntu 24.04, on X11 and Wayland.
- The Arch packages list the file dialogs Open can use as optional dependencies:
  `xdg-desktop-portal-gtk`, `zenity` and `kdialog`. KDE and GNOME already have a portal file
  chooser.

### Fixed
- On X11 the portal's file dialog belongs to the VOMD window, so the desktop keeps it in front
  of it. (On Wayland it still has no parent window.)
- A portal that crashes or stops answering no longer blocks Ctrl+O: the dialog's calls give
  up after 25 seconds, and if the portal leaves the session bus while its dialog is up, the
  window says so and Ctrl+O works again. Before, "A file dialog is already open" stayed until
  VOMD was restarted.
- The empty window offers dropping a file only on X11, decided by the display VOMD connected
  to, not by environment variables.
- The toolbar's settings button opens `settings.ini` in a terminal editor set as the default
  (`Terminal=true`, such as Neovim) through `xdg-terminal-exec`. Before, on desktops
  `xdg-open` doesn't know (Hyprland, Sway) the editor started with no terminal and waited
  unseen. If the file can't be opened, a note in the window says why.
- Release binaries are built on Debian 12 (glibc 2.36), so a glibc older than 2.39 no longer
  prints "weak version `GLIBC_2.39' not found" at every start.
- Documents over 1 MB, which are parsed in pieces, render the same as when parsed whole when
  the line that ends an HTML block looks like a table, a code fence or another HTML block, or
  when a code fence is left open in a list item. Before, the piece could be cut inside a code
  block or a table, and the rest of the document rendered differently.

## [1.0.0] - 2026-10-19

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
- Packaging: Arch PKGBUILDs `vomd` (from source) and `vomd-bin`, and a release tarball with a
  desktop entry, icon and the README's image.
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
- Budgets measured by `cargo xtask budgets`: binary size, cold and warm start, idle RSS,
  idle CPU, 4K scroll frame time. CI gates size, memory and idle CPU and reports the times
  (`--timings-advisory`). Fuzz targets for the parser, layout and settings.

[1.6.1]: https://github.com/allenguarnes/vomd/compare/v1.6.0...v1.6.1
[1.6.0]: https://github.com/allenguarnes/vomd/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/allenguarnes/vomd/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/allenguarnes/vomd/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/allenguarnes/vomd/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/allenguarnes/vomd/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/allenguarnes/vomd/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0
