//! The Settings page: the shell's own settings (the node, the shortcuts),
//! then the settings each registered page declares ([`Page::SETTINGS`]),
//! one group each in the list, and the connected node's configuration.
//! Settings is not an area: what is listed comes from the pages, and a
//! change goes into the declaring page's section, which then hears
//! [`Page::settings_changed`]. The node's configuration (`/api/config`) is
//! read each time the page comes forward or the node's link opens while it
//! shows, and shown as named settings; it changes by editing its file.
//! Typing in the search shows every setting that matches, from every group.

use gpui::{
    AnyElement, App, AppContext, Context, Entity, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use parvi_app_kit::ask::Ask;
use parvi_app_kit::host::{Request, Status};
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::page::Meta;
use parvi_app_kit::settings::DEFAULT_NODE;
use parvi_app_kit::theme::{Theme, size, theme};
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pane;
use parvi_app_kit::{Control, Host, Hub, Page, Setting};
use parvi_core::NodeId;
use serde_json::Value;
use std::collections::HashMap;

/// The page's id, which no other page may take.
pub const ID: &str = "settings";

/// The shell's own settings, kept outside any page's section.
const APP: &[Setting] = &[
    Setting::text("node", "Node", DEFAULT_NODE)
        .note("The node this app talks to, as http://host:port. Enter connects."),
    Setting::toggle("keys", "Shortcuts", true)
        .note("Parvi's shortcuts take their keys before the terminal does."),
    Setting::toggle("show_keys", "Show shortcut keys", false)
        .note("Each shortcut's keys show at the foot of the window for a moment."),
];

/// Whose a setting is: the shell's, or a page's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    App,
    Page(&'static str),
}

impl Owner {
    fn name(self) -> &'static str {
        match self {
            Owner::App => "app",
            Owner::Page(id) => id,
        }
    }
}

/// A group in the list: the shell's own, a page's, or the node's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    App,
    Page(&'static str),
    Node,
}

pub struct SettingsPage {
    hub: Entity<Hub>,
    host: Host,
    shown: bool,
    chosen: Group,
    search: Entity<TextField>,
    /// The connected node's config as it last answered, or why it did not.
    node_config: Option<Result<Value, String>>,
    /// The pages that declare settings, in the app's order.
    pages: Vec<Meta>,
    /// A field for each text setting.
    fields: HashMap<(Owner, &'static str), Entity<TextField>>,
    _fields: Vec<Subscription>,
}

impl Page for SettingsPage {
    const ID: &'static str = ID;
    const TITLE: &'static str = "Settings";
    const KEY: Option<char> = Some(',');

    /// The fields show what is kept now, and the node's config is read.
    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.refresh(cx);
            self.read_node_config(cx);
        }
    }

    fn link(
        &mut self,
        _: &NodeId,
        state: Option<LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shown && state == Some(LinkState::Open) {
            self.read_node_config(cx);
        }
    }

    /// The settings were read again from the file.
    fn settings_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh(cx);
        cx.notify();
    }
}

impl SettingsPage {
    pub fn new(
        hub: Entity<Hub>,
        host: Host,
        pages: Vec<Meta>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pages: Vec<Meta> = pages
            .into_iter()
            .filter(|p| !p.settings.is_empty())
            .collect();
        let mut fields = HashMap::new();
        let mut subscriptions = vec![];
        let declared = APP.iter().map(|s| (Owner::App, s)).chain(
            pages
                .iter()
                .flat_map(|p| p.settings.iter().map(|s| (Owner::Page(p.id), s))),
        );
        for (owner, setting) in declared {
            let Control::Text { default } = setting.control else {
                continue;
            };
            let field = cx.new(|cx| TextField::new(cx, default));
            let key = setting.key;
            subscriptions.push(cx.subscribe_in(
                &field,
                window,
                move |page: &mut Self, field, ev: &FieldEvent, _, cx| {
                    if let FieldEvent::Submit = ev {
                        let text = field.read(cx).text.trim().to_string();
                        page.write(owner, key, Value::from(text), cx);
                    }
                },
            ));
            fields.insert((owner, key), field);
        }
        let search = cx.new(|cx| TextField::new(cx, "Search settings"));
        subscriptions.push(cx.subscribe(&search, |_, _, ev: &FieldEvent, cx| {
            if let FieldEvent::Changed = ev {
                cx.notify();
            }
        }));
        Self {
            hub,
            host,
            shown: false,
            chosen: Group::App,
            search,
            node_config: None,
            pages,
            fields,
            _fields: subscriptions,
        }
    }

    /// Asks the connected node for its config.
    fn read_node_config(&mut self, cx: &mut Context<Self>) {
        let asked = self
            .host
            .request(Request::get(NodeId::default(), "/api/config"), cx);
        cx.spawn(async move |this, cx| {
            let got = match asked.await {
                Ok(a) if a.ok() => serde_json::from_slice(&a.body).map_err(|e| e.to_string()),
                Ok(a) => Err(a.error_text()),
                Err(e) => Err(e.message),
            };
            let _ = this.update(cx, |p, cx| {
                p.node_config = Some(got);
                cx.notify();
            });
        })
        .detach();
    }

    /// The connected node's configuration, a part for each section it
    /// loaded, each key a named line; `matches` keeps the lines a search
    /// asks for. Then where it lives, and how it changes.
    fn node_parts(
        &self,
        t: &Theme,
        within: Option<&str>,
        matches: &dyn Fn(&str) -> bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let named = |part: &str| match within {
            Some(group) => format!("{group} · {part}"),
            None => part.to_string(),
        };
        let mut parts = vec![];
        let config = match &self.node_config {
            None => {
                return vec![
                    pane::empty(t, "Reading the node's configuration…", None).into_any_element(),
                ];
            }
            Some(Err(why)) => {
                return vec![
                    pane::empty(
                        t,
                        format!("The node's configuration did not read: {why}"),
                        None,
                    )
                    .into_any_element(),
                ];
            }
            Some(Ok(config)) => config,
        };
        for (name, keys) in config["sections"].as_object().into_iter().flatten() {
            let mut part = pane::section(t, named(&words(name, true)));
            let mut any = false;
            for (key, value) in keys.as_object().into_iter().flatten() {
                let label = config_label(name, key);
                if !matches(&label) && !matches(name) {
                    continue;
                }
                any = true;
                let mut lines = div().flex().flex_col();
                for v in plain(value) {
                    // Each named for tests by where it is and what it says.
                    let selector = format!("node-config:{name}:{key}:{v}");
                    lines = lines.child(
                        div()
                            .debug_selector(move || selector)
                            .w_full()
                            .min_w_0()
                            .whitespace_normal()
                            .text_color(t.text)
                            .child(v),
                    );
                }
                part = part.child(pane::line(t, label, lines).items_start());
            }
            if any {
                parts.push(part.into_any_element());
            }
        }
        let path = config["path"].as_str().unwrap_or_default().to_string();
        if matches("file") || matches(&path) {
            let selector = format!("node-config:path:{path}");
            let open = path.clone();
            parts.push(
                pane::section(t, named("File"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(size::HEAD_GAP))
                            .child(
                                div()
                                    .debug_selector(move || selector)
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(t.text)
                                    .child(path),
                            )
                            .child(
                                pane::act(t, "node-config-open", "Open")
                                    .tooltip(kit::tip("Edit it in Files"))
                                    .on_click(cx.listener(move |p, _, _, cx| {
                                        let ask = Ask::OpenPath {
                                            node: NodeId::default(),
                                            path: open.clone(),
                                            beside: false,
                                        };
                                        if let Err(e) = p.host.ask(ask, cx) {
                                            p.host.report(Status::failure(e.message), cx);
                                        }
                                    })),
                            ),
                    )
                    .child(kit::note(
                        t,
                        "A change applies when the node restarts; sessions stay.",
                        Some(t.dim),
                    ))
                    .into_any_element(),
            );
        }
        parts
    }

    /// Each field shows what is kept now.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        for (&(owner, key), field) in &self.fields {
            let kept = self.value(owner, key, cx);
            let text = kept.as_str().unwrap_or_default().to_string();
            field.update(cx, |f, cx| f.set(&text, false, cx));
        }
    }

    /// What is kept for `key` now, or its default.
    fn value(&self, owner: Owner, key: &str, cx: &App) -> Value {
        let hub = self.hub.read(cx);
        match owner {
            Owner::App => match key {
                "node" => Value::from(hub.settings.node.clone()),
                "show_keys" => Value::from(hub.settings.show_keys),
                _ => Value::from(hub.settings.keys),
            },
            Owner::Page(id) => {
                let declared = self.declared(owner, key);
                declared.read(hub.settings.value(id, key))
            }
        }
    }

    fn declared(&self, owner: Owner, key: &str) -> &'static Setting {
        let list: &'static [Setting] = match owner {
            Owner::App => APP,
            Owner::Page(id) => {
                self.pages
                    .iter()
                    .find(|p| p.id == id)
                    .expect("a listed page")
                    .settings
            }
        };
        list.iter().find(|s| s.key == key).expect("a declared key")
    }

    /// Keeps a new value: the shell's own take effect here, a page's go
    /// into its section and the page hears it.
    fn write(&mut self, owner: Owner, key: &'static str, value: Value, cx: &mut Context<Self>) {
        self.hub.update(cx, |h, cx| match owner {
            Owner::App => {
                if key == "node" {
                    let node = value.as_str().unwrap_or_default();
                    h.settings.node = if node.is_empty() {
                        DEFAULT_NODE.into()
                    } else {
                        node.into()
                    };
                    h.connect(cx);
                } else if key == "show_keys" {
                    h.settings.show_keys = value.as_bool().unwrap_or(false);
                } else {
                    h.settings.keys = value.as_bool().unwrap_or(true);
                }
                h.save(cx);
                cx.notify();
            }
            Owner::Page(id) => h.set_value(id, key, value, cx),
        });
        if owner == Owner::App && key == "node" {
            let node = self.hub.read(cx).settings.node.clone();
            self.fields[&(owner, key)].update(cx, |f, cx| f.set(&node, false, cx));
        }
        cx.notify();
    }

    fn control(
        &self,
        owner: Owner,
        setting: &Setting,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = setting.key;
        let id = format!("setting-{}-{key}", owner.name());
        let value = self.value(owner, key, cx);
        match setting.control {
            Control::Toggle { .. } => {
                let on = value.as_bool().unwrap_or(false);
                let selector = id.clone();
                kit::toggle(t, SharedString::from(id), on)
                    .debug_selector(move || selector)
                    .on_click(
                        cx.listener(move |p, _, _, cx| p.write(owner, key, Value::from(!on), cx)),
                    )
                    .into_any_element()
            }
            Control::Number { .. } => {
                let declared = *setting;
                let step = |steps: f64, text: &'static str, cx: &mut Context<Self>| {
                    let name = format!("{id}-{}", if steps < 0. { "down" } else { "up" });
                    let selector = name.clone();
                    let now = value.clone();
                    kit::button(t, SharedString::from(name), text, Kind::Plain)
                        .debug_selector(move || selector)
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.write(owner, key, declared.stepped(&now, steps), cx)
                        }))
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(step(-1., "−", cx))
                    .child(
                        div()
                            .min_w(px(size::CONTROL * 2.))
                            .flex()
                            .justify_center()
                            .text_color(t.text)
                            .child(match value.as_f64() {
                                Some(n) if n.fract() == 0. => format!("{n:.0}"),
                                _ => value.to_string(),
                            }),
                    )
                    .child(step(1., "+", cx))
                    .into_any_element()
            }
            Control::Text { .. } => {
                let field = self.fields[&(owner, key)].clone();
                let selector = format!("{id}:{}", field.read(cx).text);
                div()
                    .debug_selector(move || selector)
                    .w_full()
                    .min_w_0()
                    .child(field)
                    .into_any_element()
            }
        }
    }

    /// The lines of `settings` that `matches` keeps.
    fn lines(
        &self,
        owner: Owner,
        settings: &[Setting],
        matches: &dyn Fn(&str) -> bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![];
        for setting in settings {
            if !matches(setting.label) && !matches(setting.note) {
                continue;
            }
            let value = div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(size::RADIUS))
                .child(self.control(owner, setting, t, cx))
                .when(!setting.note.is_empty(), |d| {
                    d.child(kit::note(t, setting.note, Some(t.dim)))
                });
            out.push(
                pane::line(t, setting.label, value)
                    .items_start()
                    .into_any_element(),
            );
        }
        out
    }

    /// The groups in the list's order, each with its name.
    fn groups(&self) -> Vec<(Group, String)> {
        let mut all = vec![(Group::App, "App".to_string())];
        all.extend(
            self.pages
                .iter()
                .map(|m| (Group::Page(m.id), m.title.to_string())),
        );
        all.push((Group::Node, "Node".to_string()));
        all
    }

    /// The parts of one group, or of every group a search matches.
    /// `within` when they stand under the group's own name, as a search
    /// shows them.
    fn parts(
        &self,
        group: Group,
        within: bool,
        matches: &dyn Fn(&str) -> bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match group {
            Group::App => {
                let mut lines = self.lines(Owner::App, APP, matches, t, cx);
                let hub = self.hub.read(cx);
                if let Some(h) = &hub.fleet.here().hello
                    && matches("version")
                {
                    let words = format!("Parvi {} on {} · {}", h.version, h.device, h.environment);
                    lines.push(pane::line(t, "Version", words).into_any_element());
                }
                lines
            }
            Group::Page(id) => {
                let settings = self
                    .pages
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.settings)
                    .unwrap_or_default();
                self.lines(Owner::Page(id), settings, matches, t, cx)
            }
            Group::Node => self.node_parts(t, within.then_some("Node"), matches, cx),
        }
    }
}

/// What a node's config key is called here: the few every node has by
/// their meaning, any other as its words.
fn config_label(section: &str, key: &str) -> String {
    match (section, key) {
        ("node", "listen") => "Address".into(),
        ("terminals", "socket") => "tmux socket".into(),
        ("files", "roots") => "Folders".into(),
        (_, "enabled") => "On".into(),
        _ => words(key, true),
    }
}

/// A config key or section as words: `copy_on_select` reads "Copy on
/// select"; `first` capitalises it.
fn words(key: &str, first: bool) -> String {
    let spaced = key.replace(['_', '-'], " ");
    let mut c = spaced.chars();
    match c.next() {
        Some(f) if first => f.to_uppercase().chain(c).collect(),
        _ => spaced,
    }
}

/// A config value as plain lines: a string as written, a list one item a
/// line, on and off for a switch.
fn plain(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Bool(true) => vec!["on".into()],
        Value::Bool(false) => vec!["off".into()],
        Value::Array(items) if items.is_empty() => vec!["none".into()],
        Value::Array(items) => items.iter().flat_map(plain).collect(),
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{}: {}", words(k, false), plain(v).join(", ")))
            .collect(),
        other => vec![other.to_string()],
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let query = self.search.read(cx).text.trim().to_lowercase();
        let groups = self.groups();
        let mut rows = div().flex().flex_col();
        for (group, name) in &groups {
            let group = *group;
            let chosen = query.is_empty() && self.chosen == group;
            let id = match group {
                Group::App => "app".to_string(),
                Group::Page(p) => p.to_string(),
                Group::Node => "node".to_string(),
            };
            let selector = format!("settings-group-{id}");
            rows = rows.child(
                pane::row(&t, SharedString::from(selector.clone()), chosen)
                    .debug_selector(move || selector)
                    .child(pane::cell(name.clone()))
                    .on_click(cx.listener(move |p, _, window, cx| {
                        p.chosen = group;
                        p.search.update(cx, |f, cx| f.set("", false, cx));
                        let _ = window;
                        cx.notify();
                    })),
            );
        }
        let list = pane::list(
            &t,
            "settings-groups",
            pane::list_head(&t, Some(self.search.clone().into_any_element())),
            rows,
        );
        let matches = |words: &str| query.is_empty() || words.to_lowercase().contains(&query);
        let (title, body) = if query.is_empty() {
            let name = groups
                .iter()
                .find(|(g, _)| *g == self.chosen)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            let title = match self.chosen {
                Group::Node => match &self.hub.read(cx).fleet.here().hello {
                    Some(h) => format!("Node on {} · {}", h.device, h.environment),
                    None => name,
                },
                _ => name,
            };
            (title, self.parts(self.chosen, false, &matches, &t, cx))
        } else {
            let mut all = vec![];
            for (group, name) in &groups {
                let parts = self.parts(*group, true, &matches, &t, cx);
                if parts.is_empty() {
                    continue;
                }
                // The node's parts carry their group's name themselves.
                if *group == Group::Node {
                    all.extend(parts);
                } else {
                    all.push(
                        pane::section(&t, name.clone())
                            .children(parts)
                            .into_any_element(),
                    );
                }
            }
            if all.is_empty() {
                all.push(pane::empty(&t, "No setting matches.", None).into_any_element());
            }
            (format!("Settings matching “{query}”"), all)
        };
        pane::page(
            &t,
            list,
            pane::detail(
                &t,
                "settings-page",
                pane::head(&t).child(pane::title(&t, title)),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(size::HEAD_GAP))
                    .children(body),
            ),
        )
    }
}
