//! What a page reaches the rest of the app through: its [`Host`], a cheap
//! handle on the [`Hub`]. A page never holds the shell; what it asks of
//! the shell arrives there as a [`HubEvent`](crate::hub::HubEvent), after
//! the page's own update is over.

use crate::ask::Ask;
use crate::chords::Run;
use crate::hub::{Hub, HubEvent, Surface};
use crate::link::client::LinkState;
use crate::link::http;
use crate::link::socket::Tap;
use crate::menu::{Menu, MenuRow};
use crate::settings::Place;
use gpui::{
    AnyView, App, Context, Entity, FocusHandle, Pixels, Point, SystemNotification, Task, WeakEntity,
};
use parvi_core::{Error, NodeId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

/// A page's handle on the hub. Cloning it is cheap.
#[derive(Clone)]
pub struct Host {
    page: &'static str,
    hub: WeakEntity<Hub>,
}

/// A node that answers now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub device: String,
    pub environment: String,
}

/// An HTTP request to a node, through the serving node.
#[derive(Clone, Debug)]
pub struct Request {
    /// The node it is for; empty for the serving node itself.
    pub node: NodeId,
    pub method: &'static str,
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// Its content type and bytes.
    pub body: Option<(String, Vec<u8>)>,
    /// How long the node may take to answer.
    pub within: Duration,
}

impl Request {
    pub fn new(method: &'static str, node: NodeId, path: impl Into<String>) -> Self {
        Self {
            node,
            method,
            path: path.into(),
            headers: vec![],
            body: None,
            within: http::ANSWER,
        }
    }

    pub fn get(node: NodeId, path: impl Into<String>) -> Self {
        Self::new("GET", node, path)
    }

    pub fn json(mut self, body: &serde_json::Value) -> Self {
        self.body = Some(("application/json".into(), body.to_string().into_bytes()));
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// For an action the node may take longer to answer.
    pub fn within(mut self, within: Duration) -> Self {
        self.within = within;
        self
    }
}

/// A node's answer: its status and body, whatever the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The node's `{error}` words for a failed answer, or its status.
    pub fn error_text(&self) -> String {
        http::error_text(self.status, &self.body)
    }
}

/// A line for the status line: what happened, whether it failed, and a
/// button that acts on it.
#[derive(Clone)]
pub struct Status {
    pub at: SystemTime,
    pub text: String,
    pub failure: bool,
    pub action: Option<(String, Run)>,
}

impl Status {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            at: SystemTime::now(),
            text: text.into(),
            failure: false,
            action: None,
        }
    }

    pub fn failure(text: impl Into<String>) -> Self {
        Self {
            failure: true,
            ..Self::new(text)
        }
    }

    pub fn action(
        mut self,
        label: impl Into<String>,
        run: impl Fn(&mut gpui::Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((label.into(), Rc::new(run)));
        self
    }
}

/// Where a layer draws: over the page, under the tab bar and above the
/// status line, or over the whole window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    Page,
    Window,
}

/// A page's layer on the one dismissal stack: what it draws, what has the
/// keyboard while it is on top, and what closing it does.
#[derive(Clone)]
pub struct Layer {
    pub name: String,
    pub view: AnyView,
    pub cover: Cover,
    pub focus: Option<FocusHandle>,
    pub dismiss: Option<Run>,
}

impl Layer {
    pub fn new(name: impl Into<String>, view: impl Into<AnyView>) -> Self {
        Self {
            name: name.into(),
            view: view.into(),
            cover: Cover::Page,
            focus: None,
            dismiss: None,
        }
    }

    pub fn cover(mut self, cover: Cover) -> Self {
        self.cover = cover;
        self
    }

    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    /// What closing it does, by Escape, a click outside or a close.
    pub fn on_dismiss(mut self, run: impl Fn(&mut gpui::Window, &mut App) + 'static) -> Self {
        self.dismiss = Some(Rc::new(run));
        self
    }
}

impl Host {
    /// The shell's own handle on `hub`; pages get theirs from [`crate::mount`].
    pub fn new(hub: &Entity<Hub>) -> Self {
        Self {
            page: "",
            hub: hub.downgrade(),
        }
    }

    pub fn page(&self) -> &'static str {
        self.page
    }

    /// The same hub, for page `page`.
    pub fn for_page(&self, page: &'static str) -> Host {
        Host {
            page,
            hub: self.hub.clone(),
        }
    }

    fn read<R>(&self, cx: &App, f: impl FnOnce(&Hub) -> R) -> Option<R> {
        self.hub.upgrade().map(|h| f(h.read(cx)))
    }

    fn update<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut Hub, &mut Context<Hub>) -> R,
    ) -> Option<R> {
        self.hub.update(cx, f).ok()
    }

    /// The nodes that answer now, serving node first.
    pub fn nodes(&self, cx: &App) -> Vec<Node> {
        self.read(cx, Hub::live_nodes).unwrap_or_default()
    }

    /// The serving node's address, where HTTP requests go.
    pub fn base(&self, cx: &App) -> String {
        self.read(cx, |h| h.fleet.base().to_string())
            .unwrap_or_default()
    }

    /// Sends `r` and hands back the node's answer, or why there was none:
    /// `unreachable` when it went nowhere, `unanswered` when it may have
    /// reached the node (a write's outcome is unknown, not failed).
    pub fn request(&self, r: Request, cx: &mut App) -> Task<Result<Answer, Error>> {
        let Some((base, here, rt)) = self.read(cx, |h| {
            (
                h.fleet.base().to_string(),
                h.fleet.here_id().map(str::to_string),
                h.fleet.here().runtime().clone(),
            )
        }) else {
            return Task::ready(Err(Error::new("closed", "The app is closing.")));
        };
        let path = if r.node.as_str().is_empty() || here.as_deref() == Some(r.node.as_str()) {
            r.path
        } else {
            let sep = if r.path.contains('?') { '&' } else { '?' };
            format!("{}{sep}node={}", r.path, r.node)
        };
        cx.background_executor().spawn(async move {
            let sent = rt.spawn(async move {
                let headers: Vec<(&str, &str)> = r
                    .headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let body = r.body.as_ref().map(|(t, b)| (t.as_str(), b.as_slice()));
                http::send(&base, r.method, &path, &headers, body, r.within).await
            });
            match sent.await {
                Ok(Ok((status, body))) => Ok(Answer { status, body }),
                Ok(Err(e)) if e.maybe_done => Err(Error::new("unanswered", e.message)),
                Ok(Err(e)) => Err(Error::new("unreachable", e.message)),
                Err(e) => Err(Error::new("unreachable", e.to_string())),
            }
        })
    }

    /// Sends a frame to one node. False when it is not reached now.
    pub fn send(&self, node: &NodeId, frame: Vec<u8>, cx: &App) -> bool {
        self.read(cx, |h| h.fleet.send(node.as_str(), frame))
            .unwrap_or(false)
    }

    /// The state of the link to `node`; `None` when the app follows no
    /// such node (it went from the fleet, or was never in it).
    pub fn link(&self, node: &NodeId, cx: &App) -> Option<LinkState> {
        self.read(cx, |h| h.link(node.as_str())).flatten()
    }

    /// The fleet as last seen, by node: each node's device and environment,
    /// also while it is away.
    pub fn places(&self, cx: &App) -> BTreeMap<String, Place> {
        self.read(cx, |h| h.settings.places.clone())
            .unwrap_or_default()
    }

    /// Hands every frame from every node to `tap` on the network thread
    /// first, from now on (see [`Tap`]).
    pub fn tap(&self, tap: Tap, cx: &App) {
        self.read(cx, |h| h.fleet.tap(tap));
    }

    /// Turns a channel on or off on every node, and on each node that comes
    /// into reach while it is on.
    pub fn channel(&self, kind: u8, on: bool, cx: &mut App) {
        self.update(cx, |h, _| h.fleet.channel(kind, on));
    }

    /// Says something on the status line.
    pub fn report(&self, status: Status, cx: &mut App) {
        self.update(cx, |h, cx| h.report(status, cx));
    }

    /// A moment's word in the status line.
    pub fn toast(&self, text: &str, cx: &mut App) {
        self.update(cx, |h, cx| h.toast(text, cx));
    }

    /// Opens a menu at `at`.
    pub fn menu(&self, rows: Vec<MenuRow>, at: Point<Pixels>, cx: &mut App) {
        self.open_menu(rows, at, None, cx);
    }

    /// Opens a menu at `at` with its danger row `armed` already: a key
    /// that asked for the danger opens it one confirmation away.
    pub fn menu_armed(&self, rows: Vec<MenuRow>, at: Point<Pixels>, armed: usize, cx: &mut App) {
        self.open_menu(rows, at, Some(armed), cx);
    }

    fn open_menu(&self, rows: Vec<MenuRow>, at: Point<Pixels>, armed: Option<usize>, cx: &mut App) {
        let menu = Menu { rows, at, armed };
        self.update(cx, |h, cx| h.open(Surface::Menu(menu), cx));
    }

    /// Opens this page's layer `layer.name` over the others; one of that
    /// name already open moves to the top.
    pub fn layer(&self, mut layer: Layer, cx: &mut App) {
        layer.name = self.layer_name(&layer.name);
        self.update(cx, |h, cx| h.open(Surface::Layer(layer), cx));
    }

    /// Closes this page's layer `name` and whatever opened above it.
    pub fn close_layer(&self, name: &str, cx: &mut App) {
        let kind = self.layer_name(name);
        self.update(cx, |h, cx| h.close(&kind, cx));
    }

    pub fn is_open(&self, name: &str, cx: &App) -> bool {
        let kind = self.layer_name(name);
        self.read(cx, |h| h.stack.is_open(&kind)).unwrap_or(false)
    }

    fn layer_name(&self, name: &str) -> String {
        format!("{}/{name}", self.page)
    }

    /// This page's settings: their defaults while it has none.
    pub fn setting<T: DeserializeOwned + Default>(&self, cx: &App) -> Result<T, Error> {
        self.read(cx, |h| h.settings.section(self.page))
            .unwrap_or_else(|| Ok(T::default()))
    }

    /// Keeps this page's settings.
    pub fn set_setting<T: Serialize>(&self, value: &T, cx: &mut App) -> Result<(), Error> {
        let page = self.page;
        self.update(cx, |h, cx| {
            h.settings.set_section(page, value)?;
            h.save(cx);
            Ok(())
        })
        .unwrap_or(Ok(()))
    }

    /// Hands `ask` to the page that answers it. When none does, the status
    /// line says so and so does the error.
    pub fn ask(&self, ask: Ask, cx: &mut App) -> Result<(), Error> {
        self.update(cx, |h, cx| h.ask(ask, cx))
            .unwrap_or_else(|| Err(Error::new("closed", "The app is closing.")))
    }

    /// Dials every link again now, instead of waiting out its backoff.
    pub fn redial(&self, cx: &mut App) {
        self.update(cx, |h, cx| h.redial(cx));
    }

    /// Posts a desktop notification; a click on it runs `click` in the
    /// window it came from. A later one with the same tag replaces it.
    pub fn notify(
        &self,
        notification: SystemNotification,
        click: impl Fn(&mut gpui::Window, &mut App) + 'static,
        cx: &mut App,
    ) {
        self.update(cx, |h, cx| h.notify(notification, Rc::new(click), cx));
    }

    /// Shows this page.
    pub fn show(&self, cx: &mut App) {
        let page = self.page;
        self.update(cx, |_, cx| cx.emit(HubEvent::Show(page)));
    }
}
