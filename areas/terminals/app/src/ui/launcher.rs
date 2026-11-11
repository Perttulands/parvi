//! The launcher an empty window shows: where (device and environment),
//! harness, folder, user, flags and name, then one button. Its rules live in
//! `store::launch`; what the chosen node offers comes in its launcher frame.

use super::folder_field::{FolderEvent, FolderField};
use super::page::Terminals;
use super::place;
use crate::remote::Remote;
use crate::store::workspace::LinkState;
use crate::store::{devices, launch, launch_flags};
use crate::theme::{size, theme};
use gpui::{Context, Entity, WeakEntity, Window, div, prelude::*, px};
use parvi_app_kit::host::{Host, Request};
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_core::NodeId;
use parvi_terminals_contract::Launch;

/// The launcher in the Send drawer, after the windows': its session is
/// pasted into rather than put in a window.
pub const SEND: usize = crate::store::workspace::MAX_WINDOWS;

pub struct Launcher {
    index: usize,
    shell: WeakEntity<Terminals>,
    /// The node picked to launch on; the serving node until one is.
    picked: Option<String>,
    pub harness: Option<String>,
    pub user: String,
    folder: Entity<FolderField>,
    flags: Entity<TextField>,
    name: Entity<TextField>,
    /// The operator typed a name: stop suggesting one.
    named: bool,
    flags_touched: bool,
    pending: bool,
    problem: Option<String>,
    host: Host,
    catalogue_key: Option<(String, String)>,
    catalogue: Option<Result<Vec<launch_flags::Flag>, String>>,
    panel_open: bool,
    search: Entity<TextField>,
    draft: Entity<TextField>,
    editing: Option<launch_flags::Flag>,
}

impl Launcher {
    pub fn new(
        index: usize,
        shell: WeakEntity<Terminals>,
        host: Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = cx.new(|cx| FolderField::new(index, host.clone(), window, cx));
        let flags = cx.new(|cx| TextField::new(cx, "No flags"));
        let search = cx.new(|cx| TextField::new(cx, "Search flags"));
        let draft = cx.new(|cx| TextField::new(cx, "Value"));
        let name = cx.new(|cx| TextField::new(cx, "session-name"));
        cx.subscribe_in(&folder, window, |l: &mut Self, _, ev, window, cx| {
            l.problem = None;
            if let FolderEvent::Submit = ev {
                l.launch(window, cx);
            }
            cx.notify();
        })
        .detach();
        for f in [&flags, &name] {
            cx.subscribe_in(f, window, |l: &mut Self, f, ev, window, cx| match ev {
                FieldEvent::Submit => l.launch(window, cx),
                FieldEvent::Changed => {
                    if *f == l.name {
                        l.named = !l.name.read(cx).text.is_empty();
                    }
                    if *f == l.flags {
                        l.flags_touched = true;
                    }
                    l.problem = None;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
        }
        for field in [&search, &draft] {
            cx.subscribe_in(field, window, |l: &mut Self, field, ev, _, cx| {
                if matches!(ev, FieldEvent::Changed) {
                    cx.notify();
                }
                if matches!(ev, FieldEvent::Submit) && *field == l.draft {
                    l.commit_flag(cx);
                }
            })
            .detach();
        }
        Self {
            index,
            shell,
            picked: None,
            harness: None,
            user: String::new(),
            folder,
            flags,
            name,
            named: false,
            flags_touched: false,
            pending: false,
            problem: None,
            host,
            catalogue_key: None,
            catalogue: None,
            panel_open: false,
            search,
            draft,
            editing: None,
        }
    }

    pub fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.folder.read(cx).field.clone();
        field.update(cx, |f, cx| f.set(&f.text.clone(), true, cx));
        parvi_app_kit::widgets::field::focus(&field, window, cx);
        cx.notify();
    }

    /// Points the launcher at a node: its harnesses, folders, users and
    /// flags are offered from then on. A folder or user chosen for another
    /// node is dropped, since it need not exist on this one.
    pub fn point_at(&mut self, node: &str, cx: &mut Context<Self>) {
        if self.picked.as_deref() == Some(node) {
            return;
        }
        self.picked = Some(node.to_string());
        self.panel_open = false;
        self.user.clear();
        self.flags_touched = false;
        self.problem = None;
        self.folder.update(cx, |f, cx| f.set("", cx));
        cx.notify();
    }

    /// The node a launch goes to now.
    pub fn node(&self, shell: &Terminals) -> Option<String> {
        launch::place(self.picked.as_deref(), shell.fleet.here_id(), |n| {
            shell.fleet.get(n).is_some()
        })
    }

    fn client<'a>(&self, shell: &'a Terminals) -> Option<&'a Remote> {
        shell.fleet.get(&self.node(shell)?)
    }

    pub fn done(&mut self, cx: &mut Context<Self>) {
        self.pending = false;
        self.named = false;
        self.name.update(cx, |f, cx| f.set("", false, cx));
        cx.notify();
    }

    /// Back to the node's flags for this harness, forgetting the ones used.
    fn reset_flags(&mut self, harness: &str, default: &str, cx: &mut Context<Self>) {
        self.flags_touched = false;
        self.flags.update(cx, |f, cx| f.set(default, false, cx));
        if let Some(shell) = self.shell.upgrade() {
            shell.update(cx, |s, cx| {
                if s.settings.flags.remove(harness).is_some() {
                    s.save(cx);
                }
            });
        }
        cx.notify();
    }

    fn choose(&mut self, harness: &str, cx: &mut Context<Self>) {
        self.harness = Some(harness.to_string());
        self.panel_open = false;
        self.flags_touched = false;
        cx.notify();
    }

    fn fetch_flags(&mut self, node: String, harness: String, cx: &mut Context<Self>) {
        let key = (node, harness);
        if self.catalogue_key.as_ref() == Some(&key) {
            return;
        }
        self.catalogue_key = Some(key.clone());
        self.catalogue = None;
        self.editing = None;
        let path = format!("/api/launch/flags?harness={}", crate::http::enc(&key.1));
        let task = self
            .host
            .request(Request::get(NodeId(key.0.clone()), path), cx);
        cx.spawn(async move |this, cx| {
            let answer = crate::http::json(task.await);
            let _ = this.update(cx, |l, cx| {
                if l.catalogue_key.as_ref() == Some(&key) {
                    l.catalogue = Some(answer);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn commit_flag(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = self.editing.take() {
            let value = self.draft.read(cx).text.trim().to_string();
            let line = launch_flags::add(&self.flags.read(cx).text, &flag, &value);
            self.flags.update(cx, |f, cx| f.set(&line, false, cx));
            self.flags_touched = true;
            cx.notify();
        }
    }

    fn pick_flag(&mut self, flag: launch_flags::Flag, cx: &mut Context<Self>) {
        let line = self.flags.read(cx).text.clone();
        if launch_flags::has(&line, &flag) {
            let line = launch_flags::remove(&line, &flag);
            self.flags.update(cx, |f, cx| f.set(&line, false, cx));
            self.flags_touched = true;
            self.editing = None;
        } else if flag.value.is_some() {
            let default = flag.values.first().cloned().unwrap_or_default();
            self.draft.update(cx, |f, cx| f.set(&default, false, cx));
            self.editing = Some(flag);
        } else {
            let line = launch_flags::add(&line, &flag, "");
            self.flags.update(cx, |f, cx| f.set(&line, false, cx));
            self.flags_touched = true;
        }
        cx.notify();
    }

    /// The harness, folder, flags and name the launch would use now.
    fn values(
        &self,
        shell: &Terminals,
        cx: &gpui::App,
    ) -> Option<(String, String, String, String, String)> {
        let node = self.client(shell)?;
        let l = node.launcher.as_ref()?;
        let harness = self
            .harness
            .clone()
            .or_else(|| shell.settings.last_harness.clone())
            .filter(|h| l.harnesses.iter().any(|x| &x.id == h))
            .or_else(|| l.harnesses.first().map(|h| h.id.clone()))?;
        let h = l.harnesses.iter().find(|x| x.id == harness)?;
        let typed_folder = self.folder.read(cx).chosen(cx);
        let folder = if typed_folder.is_empty() {
            launch::folders(&l.folders, &node.sessions, &self.user)
                .into_iter()
                .next()
                .unwrap_or_else(|| "~".into())
        } else {
            typed_folder
        };
        let flags = if self.flags_touched {
            self.flags.read(cx).text.clone()
        } else {
            shell
                .settings
                .flags
                .get(&harness)
                .cloned()
                .unwrap_or_else(|| h.flags.clone())
        };
        // Names are per tmux server and user: another user may have mine.
        let taken: Vec<&str> = node
            .sessions
            .iter()
            .filter(|s| s.user.as_deref().unwrap_or("") == self.user)
            .map(|s| s.name.as_str())
            .collect();
        let name = if self.named {
            self.name.read(cx).text.trim().to_string()
        } else {
            launch::suggest_name(&harness, &folder, &taken)
        };
        Some((harness, folder, flags, name, h.command.clone()))
    }

    fn launch(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let Some((harness, folder, flags, name, command)) = self.values(shell.read(cx), cx) else {
            return;
        };
        if let Some(p) = launch::name_problem(&name) {
            self.problem = Some(p.into());
            cx.notify();
            return;
        }
        let Some(node) = self.node(shell.read(cx)) else {
            return;
        };
        if self.client(shell.read(cx)).is_some_and(|c| {
            c.sessions
                .iter()
                .any(|s| s.name == name && s.user.as_deref().unwrap_or("") == self.user)
        }) {
            self.problem = Some(format!("{name} is taken."));
            cx.notify();
            return;
        }
        let index = self.index;
        let user = self.user.clone();
        let sent = shell.update(cx, |s, cx| {
            let conf = s.fleet.get(&node).and_then(|c| c.launcher.clone());
            let l = Launch {
                harness: harness.clone(),
                folder,
                name: name.clone(),
                flags: flags.clone(),
                user: user.clone(),
                notify: s.settings.notify
                    && notify_offered(&command, &user)
                    && conf.as_ref().is_some_and(|l| l.notify_off.is_none()),
                // Send's launcher targets the focused tile.
                size: s
                    .launch_grids
                    .borrow()
                    .get(if index == SEND {
                        s.settings.workspace.focus
                    } else {
                        index
                    })
                    .copied()
                    .flatten(),
            };
            let id = s.fleet.launch_on(&node, l.clone())?;
            let tab = s.settings.workspace.active;
            let window = (index != SEND).then_some(index);
            s.launches.insert((node.clone(), id), (l, window, tab));
            if index == SEND {
                s.send_launching(&node, id);
            }
            s.settings.last_harness = Some(harness.clone());
            let default = conf
                .as_ref()
                .and_then(|l| l.harnesses.iter().find(|h| h.id == harness))
                .map(|h| h.flags.clone());
            if default.as_deref() != Some(flags.as_str()) {
                s.settings.flags.insert(harness.clone(), flags.clone());
            } else {
                s.settings.flags.remove(&harness);
            }
            s.report(&format!("Launching {name}…"), false, None);
            s.save(cx);
            cx.notify();
            Some(id)
        });
        if sent.is_some() {
            self.pending = true;
        } else {
            let away = shell.read(cx).where_is(&node).map(|p| p.device);
            self.problem = Some(match away {
                Some(device) => format!("{device} is away; launch when it answers."),
                None => "Not connected to the node.".into(),
            });
        }
        cx.notify();
    }
}

impl Launcher {
    /// The launcher's first row: every device and environment the app
    /// knows, the one launched on marked, an away one dimmed and still.
    /// One node alone has nothing to choose, and no row.
    fn places(
        &self,
        devices: &[devices::DeviceRows],
        target: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = theme(cx).clone();
        if devices.iter().map(|d| d.nodes.len()).sum::<usize>() < 2 {
            return None;
        }
        let i = self.index;
        let mut row = div().flex().flex_wrap().gap(px(6.));
        for d in devices {
            for n in &d.nodes {
                let chosen = target == Some(n.node.as_str());
                let node = n.node.clone();
                let chip = div()
                    .id(gpui::SharedString::from(format!("place-{i}-{}", n.node)))
                    .h(px(28.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(size::RADIUS))
                    .border_1()
                    .border_color(if chosen { t.accent } else { t.divider })
                    .when(chosen, |c| c.bg(t.accent_soft()))
                    .child(place::passport(&t, &d.device, &n.environment));
                row = row.child(if n.live {
                    chip.cursor_pointer()
                        .on_click(cx.listener(move |l, _, _, cx| l.point_at(&node, cx)))
                } else {
                    let why = format!("{} is away", d.device);
                    chip.opacity(crate::theme::fade::DIMMED)
                        .tooltip(move |_, cx| cx.new(|_| kit::Tip(why.clone().into())).into())
                });
            }
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .pb(px(6.))
                .child(kit::label(&t, "Where"))
                .child(row)
                .into_any_element(),
        )
    }
}

/// Every device and environment the app knows, as the rail orders them,
/// each saying whether it answers now.
fn where_rows(shell: &Terminals) -> Vec<devices::DeviceRows> {
    let fleet = &shell.fleet;
    devices::devices(&shell.places, &shell.settings.kept, fleet.here_id(), |n| {
        (
            fleet.get(n).is_some_and(|c| c.state == LinkState::Open),
            None,
        )
    })
}

/// The node hooks a harness it knows, on its own tmux server only.
pub(super) fn notify_offered(command: &str, user: &str) -> bool {
    user.is_empty() && launch::notify_label(command).is_some()
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let Some(shell) = self.shell.upgrade() else {
            return div();
        };
        let shell = shell.read(cx);
        let target = self.node(shell);
        let rows = where_rows(shell);
        let client = self.client(shell);
        let conf = client.and_then(|c| c.launcher.clone());
        let waiting = conf.is_none().then(|| {
            let open = client.is_some_and(|c| c.state == LinkState::Open);
            let device = target
                .as_deref()
                .and_then(|n| shell.where_is(n))
                .map(|p| p.device);
            match (open, device) {
                (true, _) => "Waiting for the node's launcher…".to_string(),
                (false, Some(d)) => format!("Waiting for {d}; it comes back by itself."),
                (false, None) => "Not connected to the node.".to_string(),
            }
        });
        let sessions = client.map(|c| &c.sessions[..]).unwrap_or_default();
        let offered = conf
            .as_ref()
            .map(|c| launch::folders(&c.folders, sessions, &self.user))
            .unwrap_or_default();
        let notify_on = shell.settings.notify;
        let values = self.values(shell, cx);
        let places = self.places(&rows, target.as_deref(), cx);
        let alone = |places: Option<gpui::AnyElement>, note: String| {
            kit::panel(&t)
                .w(px(size::LAUNCHER))
                .max_w_full()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(places)
                .child(kit::note(&t, note, None))
        };
        let Some(conf) = conf else {
            return alone(places, waiting.unwrap_or_default());
        };
        let Some((harness, folder, flags, name, command)) = values else {
            return alone(places, "The node offers no harness.".into());
        };
        // Keep the fields showing what a launch would use.
        if !self.flags_touched && self.flags.read(cx).text != flags {
            self.flags.update(cx, |f, cx| f.set(&flags, false, cx));
            self.flags_touched = false;
        }
        if !self.named && self.name.read(cx).text != name && !self.name.read(cx).is_focused(window)
        {
            self.name.update(cx, |f, cx| f.set(&name, false, cx));
            self.named = false;
        }
        let at = target.clone().unwrap_or_default();
        let field_empty = self.folder.read(cx).text(cx).is_empty();
        let field_focused = self.folder.read(cx).field.read(cx).is_focused(window);
        self.folder.update(cx, |f, cx| {
            f.serve(&at, offered);
            if field_empty && !field_focused {
                f.set(&folder, cx);
            }
        });
        let i = self.index;
        let mut harnesses = div().flex().flex_col().gap(px(8.));
        for (n, h) in conf.harnesses.iter().enumerate() {
            let selected = h.id == harness;
            let id = h.id.clone();
            harnesses = harnesses.child(
                div()
                    .id(("harness", i * 100 + n))
                    .debug_selector({
                        let id = h.id.clone();
                        move || format!("harness-{i}-{id}")
                    })
                    .h(px(32.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(size::RADIUS))
                    .border_1()
                    .border_color(if selected { t.accent } else { t.divider })
                    .when(selected, |d| d.bg(t.accent_soft()))
                    .cursor_pointer()
                    .children(crate::harness::harness_mark(&h.id, 14., cx))
                    .child(launch::harness_label(&h.id))
                    .on_click(cx.listener(move |l, _, _, cx| l.choose(&id, cx))),
            );
        }
        let users = (!conf.users.is_empty()).then(|| {
            let mut row = div().flex().gap(px(6.)).flex_wrap();
            let me = String::new();
            for (n, u) in std::iter::once(&me).chain(conf.users.iter()).enumerate() {
                let chosen = *u == self.user;
                let value = u.clone();
                let label = if u.is_empty() {
                    "service user".to_string()
                } else {
                    u.clone()
                };
                row = row.child(
                    div()
                        .id(("user", i * 100 + n))
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(size::RADIUS))
                        .border_1()
                        .border_color(if chosen { t.accent } else { t.divider })
                        .when(chosen, |d| d.bg(t.accent_soft()))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .when(!u.is_empty(), |d| d.child(kit::user_square(&t, u)))
                        .child(label)
                        .on_click(cx.listener(move |l, _, _, cx| {
                            l.user = value.clone();
                            cx.notify();
                        })),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(kit::label(&t, "User"))
                .child(row)
        });
        let preview = launch::command_line(&command, &flags);
        let short = launch::short(&harness).to_string();
        let place = folder
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|x| !x.is_empty())
            .unwrap_or("~")
            .to_string();
        let button_label = if self.pending {
            "Launching…".to_string()
        } else if command.is_empty() {
            format!("Open shell in {place}")
        } else {
            format!("Launch {short} in {place}")
        };
        let default_flags = conf
            .harnesses
            .iter()
            .find(|h| h.id == harness)
            .map(|h| h.flags.clone())
            .unwrap_or_default();
        let notify_off = conf.notify_off.clone();
        let notify = launch::notify_label(&command)
            .filter(|_| notify_offered(&command, &self.user))
            .map(|label| match notify_off {
                // The node can't install hooks: say so where Notify would be,
                // rather than promise a notice that never comes.
                Some(why) => div().mt(px(6.)).child(kit::note(
                    &t,
                    format!("No completion notices from this device: {why}"),
                    Some(t.error),
                )),
                None => div()
                    .mt(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        kit::toggle(&t, ("notify", i), notify_on).on_click(cx.listener(
                            |l, _, _, cx| {
                                if let Some(shell) = l.shell.upgrade() {
                                    shell.update(cx, |s, cx| {
                                        s.settings.notify = !s.settings.notify;
                                        s.save(cx);
                                    });
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(div().text_size(px(12.)).text_color(t.text2).child(label)),
            });
        let reset = (flags.trim() != default_flags.trim()).then(|| {
            let h = harness.clone();
            kit::button(&t, ("flags-reset", i), "Reset", Kind::Quiet)
                .on_click(cx.listener(move |l, _, _, cx| l.reset_flags(&h, &default_flags, cx)))
        });
        let panel = self.panel_open.then(|| {
            let mut list = div()
                .id(("flag-list", i))
                .max_h(px(200.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.));
            let query = self.search.read(cx).text.trim().to_lowercase();
            match &self.catalogue {
                None => {
                    list = list.child("Reading flags…");
                }
                Some(Err(error)) => {
                    list = list.child(format!("Could not read flags: {error}"));
                }
                Some(Ok(entries)) => {
                    for (n, flag) in entries.iter().enumerate() {
                        if !format!(
                            "{} {} {}",
                            flag.name,
                            flag.short.as_deref().unwrap_or(""),
                            flag.description
                        )
                        .to_lowercase()
                        .contains(&query)
                        {
                            continue;
                        }
                        let selected = launch_flags::has(&flags, flag);
                        let chosen = flag.clone();
                        let name = format!(
                            "{}{}{}",
                            if selected { "✓ " } else { "  " },
                            flag.short
                                .as_ref()
                                .map(|short| format!("{short}, {}", flag.name))
                                .unwrap_or_else(|| flag.name.clone()),
                            if selected && flag.value.is_some() {
                                format!(" {}", launch_flags::value(&flags, flag))
                            } else {
                                flag.value
                                    .as_deref()
                                    .map(|v| format!(" {v}"))
                                    .unwrap_or_default()
                            }
                        );
                        list = list.child(
                            div()
                                .id(("catalogue-flag", i * 1000 + n))
                                .debug_selector({
                                    let name = flag.name.clone();
                                    move || format!("flag-{i}-{name}")
                                })
                                .cursor_pointer()
                                .p(px(4.))
                                .flex()
                                .flex_col()
                                .child(name)
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(t.dim)
                                        .child(flag.description.clone()),
                                )
                                .on_click(cx.listener(move |l, _, window, cx| {
                                    l.pick_flag(chosen.clone(), cx);
                                    if l.editing.is_some() {
                                        parvi_app_kit::widgets::field::focus(&l.draft, window, cx);
                                    }
                                })),
                        );
                    }
                }
            }
            let editing = self.editing.as_ref().map(|flag| {
                let mut editor = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(self.draft.clone());
                if !flag.values.is_empty() {
                    let mut choices = div().flex().flex_wrap().gap(px(4.));
                    for (n, value) in flag.values.iter().enumerate() {
                        let v = value.clone();
                        choices = choices.child(
                            kit::button(
                                &t,
                                ("flag-choice", i * 1000 + n),
                                value.clone(),
                                Kind::Quiet,
                            )
                            .on_click(cx.listener(
                                move |l, _, _, cx| {
                                    l.draft.update(cx, |f, cx| f.set(&v, false, cx));
                                    cx.notify();
                                },
                            )),
                        );
                    }
                    editor = editor.child(choices);
                }
                editor.child(
                    kit::button(&t, ("flag-add", i), "Add", Kind::Quiet)
                        .on_click(cx.listener(|l, _, _, cx| l.commit_flag(cx))),
                )
            });
            div()
                .on_key_down(cx.listener(|l, e: &gpui::KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        l.panel_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(self.search.clone())
                .child(list)
                .children(editing)
        });
        kit::panel(&t)
            .w(px(size::LAUNCHER))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(places)
            .child(kit::label(&t, "Launch"))
            .child(harnesses)
            .child(div().mt(px(6.)).child(kit::label(&t, "Folder")))
            .child(self.folder.clone())
            .children(users)
            .child(
                div()
                    .mt(px(6.))
                    .h(px(size::CONTROL))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(kit::label(&t, "Flags"))
                    .child(
                        kit::button(
                            &t,
                            ("flags-browse", i),
                            if self.panel_open { "Close" } else { "Browse" },
                            Kind::Quiet,
                        )
                        .debug_selector(move || format!("flags-browse-{i}"))
                        .on_click(cx.listener({
                            let node = target.clone().unwrap_or_default();
                            let harness = harness.clone();
                            move |l, _, window, cx| {
                                l.panel_open = !l.panel_open;
                                if l.panel_open {
                                    if matches!(l.catalogue, Some(Err(_))) {
                                        l.catalogue_key = None;
                                    }
                                    l.fetch_flags(node.clone(), harness.clone(), cx);
                                    parvi_app_kit::widgets::field::focus(&l.search, window, cx);
                                }
                                cx.notify();
                            }
                        })),
                    )
                    .children(reset),
            )
            .child(self.flags.clone())
            .children(panel)
            .child(
                div()
                    .h(px(16.))
                    .text_size(px(11.))
                    .text_color(t.dim)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .debug_selector({
                        let flags = flags.clone();
                        move || format!("flag-line-{i}-{flags}")
                    })
                    .child(if preview.is_empty() {
                        "a shell".to_string()
                    } else {
                        preview
                    }),
            )
            .child(div().mt(px(6.)).child(kit::label(&t, "Name")))
            .child(self.name.clone())
            .children(notify)
            .children(
                self.problem
                    .clone()
                    .map(|p| kit::note(&t, p, Some(t.error))),
            )
            .child(
                div().mt(px(6.)).flex().justify_end().child(
                    kit::button(&t, ("launch", i), button_label, Kind::Primary)
                        .debug_selector(move || format!("launch-{i}"))
                        .on_click(cx.listener(|l, _, window, cx| l.launch(window, cx))),
                ),
            )
    }
}
