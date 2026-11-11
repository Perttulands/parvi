//! The state the shell and every page share: the link to the nodes, the
//! settings, the status line and toast, and the one stack of layers. Pages
//! reach it through their [`Host`](crate::Host); the shell holds it and
//! hears its [`HubEvent`]s. The hub never calls a page, so a page may use
//! its Host from inside its own update.

use crate::ask::{Answers, Ask};
use crate::chords::Run;
use crate::clock::hhmm;
use crate::host::{Layer, Node, Status};
use crate::layers::{self, Stack};
use crate::link::client::{LinkState, Notice};
use crate::link::devices;
use crate::link::fleet::Fleet;
use crate::link::socket::{Event, Net};
use crate::menu::Menu;
use crate::settings::Settings;
use crate::theme::motion;
use gpui::{
    AnyEntity, AnyView, AnyWeakEntity, App, AppContext, Context, Entity, EventEmitter,
    SystemNotification, Task,
};
use parvi_core::{Error, NodeId};
use std::any::{TypeId, type_name};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

pub struct Toast {
    pub text: String,
    pub at: Instant,
}

/// What is on the dismissal stack: a menu, or a layer.
#[derive(Clone)]
pub enum Surface {
    Menu(Menu),
    Layer(Layer),
}

/// The stack's name for the one open menu.
const MENU: &str = "menu";

impl layers::Layer for Surface {
    fn kind(&self) -> &str {
        match self {
            Surface::Menu(_) => MENU,
            Surface::Layer(l) => &l.name,
        }
    }
    fn passing(&self) -> bool {
        matches!(self, Surface::Menu(_))
    }
}

/// What the shell hears from the hub, after the update that caused it.
pub enum HubEvent {
    /// A frame for the page whose kinds hold it.
    Frame {
        node: NodeId,
        kind: u8,
        body: Vec<u8>,
    },
    /// Page `to` answers this, which page `from` asked (`""`: the shell).
    Ask {
        to: &'static str,
        from: &'static str,
        ask: Ask,
    },
    /// Show this page.
    Show(&'static str),
    /// The stack changed: the keyboard goes to the top layer, or back to
    /// the page, and what closed runs its dismiss.
    Layers { closed: Vec<Surface> },
    /// A setting page `page` declares was changed for it.
    SettingChanged(&'static str),
    /// Page `page` opened or closed its dock.
    Dock(&'static str),
    /// The link to `node` changed state; `None` when the app no longer
    /// follows that node. Every page hears it.
    Link {
        node: NodeId,
        state: Option<LinkState>,
    },
}

pub struct Hub {
    pub fleet: Fleet,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    save_task: Option<Task<()>>,
    pub status: Option<Status>,
    pub toast: Option<Toast>,
    toast_task: Option<Task<()>>,
    pub stack: Stack<Surface>,
    /// The asks each page answers, by page id.
    answers: Vec<(&'static str, &'static [Answers])>,
    /// Nodes whose link is open, as last tracked.
    heard: HashSet<String>,
    /// Devices said to be away.
    away: HashSet<String>,
    /// What a click on each desktop notification posted does, by its tag.
    clicks: HashMap<String, Run>,
    /// The read models pages share, by type.
    models: HashMap<TypeId, Model>,
    /// What an answer opened beside each page, by the page.
    beside: HashMap<&'static str, AnyView>,
    /// The docks open beside every page, by the page that supplies each,
    /// in the order they opened.
    docks: Vec<(&'static str, AnyView)>,
    /// The views that may hold unsaved work, each with the page it is from.
    guards: Vec<Guard>,
}

/// What a guarded view holds unsaved now: `None` when all is saved or the
/// view is gone.
pub type Unsaved = Rc<dyn Fn(&App) -> Option<String>>;

/// A view that may hold unsaved work: while it lives, closing the window
/// asks it first.
struct Guard {
    page: &'static str,
    view: AnyWeakEntity,
    unsaved: Unsaved,
}

/// A read model: its entity, and the page that publishes it once one has.
struct Model {
    entity: AnyEntity,
    publisher: Option<&'static str>,
}

impl EventEmitter<HubEvent> for Hub {}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Hub {
    /// The hub with the settings at `settings_path` (none: nothing is
    /// kept), not yet dialling. Link events arrive on `events`.
    pub fn new(
        net: Net,
        events: async_channel::Sender<Event>,
        settings_path: Option<PathBuf>,
    ) -> Self {
        let (settings, notice) = match &settings_path {
            Some(p) => Settings::load(p),
            None => (
                Settings::default(),
                Some("No config folder; settings will not be kept.".into()),
            ),
        };
        Self {
            fleet: Fleet::new(net, events),
            settings,
            settings_path,
            save_task: None,
            status: notice.map(Status::failure),
            toast: None,
            toast_task: None,
            stack: Stack::default(),
            answers: vec![],
            heard: HashSet::new(),
            away: HashSet::new(),
            clicks: HashMap::new(),
            models: HashMap::new(),
            beside: HashMap::new(),
            docks: vec![],
            guards: vec![],
        }
    }

    /// What each page answers, as the shell mounted them.
    pub fn set_answers(&mut self, answers: Vec<(&'static str, &'static [Answers])>) {
        self.answers = answers;
    }

    /// Dials the configured node.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        let node = self.settings.node.clone();
        self.fleet.connect(&node);
        cx.notify();
    }

    /// Dials every link again now.
    pub fn redial(&mut self, cx: &mut Context<Self>) {
        self.fleet.redial();
        cx.notify();
    }

    /// Posts a desktop notification, keeping what a click on it does.
    pub fn notify(&mut self, n: SystemNotification, click: Run, cx: &mut Context<Self>) {
        self.clicks.insert(n.tag.to_string(), click);
        cx.show_system_notification(n);
    }

    /// What a click on the notification tagged `tag` does, once.
    pub fn clicked(&mut self, tag: &str) -> Option<Run> {
        self.clicks.remove(tag)
    }

    /// A status action that dials again.
    fn reconnect(cx: &mut Context<Self>) -> (String, Run) {
        let hub = cx.entity().downgrade();
        let run: Run = Rc::new(move |_, cx| {
            let _ = hub.update(cx, |h, cx| h.redial(cx));
        });
        ("Reconnect".into(), run)
    }

    /// A link event: the fleet takes it; frames go to the pages, and what
    /// the link did is told in the status line.
    pub fn handle(&mut self, ev: Event, cx: &mut Context<Self>) {
        let was = self.fleet.here().state;
        let links = self.links();
        for (node, n) in self.fleet.handle(ev, &self.settings.places) {
            self.notice(node, n, cx);
        }
        self.links_changed(links, cx);
        self.remember_fleet(cx);
        self.track(cx);
        let here = self.fleet.here();
        if was != here.state && here.state == LinkState::Refused {
            let why = here.reason.clone().unwrap_or_default();
            let (label, run) = Self::reconnect(cx);
            self.status = Some(Status {
                action: Some((label, run)),
                ..Status::failure(why)
            });
        }
        cx.notify();
    }

    fn notice(&mut self, node: String, n: Notice, cx: &mut Context<Self>) {
        match n {
            Notice::Frame { kind, body } => cx.emit(HubEvent::Frame {
                node: NodeId(node),
                kind,
                body,
            }),
            Notice::Nodes(_) | Notice::Serving(None) => {}
            Notice::Serving(Some(through)) => {
                let name = self
                    .settings
                    .places
                    .get(&through)
                    .map(|p| format!("{} ({})", p.device, p.environment))
                    .unwrap_or(through);
                let text = format!("{} is away; trying {name}", self.home_name());
                self.status = Some(Status::failure(text));
            }
            Notice::Opened if Some(node.as_str()) != self.fleet.here_id() => {}
            Notice::Opened => {
                if let Some(h) = self.fleet.here().hello.clone() {
                    let to = format!("{} ({}), Parvi {}", h.device, h.environment, h.version);
                    let text = match self.fleet.away() {
                        Some(_) => {
                            format!("Connected through {to} while {} is away", self.home_name())
                        }
                        None => format!("Connected to {to}"),
                    };
                    self.status = Some(Status::new(text));
                }
            }
        }
    }

    /// Each followed node's link state, by node.
    fn links(&self) -> BTreeMap<String, LinkState> {
        self.fleet
            .clients()
            .iter()
            .map(|c| (c.node(), c.state))
            .filter(|(n, _)| !n.is_empty())
            .collect()
    }

    /// Tells the pages of each link that changed since `before`.
    fn links_changed(&mut self, before: BTreeMap<String, LinkState>, cx: &mut Context<Self>) {
        let now = self.links();
        for (node, state) in &now {
            if before.get(node) != Some(state) {
                cx.emit(HubEvent::Link {
                    node: NodeId(node.clone()),
                    state: Some(*state),
                });
            }
        }
        for node in before.keys().filter(|n| !now.contains_key(*n)) {
            cx.emit(HubEvent::Link {
                node: NodeId(node.clone()),
                state: None,
            });
        }
    }

    /// The state of the link to `node`; `None` when the app follows no such node.
    pub fn link(&self, node: &str) -> Option<LinkState> {
        self.fleet.get(node).map(|c| c.state)
    }

    /// The configured node by its device, or by its address before it has
    /// said hello.
    fn home_name(&self) -> String {
        self.fleet
            .home_device()
            .map(str::to_string)
            .unwrap_or_else(|| self.settings.node.clone())
    }

    /// Keeps `places` the last known fleet: each node's device, environment
    /// and address. Nodes the configured node no longer lists go.
    fn remember_fleet(&mut self, cx: &mut Context<Self>) {
        let places = &mut self.settings.places;
        let before = places.clone();
        for c in self.fleet.clients() {
            let node = c.node();
            if node.is_empty() {
                continue;
            }
            let p = places.entry(node).or_default();
            if let Some(h) = &c.hello {
                p.device = h.device.clone();
                p.environment = h.environment.clone();
            }
        }
        for peer in &self.fleet.peers {
            let p = places.entry(peer.node.clone()).or_default();
            p.device = peer.device.clone();
            p.environment = peer.environment.clone();
            if !peer.address.is_empty() {
                p.address = peer.address.clone();
            }
        }
        // A node serving while the configured one is away may list a cold
        // fleet (it has not probed since nobody asked), the configured node
        // missing: forget nothing until the configured node serves again.
        if self.fleet.listed
            && self.fleet.away().is_none()
            && let Some(here) = self.fleet.here_id()
        {
            let peers = &self.fleet.peers;
            places.retain(|n, _| n == here || peers.iter().any(|p| p.node == *n));
        }
        if *places != before {
            self.save(cx);
        }
    }

    /// Stamps a node that stopped answering with when it was last heard,
    /// so an away device says since when, also after a restart; says in
    /// the status line when a device goes and comes back.
    fn track(&mut self, cx: &mut Context<Self>) {
        let before = self.heard.clone();
        let now = now_ms();
        let mut stamped = false;
        // A link still dialling for the first time (a new serving node's
        // peers) has not lost its node: only one that went says it left.
        let links: Vec<(String, LinkState)> = self
            .fleet
            .clients()
            .iter()
            .map(|c| (c.node(), c.state))
            .filter(|(n, s)| !n.is_empty() && *s != LinkState::Connecting)
            .collect();
        for (node, state) in links {
            if state == LinkState::Open {
                self.heard.insert(node);
            } else if self.heard.remove(&node)
                && let Some(p) = self.settings.places.get_mut(&node)
            {
                p.seen = Some(now);
                stamped = true;
            }
        }
        if stamped {
            self.save(cx);
        }
        let (gone, came) = devices::changed(&self.settings.places, &before, &self.heard);
        let serving = self.fleet.here().hello.as_ref().map(|h| h.device.clone());
        for device in gone {
            let line = devices::away_line(&device, Some(&hhmm(now)));
            let mut status = Status::new(line);
            if serving.as_deref() == Some(device.as_str()) {
                status.action = Some(Self::reconnect(cx));
            }
            self.status = Some(status);
            self.away.insert(device);
        }
        for device in came {
            if self.away.remove(&device) && serving.as_deref() != Some(device.as_str()) {
                self.status = Some(Status::new(format!("{device} is back")));
            }
        }
    }

    pub fn report(&mut self, status: Status, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
    }

    pub fn toast(&mut self, text: &str, cx: &mut Context<Self>) {
        let at = Instant::now();
        self.toast = Some(Toast {
            text: text.to_string(),
            at,
        });
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            // Two wakes: one to start fading, one to go.
            cx.background_executor()
                .timer(Duration::from_millis(
                    motion::TOAST_IN + motion::TOAST_SHOWN,
                ))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
            cx.background_executor()
                .timer(Duration::from_millis(motion::TOAST_OUT))
                .await;
            let _ = this.update(cx, |h, cx| {
                if h.toast.as_ref().is_some_and(|t| t.at == at) {
                    h.toast = None;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Writes the settings soon: a burst of changes is one write.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let Ok(snapshot) = this.read_with(cx, |h, _| h.settings.clone()) else {
                return;
            };
            let saved = cx
                .background_executor()
                .spawn(async move { snapshot.save(&path) })
                .await;
            if let Err(e) = saved {
                let _ = this.update(cx, |h, cx| {
                    h.report(
                        Status::failure(format!("Settings could not be saved: {e}")),
                        cx,
                    );
                });
            }
        }));
    }

    /// Opens a surface over the others.
    pub fn open(&mut self, s: Surface, cx: &mut Context<Self>) {
        let closed: Vec<Surface> = self
            .stack
            .iter()
            .filter(|x| layers::Layer::passing(*x))
            .cloned()
            .collect();
        self.stack.open(s);
        cx.emit(HubEvent::Layers { closed });
        cx.notify();
    }

    /// Closes the surface of this kind and whatever opened above it.
    pub fn close(&mut self, kind: &str, cx: &mut Context<Self>) {
        let closed = self.stack.close(kind);
        if !closed.is_empty() {
            cx.emit(HubEvent::Layers { closed });
            cx.notify();
        }
    }

    /// Closes the open menu, if one is.
    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.close(MENU, cx)
    }

    /// Escape: the top surface closes. False when nothing was open.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(top) = self.stack.dismiss() else {
            return false;
        };
        cx.emit(HubEvent::Layers { closed: vec![top] });
        cx.notify();
        true
    }

    /// The read model `T`, made empty by whichever page reaches it first.
    pub fn model<T: Default + 'static>(&mut self, cx: &mut Context<Self>) -> Entity<T> {
        self.models
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Model {
                entity: cx.new(|_| T::default()).into_any(),
                publisher: None,
            })
            .entity
            .clone()
            .downcast::<T>()
            .expect("a model is kept by its own type")
    }

    /// The read model `T` for page `page` to publish; refused when another
    /// page publishes it.
    pub fn publisher<T: Default + 'static>(
        &mut self,
        page: &'static str,
        cx: &mut Context<Self>,
    ) -> Result<Entity<T>, Error> {
        let entity = self.model::<T>(cx);
        let model = self
            .models
            .get_mut(&TypeId::of::<T>())
            .expect("made just now");
        match model.publisher {
            Some(other) if other != page => Err(Error::new(
                "published_twice",
                format!("{other} and {page} both publish {}", type_name::<T>()),
            )),
            _ => {
                model.publisher = Some(page);
                Ok(entity)
            }
        }
    }

    /// Keeps `value` for the setting `key` page `page` declares, and tells
    /// the page.
    pub fn set_value(
        &mut self,
        page: &'static str,
        key: &str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_value(page, key, value);
        self.save(cx);
        cx.emit(HubEvent::SettingChanged(page));
        cx.notify();
    }

    /// What is open beside `page`.
    pub fn beside(&self, page: &str) -> Option<AnyView> {
        self.beside.get(page).cloned()
    }

    /// Opens `view` beside `page`, or closes what is there.
    pub fn set_beside(
        &mut self,
        page: &'static str,
        view: Option<AnyView>,
        cx: &mut Context<Self>,
    ) {
        match view {
            Some(view) => self.beside.insert(page, view),
            None => self.beside.remove(page),
        };
        cx.notify();
    }

    /// The docks open, in the order they opened, by the page each is from.
    pub fn docks(&self) -> Vec<(&'static str, AnyView)> {
        self.docks.clone()
    }

    pub fn docked(&self, page: &str) -> bool {
        self.docks.iter().any(|(p, _)| *p == page)
    }

    /// Opens `view` as `page`'s dock, in place of one it had, or closes it.
    pub fn set_dock(&mut self, page: &'static str, view: Option<AnyView>, cx: &mut Context<Self>) {
        let at = self.docks.iter().position(|(p, _)| *p == page);
        match (at, view) {
            (Some(i), Some(view)) => self.docks[i].1 = view,
            (None, Some(view)) => {
                self.docks.push((page, view));
                cx.emit(HubEvent::Dock(page));
            }
            (Some(i), None) => {
                self.docks.remove(i);
                cx.emit(HubEvent::Dock(page));
            }
            (None, None) => return,
        }
        cx.notify();
    }

    /// Asks `unsaved` about `view`, of page `page`, whenever the window
    /// would close, until `view` is gone.
    pub fn guard_close(&mut self, page: &'static str, view: AnyWeakEntity, unsaved: Unsaved) {
        self.guards.retain(|g| g.view.is_upgradable());
        self.guards.push(Guard {
            page,
            view,
            unsaved,
        });
    }

    /// What every live view holds unsaved now, by the page it is from, in
    /// the order they were guarded.
    pub fn unsaved(&self, cx: &App) -> Vec<(&'static str, String)> {
        self.guards
            .iter()
            .filter_map(|g| (g.unsaved)(cx).map(|label| (g.page, label)))
            .collect()
    }

    /// Hands `ask`, from page `from`, to the page that answers it. When
    /// none does, the status line says so and so does the error.
    pub fn ask(
        &mut self,
        from: &'static str,
        ask: Ask,
        cx: &mut Context<Self>,
    ) -> Result<(), Error> {
        let to = self
            .answers
            .iter()
            .find(|(_, a)| a.iter().any(|a| ask.answered_by(a)))
            .map(|(id, _)| *id);
        match to {
            Some(to) => {
                cx.emit(HubEvent::Ask { to, from, ask });
                Ok(())
            }
            None => {
                let e = Error::new(
                    "unanswered",
                    format!("Nothing here can {}.", ask.describe()),
                );
                self.report(Status::failure(e.message.clone()), cx);
                Err(e)
            }
        }
    }

    /// The nodes that answer now, serving node first.
    pub fn live_nodes(&self) -> Vec<Node> {
        self.fleet
            .clients()
            .iter()
            .filter(|c| c.state == LinkState::Open)
            .filter_map(|c| {
                let h = c.hello.as_ref()?;
                Some(Node {
                    id: NodeId(c.node()),
                    device: h.device.clone(),
                    environment: h.environment.clone(),
                })
            })
            .collect()
    }

    /// The areas every node reached has turned off; none before any hello.
    pub fn areas_off(&self) -> Vec<String> {
        let hellos: Vec<_> = self
            .fleet
            .clients()
            .iter()
            .filter_map(|c| c.hello.as_ref())
            .collect();
        let Some(first) = hellos.first() else {
            return vec![];
        };
        first
            .off
            .iter()
            .filter(|a| hellos.iter().all(|h| h.off.contains(a)))
            .cloned()
            .collect()
    }
}
