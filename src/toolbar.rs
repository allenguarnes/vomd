//! The toolbar: one slim bar docked at the top, never over the text.
//!
//! Left: Back. Right: zoom (− 100% +), theme, reload, settings. Buttons for features that
//! aren't built yet are absent. On narrow windows the right group collapses from the left (zoom
//! first, then theme, ...) into a `⋯` button with a small dropdown, the one popup in the app.
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
/// Text size of the percentage, tooltips and menu rows.
const LABEL: f32 = 13.0;
/// Menu row height and width.
const ROW: f32 = 28.0;
const MENU_W: f32 = 230.0;
/// How long the pointer rests on a button before its tooltip shows.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// Lucide code points in `assets/fonts/icons.ttf`.
mod icon {
    pub const BACK: char = '\u{e048}';
    pub const MINUS: char = '\u{e11c}';
    pub const PLUS: char = '\u{e13d}';
    pub const THEME: char = '\u{e09d}';
    pub const RELOAD: char = '\u{e149}';
    pub const SETTINGS: char = '\u{e154}';
    pub const MORE: char = '\u{e0b6}';
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Back,
    ZoomOut,
    /// The percentage between − and +; clicking it resets the zoom.
    ZoomReset,
    ZoomIn,
    Theme,
    Reload,
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
    /// There is a settings file to open (not with `--no-settings`).
    pub settings: bool,
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

    fn px(&self) -> (i32, i32, i32, i32) {
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
            Item::ZoomOut => "Zoom out (Ctrl+−)".into(),
            Item::ZoomReset => format!("Reset zoom {}% (Ctrl+0)", ctx.zoom),
            Item::ZoomIn => "Zoom in (Ctrl+=)".into(),
            Item::Theme => match ctx.theme {
                ThemeMode::System => "Theme: system (T)",
                ThemeMode::Light => "Theme: light (T)",
                ThemeMode::Dark => "Theme: dark (T)",
            }
            .into(),
            Item::Reload => "Reload (R)".into(),
            Item::Settings => "Settings".into(),
            Item::More => "More".into(),
        }
    }

    fn icon(self) -> Option<char> {
        Some(match self {
            Item::Back => icon::BACK,
            Item::ZoomOut => icon::MINUS,
            Item::ZoomIn => icon::PLUS,
            Item::Theme => icon::THEME,
            Item::Reload => icon::RELOAD,
            Item::Settings => icon::SETTINGS,
            Item::More => icon::MORE,
            Item::ZoomReset => return None,
        })
    }

    fn command(self) -> Option<Command> {
        Some(match self {
            Item::Back => Command::Action(Action::Back),
            Item::ZoomOut => Command::Action(Action::ZoomOut),
            Item::ZoomReset => Command::Action(Action::ZoomReset),
            Item::ZoomIn => Command::Action(Action::ZoomIn),
            Item::Theme => Command::Action(Action::CycleTheme),
            Item::Reload => Command::Action(Action::Reload),
            Item::Settings => Command::Settings,
            Item::More => return None,
        })
    }

    fn enabled(self, ctx: &Ctx) -> bool {
        self != Item::Back || ctx.can_back
    }
}

/// The right group's runs of items, in collapse order (first collapses first) and with their
/// logical widths.
fn right_groups(ctx: &Ctx) -> &'static [&'static [(Item, f32)]] {
    const ALL: [&[(Item, f32)]; 4] = [
        &[
            (Item::ZoomOut, BUTTON),
            (Item::ZoomReset, PERCENT),
            (Item::ZoomIn, BUTTON),
        ],
        &[(Item::Theme, BUTTON)],
        &[(Item::Reload, BUTTON)],
        &[(Item::Settings, BUTTON)],
    ];
    if ctx.settings { &ALL } else { &ALL[..3] }
}

/// The bar's layout for this frame: Back on the left, the right group laid out from the right
/// edge, and as many of its leading groups as don't fit collapsed into `⋯`.
pub fn slots(ctx: &Ctx) -> Slots {
    let s = ctx.scale;
    let w = ctx.width as f32;
    let h = BAR * s;
    let groups = right_groups(ctx);
    let width_of = |g: &[(Item, f32)]| g.iter().map(|(_, gw)| gw * s).sum::<f32>();
    let left = (EDGE + BUTTON + GAP) * s;
    let total: f32 = groups.iter().map(|g| width_of(g)).sum();
    // Collapse groups from the left until the rest, plus `⋯`, fits.
    let mut first_shown = 0;
    if left + total + EDGE * s > w {
        let mut rest = total + BUTTON * s;
        while first_shown < groups.len() && left + rest + EDGE * s > w {
            rest -= width_of(groups[first_shown]);
            first_shown += 1;
        }
    }
    let mut items = vec![(
        Item::Back,
        Rect {
            x0: EDGE * s,
            y0: 0.0,
            x1: (EDGE + BUTTON) * s,
            y1: h,
        },
    )];
    let mut x = w - EDGE * s;
    for (item, gw) in groups[first_shown..].iter().flat_map(|g| g.iter()).rev() {
        let x0 = x - gw * s;
        items.push((
            *item,
            Rect {
                x0,
                y0: 0.0,
                x1: x,
                y1: h,
            },
        ));
        x = x0;
    }
    let overflow: Vec<Item> = groups[..first_shown]
        .iter()
        .flat_map(|g| g.iter())
        .map(|(i, _)| *i)
        .collect();
    if !overflow.is_empty() {
        // Never over Back: in a window too narrow for both, `⋯` runs off the right edge.
        let x0 = (x - BUTTON * s).max((EDGE + BUTTON) * s);
        items.push((
            Item::More,
            Rect {
                x0,
                y0: 0.0,
                x1: x0 + BUTTON * s,
                y1: h,
            },
        ));
    }
    Slots { items, overflow }
}

/// The dropdown's rows: right-aligned under `⋯`, kept inside the window.
fn menu_rows(ctx: &Ctx, slots: &Slots) -> Vec<(Item, Rect)> {
    let s = ctx.scale;
    let Some((_, more)) = slots.items.iter().find(|(i, _)| *i == Item::More) else {
        return Vec::new();
    };
    let w = (MENU_W * s).min(ctx.width as f32);
    let x1 = more.x1.max(w).min(ctx.width as f32);
    let x0 = x1 - w;
    let y0 = BAR * s + 2.0 * s;
    slots
        .overflow
        .iter()
        .enumerate()
        .map(|(n, &item)| {
            let top = y0 + (EDGE * s) + n as f32 * ROW * s;
            (
                item,
                Rect {
                    x0,
                    y0: top,
                    x1,
                    y1: top + ROW * s,
                },
            )
        })
        .collect()
}

/// The dropdown's whole box (rows plus padding).
fn menu_box(ctx: &Ctx, rows: &[(Item, Rect)]) -> Option<Rect> {
    let first = rows.first()?.1;
    let last = rows.last()?.1;
    let pad = EDGE * ctx.scale;
    Some(Rect {
        x0: first.x0,
        y0: first.y0 - pad,
        x1: first.x1,
        y1: last.y1 + pad,
    })
}

#[derive(Clone, Debug, Default)]
pub struct Toolbar {
    hidden: bool,
    hover: Option<Spot>,
    pressed: Option<Spot>,
    menu: bool,
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

    fn spot_at(&self, ctx: &Ctx, x: f64, y: f64) -> Option<Spot> {
        if self.hidden {
            return None;
        }
        let slots = slots(ctx);
        if self.menu
            && let Some(n) = menu_rows(ctx, &slots)
                .iter()
                .position(|(_, r)| r.contains(x, y))
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
        if y >= 0.0 && y < f64::from(self.height(ctx.scale)) {
            return true;
        }
        self.menu && menu_box(ctx, &menu_rows(ctx, &slots(ctx))).is_some_and(|b| b.contains(x, y))
    }

    /// The pointer moved to (`x`, `y`). Returns true if the toolbar needs a repaint.
    pub fn pointer(&mut self, ctx: &Ctx, x: f64, y: f64, now: Instant) -> bool {
        let spot = self.spot_at(ctx, x, y);
        if spot == self.hover {
            return false;
        }
        self.hover = spot;
        self.tooltip = false;
        self.hover_since = matches!(spot, Some(Spot::Bar(_))).then_some(now);
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
        let spot = self.spot_at(ctx, x, y);
        self.tooltip = false;
        self.hover_since = None;
        match spot {
            Some(spot) => {
                self.pressed = Some(spot);
                true
            }
            None if self.menu => {
                self.menu = false;
                true
            }
            None => self.covers(ctx, x, y),
        }
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
                self.menu = !self.menu;
                return (true, None);
            }
            Spot::Bar(item) => item,
            Spot::Row(n) => {
                self.menu = false;
                match menu_rows(ctx, &slots(ctx)).get(n) {
                    Some(&(item, _)) => item,
                    None => return (true, None),
                }
            }
        };
        let command = item.enabled(ctx).then(|| item.command()).flatten();
        (true, command)
    }

    /// Close the dropdown (Escape, a resize that changes what overflows). Returns true if it
    /// was open.
    pub fn close_menu(&mut self) -> bool {
        std::mem::take(&mut self.menu)
    }

    pub fn menu_open(&self) -> bool {
        self.menu
    }

    /// When the loop must wake to show the tooltip (`None`: no timer).
    pub fn deadline(&self) -> Option<Instant> {
        if self.tooltip || self.menu {
            return None;
        }
        self.hover_since.map(|t| t + TOOLTIP_DELAY)
    }

    /// Show the tooltip if its time has come. Returns true if the window needs a repaint.
    pub fn expire(&mut self, now: Instant) -> bool {
        match self.deadline() {
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
        for &(item, rect) in &slots.items {
            let spot = Spot::Bar(item);
            let on = item == Item::More && self.menu;
            self.paint_item(frame, ctx, item, rect, spot, on, fonts, cache, text, theme);
        }
        if self.menu {
            let rows = menu_rows(ctx, &slots);
            if let Some(b) = menu_box(ctx, &rows) {
                let r = (4.0 * s).round();
                frame.rounded_rect(b.px(), r, None, theme.bg, 255);
                frame.rounded_rect(b.px(), r, Some(line as f32), theme.border, 255);
            }
            // Row text stays inside the box, however narrow the window.
            let prev = rows.first().map(|(_, r)| {
                frame.set_clip(render::Clip {
                    x0: r.px().0,
                    y0: 0,
                    x1: r.px().2 - (EDGE * s) as i32,
                    y1: ctx.height as i32,
                })
            });
            for (n, &(item, rect)) in rows.iter().enumerate() {
                let hot = self.hover == Some(Spot::Row(n));
                if hot {
                    frame.fill_rect(
                        rect.px().0 + line,
                        rect.px().1,
                        rect.px().2 - line,
                        rect.px().3,
                        theme.code_bg,
                    );
                }
                let color = if item.enabled(ctx) {
                    theme.text
                } else {
                    theme.border
                };
                let icon_box = Rect {
                    x1: rect.x0 + ROW * s,
                    ..rect
                };
                if let Some(c) = item.icon() {
                    draw_icon(frame, fonts, cache, c, icon_box, s, color);
                }
                let label_box = Rect {
                    x0: icon_box.x1,
                    x1: rect.x1 - EDGE * s,
                    ..rect
                };
                let p = layout_label(fonts, text, &item.label(ctx), label_box, s, true);
                paint_label(frame, fonts, cache, &p, color, theme);
            }
            if let Some(prev) = prev {
                frame.set_clip(prev);
            }
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
        if hot || down || on {
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
        let color = if !enabled {
            theme.border
        } else if hot || on {
            theme.text
        } else {
            theme.muted
        };
        match item.icon() {
            Some(c) => draw_icon(frame, fonts, cache, c, rect, s, color),
            None => {
                let p = layout_label(fonts, text, &format!("{}%", ctx.zoom), rect, s, false);
                paint_label(frame, fonts, cache, &p, color, theme);
            }
        }
    }
}

/// An icon glyph centered in `rect` (by its ink: the Lucide glyphs fill their em box).
fn draw_icon(
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
    let mut t = Theme { ..*theme };
    t.text = color;
    render::paint_block(frame, &placed.content, placed.x, placed.y, fonts, cache, &t);
}

/// The tooltip: a small box under the hovered button, kept inside the window.
#[allow(clippy::too_many_arguments)]
fn paint_tooltip(
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
    let mut placed = layout_label(fonts, text, label, under, s, true);
    let lw = placed.width;
    let (pad_x, pad_y) = (8.0 * s, 4.0 * s);
    let bw = lw + 2.0 * pad_x;
    let bh = LABEL * 1.3 * s + 2.0 * pad_y;
    let x0 = ((under.x0 + under.x1 - bw) / 2.0)
        .min(ctx.width as f32 - bw - EDGE * s)
        .max(EDGE * s);
    let y0 = under.y1 + 4.0 * s;
    let b = Rect {
        x0,
        y0,
        x1: x0 + bw,
        y1: y0 + bh,
    };
    frame.rounded_rect(b.px(), (4.0 * s).round(), None, theme.text, 235);
    placed.x = b.x0 + pad_x;
    placed.y += b.y0 + (b.y1 - b.y0) / 2.0 - (under.y0 + under.y1) / 2.0;
    paint_label(frame, fonts, cache, &placed, theme.bg, theme);
}

#[cfg(test)]
mod tests;
