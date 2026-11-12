//! The Settings page: the shell's own settings (the node, the keys), then
//! the settings each registered page declares ([`Page::SETTINGS`]), under
//! the page's title. Settings is not an area: what is listed comes from the
//! pages, and a change goes into the declaring page's section, which then
//! hears [`Page::settings_changed`]. Below them, read-only, is the connected
//! node's `config.toml` as the node loaded it (`/api/config`), read each time
//! the page comes forward or the node's link opens while it shows.

use gpui::{
    AnyElement, App, AppContext, Context, Entity, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use parvi_app_kit::host::Request;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::page::Meta;
use parvi_app_kit::settings::DEFAULT_NODE;
use parvi_app_kit::theme::{Theme, size, theme};
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pages::{line, page, section};
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
    Setting::toggle("keys", "Keys", true)
        .note("Chords and the leader take keys from the keyboard."),
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

pub struct SettingsPage {
    hub: Entity<Hub>,
    host: Host,
    shown: bool,
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
        Self {
            hub,
            host,
            shown: false,
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

    /// The connected node's config file and each of its sections, read-only.
    fn node_rows(&self, t: &Theme) -> AnyElement {
        // Each named for tests by where it is and what it says.
        let text = |at: &str, text: String| {
            let selector = format!("node-config:{at}:{text}");
            div()
                .debug_selector(move || selector)
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_color(t.text)
                .child(text)
        };
        let mut part = section(t, "Node");
        match &self.node_config {
            None => part = part.child(kit::note(t, "Reading the node's config…", None)),
            Some(Err(why)) => {
                let why = format!("The node's config did not read: {why}");
                part = part.child(kit::note(t, why, Some(t.dim)));
            }
            Some(Ok(config)) => {
                let path = config["path"].as_str().unwrap_or_default().to_string();
                part = part.child(
                    line(
                        t,
                        "File",
                        div().flex_1().min_w_0().child(text("path", path)),
                    )
                    .items_start(),
                );
                for (name, keys) in config["sections"].as_object().into_iter().flatten() {
                    let mut lines = div().flex_1().min_w_0().flex().flex_col();
                    for (key, value) in keys.as_object().into_iter().flatten() {
                        lines = lines.child(text(name, format!("{key} = {value}")));
                    }
                    part = part.child(line(t, &format!("[{name}]"), lines).items_start());
                }
            }
        }
        part.child(kit::note(
            t,
            "An agent edits this file; restarting the node applies it, and sessions survive.",
            Some(t.dim),
        ))
        .into_any_element()
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
                            .child(value.to_string()),
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

    fn rows(
        &self,
        owner: Owner,
        title: &str,
        settings: &[Setting],
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut part = section(t, title);
        for setting in settings {
            let mut value = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(size::RADIUS));
            value = value.child(self.control(owner, setting, t, cx));
            if !setting.note.is_empty() {
                value = value.child(kit::note(t, setting.note, Some(t.dim)));
            }
            let row = line(t, setting.label, value).items_start();
            part = part.child(row);
        }
        part.into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut all = page(&t, "settings-page").child(self.rows(Owner::App, "App", APP, &t, cx));
        for meta in self.pages.clone() {
            all = all.child(self.rows(Owner::Page(meta.id), meta.title, meta.settings, &t, cx));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(kit::page_bar(&t).child(kit::page_title(&t, "Settings")))
            .child(all.child(self.node_rows(&t)).flex_1().min_h_0())
    }
}
