//! Color roles and the two palettes. Layout refers to roles; paint resolves them.
//! Colors follow GitHub's Primer light/dark palettes so documents look the way authors expect.

/// Semantic color used by layout primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Bg,
    Text,
    Muted,
    Link,
    CodeBg,
    Border,
    BorderMuted,
    QuoteBar,
    TableStripe,
    AlertNote,
    AlertTip,
    AlertImportant,
    AlertWarning,
    AlertCaution,
    /// Behind selected text.
    #[cfg(any(feature = "clipboard", feature = "find"))]
    Selection,
    /// Behind the matches of a find, and behind the current one.
    #[cfg(feature = "find")]
    FindMatch,
    #[cfg(feature = "find")]
    FindCurrent,
    /// A token in a highlighted code block (`Tok::Plain` is `Text`).
    #[cfg(feature = "highlight")]
    Syntax(crate::highlight::Tok),
}

/// `0x00RRGGBB`, the softbuffer pixel format.
pub type Rgb = u32;

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub bg: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub link: Rgb,
    pub code_bg: Rgb,
    pub border: Rgb,
    pub border_muted: Rgb,
    pub quote_bar: Rgb,
    pub table_stripe: Rgb,
    pub alerts: [Rgb; 5],
    /// Painted under selected text, which keeps its own color: a blue that reads on the page.
    #[cfg(any(feature = "clipboard", feature = "find"))]
    pub selection: Rgb,
    /// Painted under the matches of a find, and under the current match: a soft yellow, and a
    /// stronger orange, that text still reads on.
    #[cfg(feature = "find")]
    pub find_match: Rgb,
    #[cfg(feature = "find")]
    pub find_current: Rgb,
    /// Code tokens, by `Tok` from `Comment` to `Meta`: Primer's "prettylights" colours, except
    /// a diff's inserted and deleted lines, which take the success and danger colours (Primer's
    /// own are made for a tinted background).
    #[cfg(feature = "highlight")]
    pub syntax: [Rgb; 10],
}

pub const LIGHT: Theme = Theme {
    bg: 0xffffff,
    text: 0x1f2328,
    muted: 0x59636e,
    link: 0x0969da,
    code_bg: 0xf6f8fa,
    border: 0xd1d9e0,
    border_muted: 0xd1d9e0,
    quote_bar: 0xd1d9e0,
    table_stripe: 0xf6f8fa,
    alerts: [0x0969da, 0x1a7f37, 0x8250df, 0x9a6700, 0xcf222e],
    // Primer's #0969da at 20% over white.
    #[cfg(any(feature = "clipboard", feature = "find"))]
    selection: 0xcde1f8,
    #[cfg(feature = "find")]
    find_match: 0xffe98f,
    #[cfg(feature = "find")]
    find_current: 0xffb25c,
    #[cfg(feature = "highlight")]
    syntax: [
        0x59636e, 0xcf222e, 0x0a3069, 0x0550ae, 0x6639ba, 0x0550ae, 0x953800, 0x1a7f37, 0xcf222e,
        0x8250df,
    ],
};

pub const DARK: Theme = Theme {
    bg: 0x0d1117,
    text: 0xf0f6fc,
    muted: 0x9198a1,
    link: 0x4493f8,
    code_bg: 0x151b23,
    border: 0x3d444d,
    border_muted: 0x3d444d,
    quote_bar: 0x3d444d,
    table_stripe: 0x151b23,
    alerts: [0x4493f8, 0x3fb950, 0xab7df8, 0xd29922, 0xf85149],
    // Primer's #388bfd at 40% over the page.
    #[cfg(any(feature = "clipboard", feature = "find"))]
    selection: 0x1e4273,
    #[cfg(feature = "find")]
    find_match: 0x5c4a14,
    #[cfg(feature = "find")]
    find_current: 0x9c5a14,
    #[cfg(feature = "highlight")]
    syntax: [
        0x9198a1, 0xff7b72, 0xa5d6ff, 0x79c0ff, 0xd2a8ff, 0x7ee787, 0xffa657, 0x3fb950, 0xf85149,
        0xd2a8ff,
    ],
};

impl Theme {
    pub fn color(&self, role: Role) -> Rgb {
        match role {
            Role::Bg => self.bg,
            Role::Text => self.text,
            Role::Muted => self.muted,
            Role::Link => self.link,
            Role::CodeBg => self.code_bg,
            Role::Border => self.border,
            Role::BorderMuted => self.border_muted,
            Role::QuoteBar => self.quote_bar,
            Role::TableStripe => self.table_stripe,
            Role::AlertNote => self.alerts[0],
            Role::AlertTip => self.alerts[1],
            Role::AlertImportant => self.alerts[2],
            Role::AlertWarning => self.alerts[3],
            Role::AlertCaution => self.alerts[4],
            #[cfg(any(feature = "clipboard", feature = "find"))]
            Role::Selection => self.selection,
            #[cfg(feature = "find")]
            Role::FindMatch => self.find_match,
            #[cfg(feature = "find")]
            Role::FindCurrent => self.find_current,
            #[cfg(feature = "highlight")]
            Role::Syntax(crate::highlight::Tok::Plain) => self.text,
            #[cfg(feature = "highlight")]
            Role::Syntax(t) => self.syntax[t as usize - 1],
        }
    }
}

/// An Omarchy theme's `colors.toml` (`key = "#rrggbb"` lines) as a palette, and whether it is
/// dark. Colours it doesn't name, and accents that wouldn't read on its background, keep
/// GitHub's for that mode. `None` without a background and a foreground.
pub fn omarchy(toml: &str) -> Option<(bool, Theme)> {
    let value = |key: &str| {
        toml.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"'))
        })
    };
    let color = |key: &str| {
        let hex = value(key)?.strip_prefix('#').filter(|h| h.len() == 6)?;
        Rgb::from_str_radix(hex, 16).ok()
    };
    let (bg, text) = (color("background")?, color("foreground")?);
    let dark = value("mode").map_or(luminance(bg) < 0.18, |m| m != "light");
    let base = if dark { DARK } else { LIGHT };
    let ink = |key: &str| color(key).filter(|&c| contrast(c, bg) >= 3.0);
    let or = |key: &str, fallback: Rgb| ink(key).unwrap_or(fallback);
    let muted = mix(text, bg, 0.4);
    let code_bg = color("dark_background").unwrap_or(base.code_bg);
    let border = color("muted").unwrap_or(base.border);
    #[cfg(feature = "highlight")]
    let s = base.syntax;
    Some((
        dark,
        Theme {
            bg,
            text,
            muted,
            link: or("accent", base.link),
            code_bg,
            border,
            border_muted: border,
            quote_bar: border,
            table_stripe: code_bg,
            alerts: [
                or("blue", base.alerts[0]),
                or("green", base.alerts[1]),
                or("magenta", base.alerts[2]),
                or("yellow", base.alerts[3]),
                or("red", base.alerts[4]),
            ],
            #[cfg(any(feature = "clipboard", feature = "find"))]
            selection: color("selection").unwrap_or(base.selection),
            // By `Tok`, Comment to Meta, as GitHub's roles: orange is in only some themes.
            #[cfg(feature = "highlight")]
            syntax: [
                muted,
                or("red", s[1]),
                or("cyan", s[2]),
                or("blue", s[3]),
                or("magenta", s[4]),
                or("green", s[5]),
                ink("orange").or_else(|| ink("yellow")).unwrap_or(s[6]),
                or("green", s[7]),
                or("red", s[8]),
                or("magenta", s[9]),
            ],
            // GitHub's washes: they are made to keep any text on its page readable.
            #[cfg(feature = "find")]
            find_match: base.find_match,
            #[cfg(feature = "find")]
            find_current: base.find_current,
        },
    ))
}

/// `a` moved toward `b` by `t` (0..=1), per channel.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [16, 8, 0].iter().fold(0, |out, &s| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        out | ((x + (y - x) * t).round() as Rgb) << s
    })
}

/// WCAG relative luminance.
fn luminance(c: Rgb) -> f32 {
    let ch = |s: u32| {
        let v = ((c >> s) & 0xff) as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

/// WCAG contrast ratio, 1 to 21.
fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omarchy_colors_become_the_palette_of_their_mode() {
        let toml = "mode = \"light\"\naccent = \"#1e66f5\"\nbackground = \"#eff1f5\"\n\
                    foreground = \"#4c4f69\"\nyellow = \"#df8e1d\"\nred = \"#d20f39\"\nbogus\n";
        let (dark, t) = omarchy(toml).unwrap();
        assert!(!dark);
        assert_eq!((t.bg, t.text, t.link), (0xeff1f5, 0x4c4f69, 0x1e66f5));
        assert_eq!(t.alerts[4], 0xd20f39);
        // Yellow on near-white doesn't read: GitHub's stays.
        assert_eq!(t.alerts[3], LIGHT.alerts[3]);
        // Not named: GitHub's light ones.
        assert_eq!(t.code_bg, LIGHT.code_bg);
        assert_eq!(t.alerts[1], LIGHT.alerts[1]);
    }

    #[test]
    fn omarchy_mode_defaults_to_the_backgrounds_and_needs_both_ends() {
        let dark = "background = \"#101315\"\nforeground = \"#cacccc\"";
        assert!(omarchy(dark).unwrap().0);
        assert!(omarchy("background = \"#101315\"").is_none());
        assert!(omarchy("background = \"#1013\"\nforeground = \"#cacccc\"").is_none());
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert!((contrast(0x000000, 0xffffff) - 21.0).abs() < 0.01);
    }
}
