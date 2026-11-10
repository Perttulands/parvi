//! What opens over the page, each a layer on the app's one stack: the app
//! draws it, closes it on Escape or a click outside, and gives it the
//! keyboard; the page keeps its state and draws its content.

use super::page::Terminals;
use crate::store::sessions::Key;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, Pixels, Point, Render, Subscription,
    WeakEntity, Window, div,
};
use parvi_app_kit::host::Cover;

pub const SEND: &str = "send";
pub const OVERVIEW: &str = "overview";
pub const FIND: &str = "find";
pub const PEEK: &str = "peek";
pub const RENAME_TAB: &str = "rename-tab";
pub const RENAME_SESSION: &str = "rename-session";
pub const PRESET_NAME: &str = "preset-name";

/// A surface over the page, and what it is about.
#[derive(Clone, Debug)]
pub enum Surface {
    Send,
    Overview,
    Find,
    Peek,
    /// Its field is drawn in the tab itself.
    RenameTab(usize),
    RenameSession(Key),
    /// Naming a preset, where the menu was.
    PresetName(Point<Pixels>),
}

impl Surface {
    pub fn name(&self) -> &'static str {
        match self {
            Surface::Send => SEND,
            Surface::Overview => OVERVIEW,
            Surface::Find => FIND,
            Surface::Peek => PEEK,
            Surface::RenameTab(_) => RENAME_TAB,
            Surface::RenameSession(_) => RENAME_SESSION,
            Surface::PresetName(_) => PRESET_NAME,
        }
    }

    /// The drawers and Peek cover the page; the small dialogs the window.
    pub fn cover(&self) -> Cover {
        match self {
            Surface::RenameSession(_) | Surface::PresetName(_) => Cover::Window,
            _ => Cover::Page,
        }
    }
}

/// The layer's view: it draws the page's surface `name`, again whenever the
/// page changes.
pub struct View {
    page: WeakEntity<Terminals>,
    name: &'static str,
    _page: Option<Subscription>,
}

pub fn view(page: WeakEntity<Terminals>, name: &'static str, cx: &mut App) -> Entity<View> {
    cx.new(|cx| {
        let watch = page
            .upgrade()
            .map(|p| cx.observe(&p, |_, _, cx| cx.notify()));
        View {
            page,
            name,
            _page: watch,
        }
    })
}

impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.name;
        self.page
            .update(cx, |p, cx| p.render_surface(name, window, cx))
            .ok()
            .flatten()
            .unwrap_or_else(|| div().into_any_element())
    }
}

impl Terminals {
    /// The content of surface `name`, while it is open.
    fn render_surface(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let s = self.surfaces.iter().find(|s| s.name() == name)?.clone();
        match s {
            Surface::Send => Some(self.render_send(window, cx)),
            Surface::Overview => Some(self.render_overview(window, cx)),
            Surface::Find => Some(self.render_find(window, cx)),
            Surface::Peek => self.render_peek(window, cx),
            Surface::RenameTab(_) => None,
            Surface::RenameSession(k) => Some(self.render_rename(
                RENAME_SESSION,
                format!("Rename session {}", k.name),
                "Enter renames · Esc keeps the name",
                cx,
            )),
            Surface::PresetName(at) => Some(self.render_preset_name(at, cx)),
        }
    }
}
