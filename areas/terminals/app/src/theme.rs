//! The page's tokens: the app's own, and the measures, materials and fades
//! only the terminal workspace draws with. Views read these and never write
//! a colour or size of their own.

pub use parvi_app_kit::theme::*;

/// Measures in pixels, from CHROTE's layout (DESIGN-SYSTEM.md and the
/// dashboard as it renders), and the app's own.
pub mod size {
    pub use parvi_app_kit::theme::size::*;

    pub const STRIP: f32 = 44.;
    /// A tab's name field while it is renamed in place.
    pub const TAB_FIELD: f32 = 180.;
    /// A shell's `>_` mark, as a share of the mark's size.
    pub const SHELL_MARK: f32 = 0.8;
    pub const RAIL: f32 = 260.;
    /// Name budget in the standard rail, leaving room for age and actions.
    pub const RAIL_NAME_COLUMNS: usize = 22;
    /// The room a rail row keeps for its news dot.
    pub const DOT_SLOT: f32 = 12.;
    /// A session that waits for an answer: its mark, filling the dot slot.
    pub const ASK_MARK: f32 = 12.;
    /// A rail row's menu button, shown over the row's end on hover.
    pub const ROW_MENU: f32 = 20.;
    /// What another page opens beside the windows (a file a link named).
    pub const BESIDE: f32 = 560.;
    /// The least a view beside the windows narrows to, to leave them room.
    pub const BESIDE_MIN: f32 = 400.;
    /// The width a window keeps readable: under it a drawer lies over the
    /// windows rather than narrowing them.
    pub const WINDOW_MIN: f32 = 440.;
    pub const TILE_HEAD: f32 = 34.;
    pub const TILE_GAP: f32 = 6.;
    /// How far either side of the gap between two windows still counts as
    /// the seam a drag can make a window in (CHROTE's GAP_HIT_PAD).
    pub const SEAM_PAD: f32 = 12.;
    /// The widest a session's tag grows in a window's head.
    pub const TAG_MAX: f32 = 320.;
    /// The Send drawer: its width with nothing remembered, the least it may
    /// be, and what the workspace beside it keeps (CHROTE's SendDrawer).
    pub const DRAWER: f32 = 380.;
    pub const DRAWER_MIN: f32 = 320.;
    pub const DRAWER_ROOM: f32 = 480.;
    /// Scrollback search shares the same space rules as Send.
    pub const FIND_DRAWER: f32 = 460.;
    /// The grip at the drawer's inner edge that resizes it.
    pub const DRAWER_GRIP: f32 = 6.;
    pub const LAUNCHER: f32 = 420.;
    /// The share of an empty window above its launcher.
    pub const LAUNCHER_TOP: f32 = 0.12;
    /// Peek covers this share of the page; its header is this tall.
    pub const PEEK_SHARE: f32 = 0.9;
    pub const PEEK_HEAD: f32 = 30.;
    /// An Overview card and the screen text inside it.
    pub const THUMB_W: f32 = 300.;
    pub const THUMB_H: f32 = 220.;
    pub const THUMB_LINES: usize = 14;
    pub const THUMB_TEXT: f32 = 9.;
    pub const THUMB_LINE: f32 = 12.;
    /// Space between a window's edge and its terminal grid.
    pub const TERM_PAD: f32 = 4.;
    /// Between a history and the live input docked below it; the rule
    /// between them runs through the middle.
    pub const DOCK_GAP: f32 = 9.;
    /// The way back to the live end, just above that rule.
    pub const LIVE_PILL_H: f32 = 20.;
    /// Send's target list shows at most this many whole rows, then scrolls.
    pub const SEND_TARGET_ROWS: f32 = 9.;
    /// Terminal font: Settings may pick 12..=20; the fit never goes below
    /// the floor, and clips instead.
    pub const TERM_DEFAULT: f32 = 14.;
    pub const TERM_MIN: f32 = 12.;
    pub const TERM_MAX: f32 = 20.;
    pub const TERM_FLOOR: f32 = 11.;
}

/// How much of a thing shows when it steps back.
pub mod fade {
    /// A device that went away: its windows and rows.
    pub const DIMMED: f32 = 0.5;
}

/// Devices and environments (the operator's direction, 2026-11-08): a
/// device is the frame, a solid plate in its own colour; an environment is
/// the material inside, a named strip with its own texture. Change the
/// treatment here and in `view/place.rs`, nowhere else.
pub mod place {
    /// Plate colours, picked per device name.
    pub const DEVICES: [u32; 6] = [0x3d5a80, 0x7a4f2e, 0x3f6b4a, 0x6a4a7d, 0x7d4a55, 0x2f6b6b];
    pub const PLATE_H: f32 = 18.;
    /// The device's colour along the left of each of its rows.
    pub const EDGE: f32 = 3.;
    pub const STRIP_H: f32 = 18.;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Texture {
        /// WSL: diagonal hatching.
        Hatch,
        /// Windows: two horizontal rules.
        Rules,
        /// Linux and anything else: a dot grid.
        Dots,
    }

    /// The environment's word and texture. `environment` is the hello's.
    pub fn environment(environment: &str) -> (String, Texture) {
        let e = environment.to_lowercase();
        if e.contains("wsl") {
            ("WSL".into(), Texture::Hatch)
        } else if e.contains("windows") {
            ("WINDOWS".into(), Texture::Rules)
        } else if e.is_empty() {
            ("HOST".into(), Texture::Dots)
        } else {
            (environment.to_uppercase(), Texture::Dots)
        }
    }

    pub fn device_colour(device: &str) -> u32 {
        let h = device
            .to_lowercase()
            .bytes()
            .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        DEVICES[h as usize % DEVICES.len()]
    }
}

/// The colours of the harness marks that keep their own.
pub mod mark {
    /// Claude Code's critter, in Anthropic's clay.
    pub const CLAUDE: &str = "#D97757";
}
