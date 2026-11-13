//! Every visual value the app draws with: colours from a CHROTE theme
//! (schema 1, the file CHROTE's `/api/theme` serves and a node's `/theme`
//! passes on), and the fixed measures of the chrome. Views read these and
//! never write a colour or size of their own.

use gpui::{App, Global, Hsla, Rgba};
use serde::Deserialize;

/// Measures in pixels, from CHROTE's layout (DESIGN-SYSTEM.md and the
/// dashboard as it renders). A page's own measures live with the page.
pub mod size {
    pub const TAB_BAR: f32 = 44.;
    pub const STATUS: f32 = 28.;
    pub const ROW: f32 = 28.;
    pub const CONTROL: f32 = 28.;
    pub const RADIUS: f32 = 4.;
    /// Chrome text and its small companion (labels, counts, hints).
    pub const TEXT: f32 = 13.;
    pub const SMALL: f32 = 11.;
    /// The widest a notice over an empty page grows (a failed page's).
    pub const NOTICE: f32 = 420.;
    /// The name column of a page of settings or help.
    pub const LABEL_COL: f32 = 240.;
    /// The keys panel and a menu: their widths.
    pub const KEYS: f32 = 520.;
    pub const MENU: f32 = 220.;
    /// A tab card's gap above it, and a pill's above and below.
    pub const TAB_CARD_TOP: f32 = 6.;
    pub const TAB_PILL_Y: f32 = 8.;
    /// The underline of an active tab or toggle in the line chrome.
    pub const TAB_LINE: f32 = 2.;
    /// A list row's inset from its list's sides, where rows are inset.
    pub const ROW_INSET: f32 = 6.;
}

/// Measures the pages that list things share (a rail of rows beside a
/// reader, a table, a device's room), carried over from the old app.
pub mod board {
    /// A page's padding, and the gap between its parts.
    pub const PAD: f32 = 24.;
    pub const GAP: f32 = 16.;
    /// A page heading.
    pub const HEADING: f32 = 18.;
    /// The side list beside a detail.
    pub const LIST_W: f32 = 380.;
    /// A reading column's widest.
    pub const READER_MAX: f32 = 760.;
    /// A device's room has walls this thick.
    pub const WALL: f32 = 2.;
    /// A table row, and the status glyph in it.
    pub const ROW: f32 = 34.;
    pub const GLYPH: f32 = 8.;
}

/// How much of a thing shows when it steps back.
pub mod fade {
    pub const HOVER: f32 = 0.9;
    /// A control with nothing to do now.
    pub const IDLE: f32 = 0.5;
}

/// Toast timing in milliseconds: in, shown, out.
pub mod motion {
    pub const TOAST_IN: u64 = 120;
    pub const TOAST_SHOWN: u64 = 1800;
    pub const TOAST_OUT: u64 = 200;
}

pub const FAMILY: &str = "JetBrains Mono";

/// How the chrome draws its tabs and lists: the tab bar, a page's own
/// toggles, a list of rows. One is the app's; the others stay a word away
/// (`PARVI_CHROME=line|pills`) while the operator compares them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chrome {
    /// Tabs as cards: the active one is the page's own surface, joined to
    /// it; rows are inset, their state a quiet fill.
    #[default]
    Tabs,
    /// Words on one line: the active one underlined; rows run edge to edge.
    Line,
    /// Words in rounded fills: the active one washed in the accent.
    Pills,
}

impl Global for Chrome {}

impl Chrome {
    /// The one `PARVI_CHROME` names, else the app's.
    pub fn from_env() -> Self {
        Self::named(&std::env::var("PARVI_CHROME").unwrap_or_default())
    }

    fn named(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "line" => Self::Line,
            "pills" => Self::Pills,
            _ => Self::Tabs,
        }
    }
}

pub fn chrome(cx: &App) -> Chrome {
    cx.try_global::<Chrome>().copied().unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub divider: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub dim: Hsla,
    pub accent: Hsla,
    pub error: Hsla,
    pub term_bg: Hsla,
    pub term_fg: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    /// ANSI 0..16 as 0xRRGGBB, for the terminal's palette.
    pub ansi: [u32; 16],
    pub term_bg_rgb: u32,
    pub term_fg_rgb: u32,
    pub identity: Vec<Hsla>,
    /// Art file names the node serves under `/theme/art/`.
    pub art: Vec<String>,
}

impl Global for Theme {}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

pub fn rgb(c: u32) -> Hsla {
    rgba(c, 1.)
}

pub fn rgba(c: u32, a: f32) -> Hsla {
    Rgba {
        r: ((c >> 16) & 0xff) as f32 / 255.,
        g: ((c >> 8) & 0xff) as f32 / 255.,
        b: (c & 0xff) as f32 / 255.,
        a,
    }
    .into()
}

pub fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<(u32, f32)> {
    let h = s.trim().strip_prefix('#')?;
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => h.into(),
        _ => return None,
    };
    let v = u32::from_str_radix(&full[..6], 16).ok()?;
    let a = if full.len() == 8 {
        u8::from_str_radix(&full[6..], 16).ok()? as f32 / 255.
    } else {
        1.
    };
    Some((v, a))
}

#[derive(Deserialize)]
struct File {
    schema: u32,
    ui: FileUi,
    terminal: FileTerm,
    #[serde(default)]
    identity: Vec<String>,
    #[serde(default)]
    art: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileUi {
    background: String,
    surface: String,
    surface_raised: String,
    divider: String,
    text: String,
    text_secondary: String,
    text_dim: String,
    accent: String,
    error: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileTerm {
    background: String,
    foreground: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    selection_background: Option<String>,
    ansi: Vec<String>,
}

impl Theme {
    /// Parvi's own palette, chrote-violet (ui/src/styles/tokens.css), for a
    /// node that serves no theme.
    pub fn violet() -> Self {
        Self::parse(include_str!("../assets/chrote-violet.json"))
            .expect("the embedded theme parses")
    }

    pub fn parse(json: &str) -> Result<Self, String> {
        let f: File = serde_json::from_str(json).map_err(|e| format!("theme: {e}"))?;
        if f.schema != 1 {
            return Err(format!("theme schema {} is not 1", f.schema));
        }
        let c = |s: &str| {
            parse_hex(s)
                .map(|(v, a)| rgba(v, a))
                .ok_or_else(|| format!("theme colour {s:?}"))
        };
        let v = |s: &str| {
            parse_hex(s)
                .map(|(v, _)| v)
                .ok_or_else(|| format!("theme colour {s:?}"))
        };
        if f.terminal.ansi.len() != 16 {
            return Err("theme terminal.ansi needs 16 colours".into());
        }
        let mut ansi = [0u32; 16];
        for (i, a) in f.terminal.ansi.iter().enumerate() {
            ansi[i] = v(a)?;
        }
        let accent = c(&f.ui.accent)?;
        let selection = match &f.terminal.selection_background {
            Some(s) => c(s)?,
            None => alpha(accent, 0.25),
        };
        let mut identity = f
            .identity
            .iter()
            .map(|s| c(s))
            .collect::<Result<Vec<_>, _>>()?;
        if identity.is_empty() {
            identity.push(c(&f.ui.text_dim)?);
        }
        Ok(Self {
            bg: c(&f.ui.background)?,
            surface: c(&f.ui.surface)?,
            raised: c(&f.ui.surface_raised)?,
            divider: c(&f.ui.divider)?,
            text: c(&f.ui.text)?,
            text2: c(&f.ui.text_secondary)?,
            dim: c(&f.ui.text_dim)?,
            accent,
            error: c(&f.ui.error)?,
            term_bg: c(&f.terminal.background)?,
            term_fg: c(&f.terminal.foreground)?,
            cursor: c(f
                .terminal
                .cursor
                .as_deref()
                .unwrap_or(&f.terminal.foreground))?,
            selection,
            ansi,
            term_bg_rgb: v(&f.terminal.background)?,
            term_fg_rgb: v(&f.terminal.foreground)?,
            identity,
            art: f
                .art
                .into_iter()
                .filter(|a| !a.contains('/') && !a.contains('\\'))
                .collect(),
        })
    }

    /// The accent at the washes CHROTE mixes: 12% for a chosen thing, 7% for
    /// the current row.
    pub fn accent_light(&self) -> Hsla {
        alpha(self.accent, 0.12)
    }
    pub fn accent_soft(&self) -> Hsla {
        alpha(self.accent, 0.07)
    }

    /// The colour of a Unix user's square, stable per name.
    pub fn identity_of(&self, user: &str) -> Hsla {
        let h = user
            .bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        self.identity[h as usize % self.identity.len()]
    }

    /// Text on an accent fill.
    pub fn on_accent(&self) -> Hsla {
        self.bg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chrome_is_named_by_a_word_and_defaults_to_tabs() {
        assert_eq!(Chrome::named("Line"), Chrome::Line);
        assert_eq!(Chrome::named(" pills"), Chrome::Pills);
        assert_eq!(Chrome::named(""), Chrome::Tabs);
        assert_eq!(Chrome::named("neon"), Chrome::Tabs);
    }

    #[test]
    fn hex_forms() {
        assert_eq!(parse_hex("#fff"), Some((0xffffff, 1.)));
        assert_eq!(parse_hex("#6b9fff"), Some((0x6b9fff, 1.)));
        let (v, a) = parse_hex("#6b9fff40").unwrap();
        assert_eq!(v, 0x6b9fff);
        assert!((a - 0.25).abs() < 0.01);
        assert_eq!(parse_hex("6b9fff"), None);
    }

    #[test]
    fn the_embedded_theme_and_chrote_dark_parse() {
        let t = Theme::violet();
        assert_eq!(t.ansi[1], 0xf87171);
        let dark = r##"{"schema":1,"name":"chrote-dark","ui":{"background":"#0f0f0f","surface":"#1a1a1a","surfaceRaised":"#252525","divider":"#3a3a3a","text":"#e5e5e5","textSecondary":"#a3a3a3","textDim":"#737373","accent":"#6b9fff","error":"#f87171"},"terminal":{"background":"#0a0a0a","foreground":"#e5e5e5","cursor":"#e5e5e5","selectionBackground":"#6b9fff40","ansi":["#0f0f0f","#f87171","#8bd450","#e5c07b","#6b9fff","#c084fc","#45d6d6","#a3a3a3","#737373","#ff8a8a","#a6e37a","#f0d48a","#8fb5ff","#d3a4ff","#7ae2e2","#ffffff"]},"identity":["#4f6d8f"],"art":["town.webp","../x.webp"]}"##;
        let t = Theme::parse(dark).unwrap();
        assert_eq!(t.term_bg_rgb, 0x0a0a0a);
        assert_eq!(t.art, vec!["town.webp".to_string()]);
    }

    #[test]
    fn another_schema_is_refused() {
        assert!(Theme::parse(r#"{"schema":2,"ui":{},"terminal":{}}"#).is_err());
    }
}
