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
}

/// `0x00RRGGBB`, the softbuffer pixel format.
pub type Rgb = u32;

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
        }
    }
}
