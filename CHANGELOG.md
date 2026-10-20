# Changelog

All notable changes to VOMD. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[1.2.0]: https://github.com/allenguarnes/vomd/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/allenguarnes/vomd/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0
