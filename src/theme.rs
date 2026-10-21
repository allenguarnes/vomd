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
}

/// `0x00RRGGBB`, the softbuffer pixel format.
pub type Rgb = u32;

#[derive(Clone, Copy)]
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
        }
    }
}
