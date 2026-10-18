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
- Budgets measured by `cargo xtask budgets`: binary size, cold and warm start, idle RSS,
  idle CPU, 4K scroll frame time. CI gates size, memory and idle CPU and reports the times
  (`--timings-advisory`). Fuzz targets for the parser, layout and settings.

[1.0.0]: https://github.com/allenguarnes/vomd/releases/tag/v1.0.0
