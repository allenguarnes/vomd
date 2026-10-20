//! The toolbar: one slim bar docked at the top, never over the text.
//!
//! Left: Back, Open, contents. Right: remote images (only when the document has any), zoom (− 100% +), wide
//! layout, theme, reload, find, settings. Buttons for features that aren't built yet are absent. On
//! narrow windows the right group collapses into a `⋯` button with a small dropdown, the one
//! popup in the app: zoom first, then remote images, wide layout, theme, ...
//! A tooltip with the name and shortcut appears after 500 ms. Icons are glyphs of an embedded
//! Lucide subset, drawn through the glyph cache, so they're crisp at every scale.
//!
//! Geometry is in physical pixels; sizes below are logical and scale with the window's HiDPI
//! factor, not with the document zoom.

use std::time::{Duration, Instant};

use crate::fonts::Fonts;
use crate::layout::BlockBox;
use crate::render::{self, Frame};
use crate::text::cache::{GlyphCache, GlyphKey};
use crate::text::{Align, Para, TextCtx, TextStyle};
use crate::theme::{Role, Theme};
use crate::view::{Action, ThemeMode};

/// Bar height, button size, icon size, the zoom percentage's width, the margin at the window
/// edges and the gap between the groups, in logical pixels.
const BAR: f32 = 32.0;
const BUTTON: f32 = 32.0;
const ICON: f32 = 16.0;
const PERCENT: f32 = 48.0;
const EDGE: f32 = 4.0;
const GAP: f32 = 8.0;
/// The left group: Back and Open.
const LEFT: f32 = 2.0 * BUTTON;
/// Text size of the percentage, tooltips and menu rows.
const LABEL: f32 = 13.0;
/// Menu row height and width.
const ROW: f32 = 28.0;
const MENU_W: f32 = 230.0;
/// How long the pointer rests on a button before its tooltip shows.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// Lucide code points in `assets/fonts/icons.ttf`.
pub(crate) mod icon {
    pub const BACK: char = '\u{e048}';
    pub const MINUS: char = '\u{e11c}';
    pub const PLUS: char = '\u{e13d}';
    pub const THEME: char = '\u{e09d}';
    pub const RELOAD: char = '\u{e149}';
    pub const SETTINGS: char = '\u{e154}';
    pub const OPEN: char = '\u{e247}';
    pub const MORE: char = '\u{e0b6}';
    pub const IMAGE: char = '\u{e0f6}';
    /// The contents panel.
    #[cfg(feature = "toc")]
    pub const LIST: char = '\u{e106}';
    /// Wide layout is off (the button widens) and on (it narrows).
    pub const UNFOLD: char = '\u{e43d}';
    pub const FOLD: char = '\u{e43b}';
    /// The copy button on code blocks, and its mark once the code is copied.
    #[cfg(feature = "clipboard")]
    pub const COPY: char = '\u{e09e}';
    #[cfg(feature = "clipboard")]
    pub const CHECK: char = '\u{e06c}';
    /// The find button, and the find bar's previous and next match and close buttons.
    #[cfg(feature = "find")]
    pub const SEARCH: char = '\u{e151}';
    #[cfg(feature = "find")]
    pub const CHEVRON_UP: char = '\u{e070}';
    #[cfg(feature = "find")]
    pub const CHEVRON_DOWN: char = '\u{e06d}';
    #[cfg(feature = "find")]
    pub const CLOSE: char = '\u{e1b2}';
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Back,
    /// Choose a file to open.
    Open,
    /// Show or hide the contents panel.
    #[cfg(feature = "toc")]
    Contents,
    /// Load or hide remote images, with a badge counting them.
    Images,
    ZoomOut,
    /// The percentage between − and +; clicking it resets the zoom.
    ZoomReset,
    ZoomIn,
    /// Wide layout on or off.
    Wide,
    Theme,
    Reload,
    /// Show or hide the find bar.
    #[cfg(feature = "find")]
    Find,
    Settings,
    /// `⋯`: opens the dropdown with the items that didn't fit.
    More,
}

/// What a click on the toolbar asks the shell to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Action(Action),
    /// Open the settings file (⚙).
    Settings,
}

/// Everything outside the toolbar it depends on, for one frame or event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ctx {
    /// Window size, physical pixels.
    pub width: u32,
    pub height: u32,
    /// HiDPI factor (not the zoom).
    pub scale: f32,
    /// The back stack isn't empty.
    pub can_back: bool,
    pub zoom: u16,
    pub theme: ThemeMode,
    /// Wide layout is on.
    pub wide: bool,
    /// The contents panel is open.
    #[cfg(feature = "toc")]
    pub toc: bool,
    /// The find bar is open.
    #[cfg(feature = "find")]
    pub find: bool,
    /// There is a settings file to open (not with `--no-settings`).
    pub settings: bool,
    /// Remote images in the document (0: no remote-images button).
    pub remote: u32,
    /// Remote images are on for the document.
    pub remote_on: bool,
    /// `curl` couldn't be run: the button says so and does nothing.
    pub no_curl: bool,
    /// The remote-images button pulses (once, after the first frame).
    pub pulse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        (f64::from(self.x0)..f64::from(self.x1)).contains(&x)
            && (f64::from(self.y0)..f64::from(self.y1)).contains(&y)
    }

    pub(crate) fn px(&self) -> (i32, i32, i32, i32) {
        let r = |v: f32| v.round() as i32;
        (r(self.x0), r(self.y0), r(self.x1), r(self.y1))
    }
}

/// Where the bar's items are this frame, and which right-group items went into the dropdown.
#[derive(Clone, Debug, PartialEq)]
pub struct Slots {
    pub items: Vec<(Item, Rect)>,
    pub overflow: Vec<Item>,
}

/// A spot the pointer can be over: a bar item, or a dropdown row (by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Bar(Item),
    Row(usize),
}

impl Item {
    /// Tooltip and menu text: name and shortcut.
    pub fn label(self, ctx: &Ctx) -> String {
        match self {
            Item::Back => "Back (Backspace)".into(),
            Item::Open => "Open a file (Ctrl+O)".into(),
            #[cfg(feature = "toc")]
            Item::Contents if ctx.toc => "Hide contents (C)".into(),
            #[cfg(feature = "toc")]
            Item::Contents => "Contents (C)".into(),
            Item::Images if ctx.no_curl => "Remote images: curl not found".into(),
            Item::Images if ctx.remote_on => "Hide remote images (I)".into(),
            Item::Images => {
                let s = if ctx.remote == 1 { "" } else { "s" };
                format!("Load {} remote image{s} (I)", ctx.remote)
            }
            Item::ZoomOut => "Zoom out (Ctrl+−)".into(),
            Item::ZoomReset => format!("Reset zoom {}% (Ctrl+0)", ctx.zoom),
            Item::ZoomIn => "Zoom in (Ctrl+=)".into(),
            Item::Wide if ctx.wide => "Normal width (W)".into(),
            Item::Wide => "Wide layout (W)".into(),
            Item::Theme => match ctx.theme {
                ThemeMode::System => "Theme: system (T)",
                ThemeMode::Light => "Theme: light (T)",
                ThemeMode::Dark => "Theme: dark (T)",
            }
            .into(),
            Item::Reload => "Reload (R)".into(),
            #[cfg(feature = "find")]
            Item::Find if ctx.find => "Close find (Esc)".into(),
            #[cfg(feature = "find")]
            Item::Find => "Find (Ctrl+F)".into(),
            Item::Settings => "Settings".into(),
            Item::More => "More".into(),
        }
    }

    fn icon(self, ctx: &Ctx) -> Option<char> {
        Some(match self {
            Item::Back => icon::BACK,
            Item::Open => icon::OPEN,
            #[cfg(feature = "toc")]
            Item::Contents => icon::LIST,
            Item::Images => icon::IMAGE,
            Item::ZoomOut => icon::MINUS,
            Item::ZoomIn => icon::PLUS,
            Item::Wide if ctx.wide => icon::FOLD,
            Item::Wide => icon::UNFOLD,
            Item::Theme => icon::THEME,
            Item::Reload => icon::RELOAD,
            #[cfg(feature = "find")]
            Item::Find => icon::SEARCH,
            Item::Settings => icon::SETTINGS,
            Item::More => icon::MORE,
            Item::ZoomReset => return None,
        })
    }

    fn command(self) -> Option<Command> {
        Some(match self {
            Item::Back => Command::Action(Action::Back),
            Item::Open => Command::Action(Action::Open),
            #[cfg(feature = "toc")]
            Item::Contents => Command::Action(Action::ToggleToc),
            Item::Images => Command::Action(Action::ToggleRemote),
            Item::ZoomOut => Command::Action(Action::ZoomOut),
            Item::ZoomReset => Command::Action(Action::ZoomReset),
            Item::ZoomIn => Command::Action(Action::ZoomIn),
            Item::Wide => Command::Action(Action::ToggleWide),
            Item::Theme => Command::Action(Action::CycleTheme),
            Item::Reload => Command::Action(Action::Reload),
            #[cfg(feature = "find")]
            Item::Find => Command::Action(Action::ToggleFind),
            Item::Settings => Command::Settings,
            Item::More => return None,
        })
    }

    fn enabled(self, ctx: &Ctx) -> bool {
        match self {
            Item::Back => ctx.can_back,
            Item::Images => !ctx.no_curl,
            _ => true,
        }
    }
}

/// The right group's items, left to right, with their logical widths and collapse groups: the
/// lowest group collapses into `⋯` first. The remote-images button is the first, and only there
/// when the document has remote images; it goes after the zoom (collapsed alone, `⋯` would take
/// its place for nothing). Wide layout goes just before the theme and collapses just before it.
/// Find goes after reload, and ⚙ is the last, and only there with a settings file.
#[cfg(feature = "find")]
const RIGHT: [(Item, f32, u8); 9] = [
    (Item::Images, BUTTON, 1),
    (Item::ZoomOut, BUTTON, 0),
    (Item::ZoomReset, PERCENT, 0),
    (Item::ZoomIn, BUTTON, 0),
    (Item::Wide, BUTTON, 2),
    (Item::Theme, BUTTON, 3),
    (Item::Reload, BUTTON, 4),
    (Item::Find, BUTTON, 5),
    (Item::Settings, BUTTON, 6),
];
#[cfg(not(feature = "find"))]
const RIGHT: [(Item, f32, u8); 8] = [
    (Item::Images, BUTTON, 1),
    (Item::ZoomOut, BUTTON, 0),
    (Item::ZoomReset, PERCENT, 0),
    (Item::ZoomIn, BUTTON, 0),
    (Item::Wide, BUTTON, 2),
    (Item::Theme, BUTTON, 3),
    (Item::Reload, BUTTON, 4),
    (Item::Settings, BUTTON, 6),
];

/// The contents button's collapse group: it goes last, after everything of the right group, since
/// it sits with Back and Open at the left.
const CONTENTS_GROUP: u8 = 7;

/// The right group's items in this window.
fn right_items(ctx: &Ctx) -> &'static [(Item, f32, u8)] {
    &RIGHT[usize::from(ctx.remote == 0)..RIGHT.len() - 1 + usize::from(ctx.settings)]
}

/// How many of the right group's leading groups don't fit and collapse into `⋯`.
fn collapsed(ctx: &Ctx) -> u8 {
    let right = right_items(ctx);
    let last = right.last().map_or(0, |r| r.2);
    let last = if cfg!(feature = "toc") {
        last.max(CONTENTS_GROUP)
    } else {
        last
    };
    let mut first = 0;
    while first <= last {
        let rest: f32 = right.iter().filter(|r| r.2 >= first).map(|r| r.1).sum();
        let more = if first > 0 { BUTTON } else { 0.0 };
        if (EDGE + left_width(first) + GAP + rest + more + EDGE) * ctx.scale <= ctx.width as f32 {
            break;
        }
        first += 1;
    }
    first
}

/// The left group's width, in logical pixels, when the groups below `first` are collapsed: Back
/// and Open, and the contents button while it is still there.
fn left_width(first: u8) -> f32 {
    if cfg!(feature = "toc") && first <= CONTENTS_GROUP {
        LEFT + BUTTON
    } else {
        LEFT
    }
}

/// The bar's layout for this frame: Back, Open and contents on the left, the right group laid out
/// from the right edge, and as many of its leading groups as don't fit collapsed into `⋯`.
pub fn slots(ctx: &Ctx) -> Slots {
    let s = ctx.scale;
    let h = BAR * s;
    let (right, first) = (right_items(ctx), collapsed(ctx));
    let bar = |x0, x1| Rect {
        x0,
        y0: 0.0,
        x1,
        y1: h,
    };
    let mut items = vec![
        (Item::Back, bar(EDGE * s, (EDGE + BUTTON) * s)),
        (Item::Open, bar((EDGE + BUTTON) * s, (EDGE + LEFT) * s)),
    ];
    #[cfg(feature = "toc")]
    if first <= CONTENTS_GROUP {
        items.push((
            Item::Contents,
            bar((EDGE + LEFT) * s, (EDGE + LEFT + BUTTON) * s),
        ));
    }
    let mut x = ctx.width as f32 - EDGE * s;
    for &(item, iw, _) in right.iter().rev().filter(|r| r.2 >= first) {
        items.push((item, bar(x - iw * s, x)));
        x -= iw * s;
    }
    let mut overflow: Vec<Item> = Vec::new();
    #[cfg(feature = "toc")]
    if first > CONTENTS_GROUP {
        overflow.push(Item::Contents);
    }
    overflow.extend(right.iter().filter(|r| r.2 < first).map(|r| r.0));
    if !overflow.is_empty() {
        // Never over the left group: in a window too narrow for both, `⋯` runs off the right
        // edge.
        let x0 = (x - BUTTON * s).max((EDGE + left_width(first)) * s);
        items.push((Item::More, bar(x0, x0 + BUTTON * s)));
    }
    Slots { items, overflow }
}

/// The dropdown's box: right-aligned under `⋯`, kept inside the window and cut short at its
/// bottom edge.
fn menu_box(ctx: &Ctx, slots: &Slots) -> Option<Rect> {
    let s = ctx.scale;
    let (_, more) = slots.items.iter().find(|(i, _)| *i == Item::More)?;
    let w = (MENU_W * s).min(ctx.width as f32);
    let x1 = more.x1.max(w).min(ctx.width as f32);
    let y0 = (BAR + 2.0) * s;
    Some(Rect {
        x0: x1 - w,
        y0,
        x1,
        y1: (y0 + menu_height(ctx, slots))
            .min(ctx.height as f32)
            .max(y0),
    })
}

/// The height of all of the dropdown's rows plus padding.
fn menu_height(ctx: &Ctx, slots: &Slots) -> f32 {
    (2.0 * EDGE + slots.overflow.len() as f32 * ROW) * ctx.scale
}

/// Row `n` of the dropdown (in `b`), scrolled up by `scroll` pixels: it's only seen inside the
/// box.
fn menu_row(ctx: &Ctx, b: Rect, scroll: f32, n: usize) -> Rect {
    let s = ctx.scale;
    let y0 = b.y0 + (EDGE + n as f32 * ROW) * s - scroll;
    Rect {
        y0,
        y1: y0 + ROW * s,
        ..b
    }
}

#[derive(Clone, Debug, Default)]
pub struct Toolbar {
    hidden: bool,
    hover: Option<Spot>,
    pressed: Option<Spot>,
    /// The dropdown is open, scrolled up by this many pixels.
    menu: Option<f32>,
    /// When the pointer came to rest on the hovered bar item (the tooltip's timer).
    hover_since: Option<Instant>,
    tooltip: bool,
}

impl Toolbar {
    pub fn visible(&self) -> bool {
        !self.hidden
    }

    /// Show or hide the bar (`B`, the `toolbar` setting). Hiding forgets its pointer state.
    pub fn set_visible(&mut self, visible: bool) {
        self.hidden = !visible;
        if self.hidden {
            *self = Toolbar {
                hidden: true,
                ..Toolbar::default()
            };
        }
    }

    /// The bar's height in physical pixels (0 when hidden): the document area starts below it.
    pub fn height(&self, scale: f32) -> u32 {
        if self.hidden {
            0
        } else {
            (BAR * scale).round() as u32
        }
    }

    /// The dropdown is open and has something to show: one that lost its `⋯` (a window widened
    /// until nothing overflows) counts as closed.
    fn open(&self, ctx: &Ctx) -> bool {
        self.menu.is_some() && collapsed(ctx) > 0
    }

    fn spot_at(&self, ctx: &Ctx, x: f64, y: f64) -> Option<Spot> {
        // Nothing outside the window (a drag can end there), nor anything cut off by its edge.
        if self.hidden
            || !(0.0..f64::from(ctx.width)).contains(&x)
            || !(0.0..f64::from(ctx.height)).contains(&y)
        {
            return None;
        }
        let slots = slots(ctx);
        if let Some(scroll) = self.menu
            && let Some(b) = menu_box(ctx, &slots)
            && b.contains(x, y)
            && let Some(n) =
                (0..slots.overflow.len()).find(|&n| menu_row(ctx, b, scroll, n).contains(x, y))
        {
            return Some(Spot::Row(n));
        }
        slots
            .items
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|&(i, _)| Spot::Bar(i))
    }

    /// The point is the toolbar's: on the bar or on the open dropdown, so the document doesn't
    /// see it.
    pub fn covers(&self, ctx: &Ctx, x: f64, y: f64) -> bool {
        if self.hidden {
            return false;
        }
        (y >= 0.0 && y < f64::from(self.height(ctx.scale))) || self.menu_covers(ctx, x, y)
    }

    /// The point is on the open dropdown, which hangs over whatever is below the bar: the find
    /// bar, the contents panel, the document.
    pub fn menu_covers(&self, ctx: &Ctx, x: f64, y: f64) -> bool {
        !self.hidden
            && self.menu.is_some()
            && menu_box(ctx, &slots(ctx)).is_some_and(|b| b.contains(x, y))
    }

    /// The pointer moved to (`x`, `y`). Returns true if the toolbar needs a repaint.
    pub fn pointer(&mut self, ctx: &Ctx, x: f64, y: f64, now: Instant) -> bool {
        let spot = self.spot_at(ctx, x, y);
        if spot == self.hover {
            return false;
        }
        self.hover = spot;
        self.tooltip = false;
        // No timer under an open dropdown: it would be due the moment the dropdown closes.
        self.hover_since = (matches!(spot, Some(Spot::Bar(_))) && !self.open(ctx)).then_some(now);
        true
    }

    /// The pointer left the window. Returns true if the toolbar needs a repaint.
    pub fn leave(&mut self) -> bool {
        let changed = self.hover.is_some() || self.tooltip;
        self.hover = None;
        self.hover_since = None;
        self.tooltip = false;
        changed
    }

    /// Mouse button down at (`x`, `y`). Returns true if the toolbar took it: on one of its
    /// items, or anywhere while the dropdown is open (an outside press closes it).
    pub fn press(&mut self, ctx: &Ctx, x: f64, y: f64) -> bool {
        let was_open = self.open(ctx);
        let spot = self.spot_at(ctx, x, y);
        // Another button closes the dropdown (and still acts); `⋯` toggles on release and a row
        // closes it on release.
        if !(was_open && matches!(spot, Some(Spot::Row(_) | Spot::Bar(Item::More)))) {
            self.menu = None;
        }
        self.pressed = spot;
        self.tooltip = false;
        self.hover_since = None;
        spot.is_some() || was_open || self.covers(ctx, x, y)
    }

    /// Mouse button up at (`x`, `y`): a press and release on the same enabled item is a click.
    /// Returns whether the toolbar handled the press (repaint) and the command clicked, if any.
    pub fn release(&mut self, ctx: &Ctx, x: f64, y: f64) -> (bool, Option<Command>) {
        let Some(pressed) = self.pressed.take() else {
            return (false, None);
        };
        if self.spot_at(ctx, x, y) != Some(pressed) {
            return (true, None);
        }
        let item = match pressed {
            Spot::Bar(Item::More) => {
                self.menu = self.menu.is_none().then_some(0.0);
                return (true, None);
            }
            Spot::Bar(item) => item,
            Spot::Row(n) => {
                self.menu = None;
                match slots(ctx).overflow.get(n) {
                    Some(&item) => item,
                    None => return (true, None),
                }
            }
        };
        let command = item.enabled(ctx).then(|| item.command()).flatten();
        (true, command)
    }

    /// Close the dropdown (Escape, a resize). Returns true if it was open.
    pub fn close_menu(&mut self, ctx: &Ctx) -> bool {
        let was = self.open(ctx);
        self.menu = None;
        was
    }

    pub fn menu_open(&self, ctx: &Ctx) -> bool {
        self.open(ctx)
    }

    /// How far the dropdown's rows are scrolled, if it is open.
    #[cfg(all(test, feature = "find", feature = "embed-fonts"))]
    pub(crate) fn menu_scroll(&self) -> Option<f32> {
        self.menu
    }

    /// The wheel (or an arrow key) moves the dropdown's rows by `dy` pixels when the window
    /// cuts it short. Returns true if the dropdown is open: it takes the scrolling.
    pub fn scroll_menu(&mut self, ctx: &Ctx, dy: f32) -> bool {
        let slots = slots(ctx);
        let (Some(scroll), Some(b)) = (self.menu, menu_box(ctx, &slots)) else {
            return false;
        };
        let max = (menu_height(ctx, &slots) - (b.y1 - b.y0)).max(0.0);
        self.menu = Some((scroll + dy).max(0.0).min(max));
        true
    }

    /// When the loop must wake to show the tooltip (`None`: no timer).
    pub fn deadline(&self, ctx: &Ctx) -> Option<Instant> {
        if self.tooltip || self.open(ctx) {
            return None;
        }
        self.hover_since.map(|t| t + TOOLTIP_DELAY)
    }

    /// Show the tooltip if its time has come. Returns true if the window needs a repaint.
    pub fn expire(&mut self, ctx: &Ctx, now: Instant) -> bool {
        match self.deadline(ctx) {
            Some(t) if now >= t => {
                self.tooltip = true;
                true
            }
            _ => false,
        }
    }

    /// Paint the bar, then the open dropdown and the tooltip over the document.
    pub fn paint(
        &self,
        frame: &mut Frame,
        ctx: &Ctx,
        fonts: &Fonts,
        cache: &mut GlyphCache,
        text: &mut TextCtx,
        theme: &Theme,
    ) {
        if self.hidden {
            return;
        }
        let s = ctx.scale;
        let h = self.height(s) as i32;
        let w = ctx.width as i32;
        let line = s.round().max(1.0) as i32;
        frame.fill_rect(0, 0, w, h, theme.code_bg);
        frame.fill_rect(0, h - line, w, h, theme.border);
        let slots = slots(ctx);
        let scroll = self.menu.filter(|_| !slots.overflow.is_empty());
        let open = scroll.is_some();
        // The pulse is on `⋯` while the button is inside its dropdown.
        let pulse_on = if slots.overflow.contains(&Item::Images) {
            Item::More
        } else {
            Item::Images
        };
        for &(item, rect) in &slots.items {
            let on = (item == Item::More && open) || (item == Item::Images && ctx.remote_on);
            #[cfg(feature = "toc")]
            let on = on || (item == Item::Contents && ctx.toc);
            #[cfg(feature = "find")]
            let on = on || (item == Item::Find && ctx.find);
            if cfg!(feature = "remote-images") && item == pulse_on && ctx.pulse {
                let inset = 3.0 * s;
                let r = Rect {
                    x0: rect.x0 + inset,
                    y0: rect.y0 + inset,
                    x1: rect.x1 - inset,
                    y1: rect.y1 - inset,
                };
                frame.rounded_rect(r.px(), (4.0 * s).round(), None, theme.link, 90);
            }
            self.paint_item(
                frame,
                ctx,
                item,
                rect,
                Spot::Bar(item),
                on,
                fonts,
                cache,
                text,
                theme,
            );
            if cfg!(feature = "remote-images")
                && item == Item::Images
                && !ctx.remote_on
                && !ctx.no_curl
            {
                paint_badge(frame, ctx, fonts, cache, text, theme, rect);
            }
        }
        if let Some(scroll) = scroll
            && let Some(b) = menu_box(ctx, &slots)
        {
            let r = (4.0 * s).round();
            frame.rounded_rect(b.px(), r, None, theme.bg, 255);
            frame.rounded_rect(b.px(), r, Some(line as f32), theme.border, 255);
            // Rows (and their text) stay inside the box, however narrow or short the window.
            let (x0, y0, x1, y1) = b.px();
            let prev = frame.set_clip(render::Clip {
                x0,
                y0,
                x1: x1 - (EDGE * s) as i32,
                y1,
            });
            for (n, &item) in slots.overflow.iter().enumerate() {
                let rect = menu_row(ctx, b, scroll, n);
                let spot = Spot::Row(n);
                self.paint_item(
                    frame, ctx, item, rect, spot, false, fonts, cache, text, theme,
                );
            }
            frame.set_clip(prev);
        }
        if self.tooltip
            && let Some(Spot::Bar(item)) = self.hover
            && let Some(&(_, rect)) = slots.items.iter().find(|(i, _)| *i == item)
        {
            paint_tooltip(
                frame,
                ctx,
                fonts,
                cache,
                text,
                theme,
                &item.label(ctx),
                rect,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_item(
        &self,
        frame: &mut Frame,
        ctx: &Ctx,
        item: Item,
        rect: Rect,
        spot: Spot,
        on: bool,
        fonts: &Fonts,
        cache: &mut GlyphCache,
        text: &mut TextCtx,
        theme: &Theme,
    ) {
        let s = ctx.scale;
        let enabled = item.enabled(ctx);
        let hot = enabled && self.hover == Some(spot);
        let down = hot && self.pressed == Some(spot);
        if hot || on {
            let inset = 3.0 * s;
            let r = Rect {
                x0: rect.x0 + inset,
                y0: rect.y0 + inset,
                x1: rect.x1 - inset,
                y1: rect.y1 - inset,
            };
            let alpha = if down || on { 200 } else { 110 };
            frame.rounded_rect(r.px(), (4.0 * s).round(), None, theme.border, alpha);
        }
        let row = matches!(spot, Spot::Row(_));
        let color = if !enabled {
            theme.border
        } else if hot || on || row {
            theme.text
        } else {
            theme.muted
        };
        // A row has its icon in a square at the left and its label next to it; a bar item's
        // icon, or the zoom percentage, is centered in its button.
        let x = rect.x0 + ROW * s;
        let (icon_box, label_box) = if row {
            let label = Rect {
                x0: x,
                x1: rect.x1 - EDGE * s,
                ..rect
            };
            (Rect { x1: x, ..rect }, label)
        } else {
            (rect, rect)
        };
        if let Some(c) = item.icon(ctx) {
            draw_icon(frame, fonts, cache, c, icon_box, s, color);
        }
        let label = if row {
            Some(item.label(ctx))
        } else {
            item.icon(ctx).is_none().then(|| format!("{}%", ctx.zoom))
        };
        if let Some(label) = label {
            let p = layout_label(fonts, text, &label, label_box, s, row);
            paint_label(frame, fonts, cache, &p, color, theme);
        }
    }
}

/// An icon glyph centered in `rect` (by its ink: the Lucide glyphs fill their em box), 16 logical
/// pixels tall at scale `s`.
pub(crate) fn draw_icon(
    frame: &mut Frame,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    c: char,
    rect: Rect,
    s: f32,
    color: u32,
) {
    let face = fonts.icons();
    let id = fonts.face(face).glyph(c);
    let (key, _) = GlyphKey::at(face, id, ICON * s, 0.0, 0);
    let Some(img) = cache.get(fonts, key) else {
        return;
    };
    let cx = (rect.x0 + rect.x1) / 2.0;
    let cy = (rect.y0 + rect.y1) / 2.0;
    let x = (cx - img.width as f32 / 2.0).round() as i32;
    let y = (cy - img.height as f32 / 2.0).round() as i32;
    frame.blit_mask(x, y, img.width, img.height, &img.data, color);
}

/// A line of UI text laid out and placed in its box.
struct Placed {
    content: BlockBox,
    x: f32,
    y: f32,
    width: f32,
}

/// One line of UI text in `rect`: centered, or at its left edge.
fn layout_label(
    fonts: &Fonts,
    text: &mut TextCtx,
    label: &str,
    rect: Rect,
    s: f32,
    left: bool,
) -> Placed {
    let style = TextStyle {
        bold: false,
        italic: false,
        mono: false,
        size: LABEL * s,
        role: Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    };
    let mut para = Para::default();
    para.push_text(label, 0, style);
    let pl = text.layout(fonts, &para, 1.0e6, Align::Start, 1.3);
    let (w, h) = (pl.width, pl.height);
    let content = BlockBox::from_para(pl, &para.styles);
    let x = if left {
        rect.x0
    } else {
        (rect.x0 + rect.x1 - w) / 2.0
    };
    let y = (rect.y0 + rect.y1 - h) / 2.0;
    Placed {
        content,
        x,
        y,
        width: w,
    }
}

fn paint_label(
    frame: &mut Frame,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    placed: &Placed,
    color: u32,
    theme: &Theme,
) {
    // UI text takes the given color whatever role its style names.
    let mut t = *theme;
    t.text = color;
    render::paint_block(
        frame,
        &placed.content,
        placed.x,
        placed.y,
        fonts,
        cache,
        &t,
        #[cfg(feature = "clipboard")]
        None,
    );
}

/// The count of remote images in a small pill at the top right of their button.
fn paint_badge(
    frame: &mut Frame,
    ctx: &Ctx,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    text: &mut TextCtx,
    theme: &Theme,
    button: Rect,
) {
    let s = ctx.scale;
    let label = if ctx.remote > 99 {
        "99+".to_owned()
    } else {
        ctx.remote.to_string()
    };
    let style = TextStyle {
        bold: true,
        italic: false,
        mono: false,
        size: 9.0 * s,
        role: Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    };
    let mut para = Para::default();
    para.push_text(&label, 0, style);
    let pl = text.layout(fonts, &para, 1.0e6, Align::Start, 1.2);
    let (tw, th) = (pl.width, pl.height);
    let h = th.max(12.0 * s);
    let w = (tw + 6.0 * s).max(h);
    let x1 = button.x1 - 2.0 * s;
    let y0 = button.y0 + 3.0 * s;
    let b = Rect {
        x0: x1 - w,
        y0,
        x1,
        y1: y0 + h,
    };
    frame.rounded_rect(b.px(), (h / 2.0).round(), None, theme.link, 255);
    let placed = Placed {
        content: BlockBox::from_para(pl, &para.styles),
        x: b.x0 + (w - tw) / 2.0,
        y: b.y0 + (h - th) / 2.0,
        width: tw,
    };
    paint_label(frame, fonts, cache, &placed, theme.bg, theme);
}

/// The tooltip: a small box under the hovered button, kept inside the window (the find bar's
/// buttons have them too).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_tooltip(
    frame: &mut Frame,
    ctx: &Ctx,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    text: &mut TextCtx,
    theme: &Theme,
    label: &str,
    under: Rect,
) {
    let s = ctx.scale;
    let pad_x = 8.0 * s;
    let y0 = under.y1 + 4.0 * s;
    let y1 = y0 + (LABEL * 1.3 + 8.0) * s;
    // The text is centered between the box's top and bottom; its left edge is set below.
    let mut placed = layout_label(fonts, text, label, Rect { y0, y1, ..under }, s, true);
    let bw = placed.width + 2.0 * pad_x;
    let x0 = ((under.x0 + under.x1 - bw) / 2.0)
        .min(ctx.width as f32 - bw - EDGE * s)
        .max(EDGE * s);
    let b = Rect {
        x0,
        y0,
        x1: x0 + bw,
        y1,
    };
    frame.rounded_rect(b.px(), (4.0 * s).round(), None, theme.text, 235);
    placed.x = x0 + pad_x;
    paint_label(frame, fonts, cache, &placed, theme.bg, theme);
}

#[cfg(test)]
mod tests;
