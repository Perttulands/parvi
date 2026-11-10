//! A page: one tab of the app, one entity. The shell knows pages only
//! through this trait and the [`Mounted`] page each [`Build`] makes.

use crate::ask::{Answers, Ask};
use crate::chords::Chord;
use crate::declared::Setting;
use crate::host::Host;
use crate::link::client::LinkState;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EntityId, FocusHandle, IntoElement, Render,
    Subscription, Window,
};
use parvi_core::NodeId;
use parvi_core::frame::Kinds;

pub trait Page: Render + Sized + 'static {
    /// Names the page in view control, in settings and in its layers.
    const ID: &'static str;
    /// Its tab.
    const TITLE: &'static str;
    /// The node area it needs: the tab is hidden while every node reached
    /// has that area off.
    const AREA: Option<&'static str> = None;
    /// The frame kinds that are this page's; [`Page::frame`] gets them.
    const KINDS: Kinds = &[];
    /// The channels turned on while the page shows, and off when it hides.
    const CHANNELS: &'static [u8] = &[];
    /// The asks this page answers; [`Page::answer`] gets them.
    const ANSWERS: &'static [Answers] = &[];
    /// The settings the shell's Settings page lists for it, kept in its
    /// section of `app.json`.
    const SETTINGS: &'static [Setting] = &[];

    /// A frame of one of its kinds, from `node`.
    fn frame(
        &mut self,
        _node: &NodeId,
        _kind: u8,
        _body: &[u8],
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// The link to `node` changed state; `None` when the app no longer
    /// follows that node.
    fn link(
        &mut self,
        _node: &NodeId,
        _state: Option<LinkState>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Its own tabs on the tab bar, in its place there, when one tab is not
    /// enough; `shown` says whether the page is the one shown. A click on
    /// them shows the page through its Host.
    fn tabs(
        &mut self,
        _shown: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }

    /// The page came forward (`true`) or went behind another.
    fn shown(&mut self, _shown: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// An ask of a kind it answers, from page `from` (`""`: the shell). A
    /// beside ask ([`Ask::beside`]) is answered with
    /// [`Host::show_beside`] and leaves `from` shown.
    fn answer(
        &mut self,
        _ask: Ask,
        _from: &'static str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// One of its [`Page::SETTINGS`] was changed in its section: it reads
    /// its section again.
    fn settings_changed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Its chords, live while it shows; those marked
    /// [`Chord::everywhere`] are live while any page shows.
    fn chords(&self, _cx: &App) -> Vec<Chord> {
        vec![]
    }

    /// What has the keyboard while it shows and nothing is over it.
    fn focus(&self, _cx: &App) -> Option<FocusHandle> {
        None
    }
}

/// Makes a page, given its Host. The app lists these in its `PAGES`.
pub type Build = fn(Host, &mut Window, &mut App) -> Mounted;

/// What the shell knows of a page without knowing its type.
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub id: &'static str,
    pub title: &'static str,
    pub area: Option<&'static str>,
    pub kinds: Kinds,
    pub channels: &'static [u8],
    pub answers: &'static [Answers],
    pub settings: &'static [Setting],
}

impl Meta {
    /// Whether frames of `kind` are this page's.
    pub fn takes(&self, kind: u8) -> bool {
        parvi_core::frame::owns(self.kinds, kind)
    }
}

/// A page the shell holds: what it is, and its entity.
pub struct Mounted {
    pub meta: Meta,
    slot: Box<dyn Slot>,
}

/// Makes page `P` with `make`, its Host addressed to it. A page's [`Build`]
/// is `|host, window, cx| mount(host, window, cx, MyPage::new)`.
pub fn mount<P: Page>(
    host: Host,
    window: &mut Window,
    cx: &mut App,
    make: impl FnOnce(Host, &mut Window, &mut Context<P>) -> P,
) -> Mounted {
    let host = host.for_page(P::ID, P::KINDS);
    let entity = cx.new(|cx| make(host, window, cx));
    Mounted {
        meta: Meta {
            id: P::ID,
            title: P::TITLE,
            area: P::AREA,
            kinds: P::KINDS,
            channels: P::CHANNELS,
            answers: P::ANSWERS,
            settings: P::SETTINGS,
        },
        slot: Box::new(entity),
    }
}

impl Mounted {
    pub fn entity_id(&self) -> EntityId {
        self.slot.entity_id()
    }
    /// Draws the page now (inside the shell's error boundary).
    pub fn draw(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.slot.draw(window, cx)
    }
    pub fn frame(&self, node: &NodeId, kind: u8, body: &[u8], window: &mut Window, cx: &mut App) {
        self.slot.frame(node, kind, body, window, cx)
    }
    pub fn link(&self, node: &NodeId, state: Option<LinkState>, window: &mut Window, cx: &mut App) {
        self.slot.link(node, state, window, cx)
    }
    pub fn tabs(&self, shown: bool, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.slot.tabs(shown, window, cx)
    }
    pub fn shown(&self, shown: bool, window: &mut Window, cx: &mut App) {
        self.slot.shown(shown, window, cx)
    }
    pub fn answer(&self, ask: Ask, from: &'static str, window: &mut Window, cx: &mut App) {
        self.slot.answer(ask, from, window, cx)
    }
    pub fn settings_changed(&self, window: &mut Window, cx: &mut App) {
        self.slot.settings_changed(window, cx)
    }
    pub fn chords(&self, cx: &App) -> Vec<Chord> {
        self.slot.chords(cx)
    }
    pub fn focus(&self, cx: &App) -> Option<FocusHandle> {
        self.slot.focus(cx)
    }
    /// Redraws `viewer` (the shell) whenever the page notifies.
    pub fn redraws(&self, viewer: EntityId, cx: &mut App) -> Subscription {
        self.slot.redraws(viewer, cx)
    }
}

trait Slot {
    fn entity_id(&self) -> EntityId;
    fn draw(&self, window: &mut Window, cx: &mut App) -> AnyElement;
    fn frame(&self, node: &NodeId, kind: u8, body: &[u8], window: &mut Window, cx: &mut App);
    fn link(&self, node: &NodeId, state: Option<LinkState>, window: &mut Window, cx: &mut App);
    fn tabs(&self, shown: bool, window: &mut Window, cx: &mut App) -> Option<AnyElement>;
    fn shown(&self, shown: bool, window: &mut Window, cx: &mut App);
    fn answer(&self, ask: Ask, from: &'static str, window: &mut Window, cx: &mut App);
    fn settings_changed(&self, window: &mut Window, cx: &mut App);
    fn chords(&self, cx: &App) -> Vec<Chord>;
    fn focus(&self, cx: &App) -> Option<FocusHandle>;
    fn redraws(&self, viewer: EntityId, cx: &mut App) -> Subscription;
}

impl<P: Page> Slot for Entity<P> {
    fn entity_id(&self) -> EntityId {
        Entity::entity_id(self)
    }
    fn draw(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.update(cx, |p, cx| p.render(window, cx).into_any_element())
    }
    fn frame(&self, node: &NodeId, kind: u8, body: &[u8], window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.frame(node, kind, body, window, cx))
    }
    fn link(&self, node: &NodeId, state: Option<LinkState>, window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.link(node, state, window, cx))
    }
    fn tabs(&self, shown: bool, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.update(cx, |p, cx| p.tabs(shown, window, cx))
    }
    fn shown(&self, shown: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.shown(shown, window, cx))
    }
    fn answer(&self, ask: Ask, from: &'static str, window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.answer(ask, from, window, cx))
    }
    fn settings_changed(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.settings_changed(window, cx))
    }
    fn chords(&self, cx: &App) -> Vec<Chord> {
        self.read(cx).chords(cx)
    }
    fn focus(&self, cx: &App) -> Option<FocusHandle> {
        self.read(cx).focus(cx)
    }
    fn redraws(&self, viewer: EntityId, cx: &mut App) -> Subscription {
        cx.observe(self, move |_, cx| cx.notify(viewer))
    }
}
