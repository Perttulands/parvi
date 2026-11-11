//! The page: what it reads from each node, what the form changes, and the
//! requests that save, act and delete. Drawing is in [`crate::view`].

use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, Window};
use parvi_app_kit::host::Request;
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::{Host, Page};
use parvi_core::NodeId;
use parvi_scheduled_contract::{CHANGED, INTENT, INTENT_VALUE, PATH, Target, Task, WATCH};
use parvi_terminals_contract::SessionList;
use serde_json::{Value, json};

use crate::ACTOR;
use crate::form::TaskForm;
use crate::state::{Saved, Scheduled, data};

/// How often the countdowns move: CHROTE's.
const TICK: Duration = Duration::from_secs(30);

/// The page's text fields.
pub(crate) struct Fields {
    pub prompt: Entity<TextField>,
    pub name: Entity<TextField>,
    pub every: Entity<TextField>,
    pub time: Entity<TextField>,
    pub cron: Entity<TextField>,
}

#[derive(Clone, Copy)]
enum Field {
    Prompt,
    Name,
    Every,
    Time,
    Cron,
}

/// A session a task may send to, as a node's terminals list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Session {
    pub name: String,
    pub user: Option<String>,
}

impl Session {
    pub fn target(&self) -> Target {
        Target {
            session_name: self.name.clone(),
            unix_user: self.user.clone().unwrap_or_default(),
        }
    }
}

pub struct ScheduledPage {
    pub(crate) host: Host,
    pub(crate) state: Scheduled,
    pub(crate) fields: Fields,
    /// The sessions the terminals page hears, across nodes.
    sessions: Option<Entity<SessionList>>,
    /// The page is on show.
    shown: bool,
    /// The countdown clock runs (only while the page is shown).
    ticking: bool,
}

impl Page for ScheduledPage {
    const ID: &'static str = crate::NAME;
    const TITLE: &'static str = "Scheduled";
    const AREA: Option<&'static str> = Some(crate::NAME);
    const KINDS: parvi_core::frame::Kinds = &[CHANGED..=CHANGED];
    const CHANNELS: &'static [u8] = &[WATCH];

    fn frame(&mut self, node: &NodeId, _: u8, _: &[u8], _: &mut Window, cx: &mut Context<Self>) {
        if self.shown {
            self.read(node.as_str(), cx);
        }
    }

    /// A node that came into reach while the page shows is read at once;
    /// one that came while it was behind is read when it comes forward.
    fn link(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shown && state == Some(LinkState::Open) {
            self.read(node.as_str(), cx);
        }
    }

    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.read_all(cx);
        }
    }
}

impl ScheduledPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut make = |which: Field, placeholder: &str, prose: bool| {
            let f = cx.new(|cx| {
                let f = TextField::new(cx, placeholder);
                if prose { f.prose() } else { f }
            });
            cx.subscribe_in(&f, window, move |page, f, ev, _, cx| {
                if let FieldEvent::Changed = ev {
                    let text = f.read(cx).text.clone();
                    page.state.change(|form| match which {
                        Field::Prompt => form.prompt = text,
                        Field::Name => form.name = text,
                        Field::Every => form.schedule.every_value = text,
                        Field::Time => form.schedule.time = text,
                        Field::Cron => form.schedule.cron = text,
                    });
                    cx.notify();
                }
            })
            .detach();
            f
        };
        let fields = Fields {
            prompt: make(Field::Prompt, "Continue if work is clear…", true),
            name: make(
                Field::Name,
                "Defaults to the first line of the prompt",
                false,
            ),
            every: make(Field::Every, "30", false),
            time: make(Field::Time, "HH:MM", false),
            cron: make(Field::Cron, "minute hour day month weekday", false),
        };
        let sessions = host.model::<SessionList>(cx);
        if let Some(sessions) = &sessions {
            cx.observe(sessions, |_, _, cx| cx.notify()).detach();
        }
        ScheduledPage {
            host,
            state: Scheduled::default(),
            fields,
            sessions,
            shown: false,
            ticking: false,
        }
    }

    /// A node's device and environment, for words.
    pub(crate) fn place_name(&self, node: &str, cx: &gpui::App) -> String {
        self.host
            .nodes(cx)
            .into_iter()
            .find(|n| n.id.as_str() == node)
            .map(|n| format!("{} ({})", n.device, n.environment))
            .unwrap_or_else(|| node.to_string())
    }

    /// Sends `request` to `node` and hands the answer's status and body to
    /// `done`; a request that went nowhere reads as status 0 and its reason.
    fn ask(
        &mut self,
        request: Request,
        done: impl FnOnce(&mut Self, u16, Vec<u8>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let asked = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let (status, body) = match asked.await {
                Ok(a) => (a.status, a.body),
                Err(e) => (0, e.message.into_bytes()),
            };
            let _ = this.update(cx, |page, cx| done(page, status, body, cx));
        })
        .detach();
    }

    /// A change to the node's tasks: it carries the intent header.
    fn write(node: &str, method: &'static str, path: &str, body: Option<Value>) -> Request {
        let request =
            Request::new(method, NodeId(node.to_string()), path).header(INTENT, INTENT_VALUE);
        match body {
            Some(body) => request.json(&body),
            None => request,
        }
    }

    /// Reads every live node's tasks (the page opened).
    pub(crate) fn read_all(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            self.read(node.id.as_str(), cx);
        }
        self.tick(cx);
    }

    /// The sessions on `node` a task may send to, by name.
    pub(crate) fn sessions_on(&self, node: &str, cx: &App) -> Vec<Session> {
        let Some(list) = &self.sessions else {
            return vec![];
        };
        let mut sessions: Vec<Session> = list
            .read(cx)
            .sessions
            .iter()
            .filter(|s| s.node == node)
            .map(|s| Session {
                name: s.name.clone(),
                user: s.user.clone(),
            })
            .collect();
        sessions.sort_by(|a, b| a.name.cmp(&b.name));
        sessions
    }

    /// Reads one node's tasks, once.
    pub(crate) fn read(&mut self, node: &str, cx: &mut Context<Self>) {
        let n = self.state.nodes.entry(node.to_string()).or_default();
        if n.reading {
            n.reread = true;
            return;
        }
        n.reading = true;
        let at = node.to_string();
        self.ask(
            Request::get(NodeId(node.to_string()), PATH),
            move |page, status, body, cx| {
                let n = page.state.nodes.entry(at.clone()).or_default();
                if std::mem::take(&mut n.reread) {
                    n.reading = false;
                    page.read(&at, cx);
                    return;
                }
                let before = page
                    .state
                    .selected_task()
                    .map(|(n, t)| (n.to_string(), t.clone()));
                page.state
                    .nodes
                    .entry(at)
                    .or_default()
                    .listed(status, &body);
                let after = page
                    .state
                    .selected_task()
                    .map(|(n, t)| (n.to_string(), t.clone()));
                if before != after && status == 200 {
                    page.state.notice = None;
                    page.state.delete_armed = None;
                }
                cx.notify();
            },
            cx,
        );
    }

    /// The countdowns move every 30 s while the page is shown.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let go_on = this
                    .update(cx, |page, cx| {
                        if !page.shown {
                            page.ticking = false;
                        }
                        cx.notify();
                        page.shown
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        })
        .detach();
    }

    /// Fills the fields from the form just opened.
    fn fill(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.state.form else {
            return;
        };
        let fields = &self.fields;
        let pairs = [
            (fields.prompt.clone(), f.prompt.clone()),
            (fields.name.clone(), f.name.clone()),
            (fields.every.clone(), f.schedule.every_value.clone()),
            (fields.time.clone(), f.schedule.time.clone()),
            (fields.cron.clone(), f.schedule.cron.clone()),
        ];
        for (field, text) in pairs {
            field.update(cx, |f, cx| f.set(&text, false, cx));
        }
    }

    pub(crate) fn open(&mut self, form: TaskForm, cx: &mut Context<Self>) {
        if self.state.open(form, now_ms()) {
            self.fill(cx);
        }
        cx.notify();
    }

    /// New task: on the node of the selected task, or the first live node
    /// (the serving node's).
    pub(crate) fn new_task(&mut self, cx: &mut Context<Self>) {
        let node = self
            .state
            .selected
            .as_ref()
            .map(|(n, _)| n.clone())
            .or_else(|| {
                self.host
                    .nodes(cx)
                    .first()
                    .map(|n| n.id.as_str().to_string())
            })
            .unwrap_or_default();
        self.open(TaskForm::new(&node, vec![]), cx);
    }

    /// Adds or removes a session as a target. A session of another node
    /// moves a new task there, with its sessions; an edited task stays on
    /// its node.
    pub(crate) fn pick(&mut self, node: &str, session: &Session, cx: &mut Context<Self>) {
        let target = session.target();
        let Some(form) = &self.state.form else {
            return;
        };
        if form.node != node {
            if form.id.is_some() {
                let place = self.place_name(&form.node.clone(), cx);
                self.state.notice = Some(format!(
                    "This task runs on {place}; its sessions are there."
                ));
                cx.notify();
                return;
            }
            let moved = !form.targets.is_empty();
            self.state.change(|f| {
                f.node = node.to_string();
                f.targets = vec![target];
            });
            if moved {
                let place = self.place_name(node, cx);
                self.state.notice = Some(format!(
                    "A task sends into one place's sessions: now {place}."
                ));
            }
            cx.notify();
            return;
        }
        self.state.change(|f| f.toggle(target));
        cx.notify();
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.state.form.clone() else {
            return;
        };
        if let Some(why) = form.error() {
            self.state.error = Some(why.into());
            cx.notify();
            return;
        }
        self.state.busy = true;
        self.state.error = None;
        let (method, path) = match &form.id {
            Some(id) => ("PATCH", format!("{PATH}/{id}")),
            None => ("POST", PATH.to_string()),
        };
        let node = form.node.clone();
        let request = Self::write(&node, method, &path, Some(form.body()));
        self.ask(
            request,
            move |page, status, body, cx| {
                let sc = &mut page.state;
                sc.busy = false;
                match data::<Saved>(status, &body) {
                    Ok(saved) => {
                        sc.notice = Some(
                            if form.id.is_some() {
                                "Task updated."
                            } else {
                                "Task created."
                            }
                            .into(),
                        );
                        sc.selected = Some((node.clone(), saved.task.id.clone()));
                        sc.nodes.entry(node.clone()).or_default().put(saved.task);
                        sc.form = None;
                        sc.dirty = false;
                        page.read(&node, cx);
                    }
                    Err(e) => sc.error = Some(e),
                }
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }

    /// Pause, resume or run now.
    pub(crate) fn act(
        &mut self,
        node: &str,
        task: &Task,
        action: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.state.busy = true;
        self.state.error = None;
        let path = format!("{PATH}/{}/{action}", task.id);
        let (at, targets) = (node.to_string(), task.targets.len());
        let request = Self::write(node, "POST", &path, Some(json!({ "updatedBy": ACTOR })));
        self.ask(
            request,
            move |page, status, body, cx| {
                let sc = &mut page.state;
                sc.busy = false;
                match data::<Saved>(status, &body) {
                    Ok(saved) => {
                        sc.notice = Some(match (action, &saved.run) {
                            ("run-now", Some(run)) => Scheduled::sent_words(run, targets),
                            ("pause", _) => "Task paused.".into(),
                            _ => "Task resumed.".into(),
                        });
                        sc.nodes.entry(at.clone()).or_default().put(saved.task);
                        page.read(&at, cx);
                    }
                    Err(e) => sc.error = Some(e),
                }
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }

    pub(crate) fn delete(&mut self, node: &str, id: &str, cx: &mut Context<Self>) {
        if self.state.busy {
            return;
        }
        if !self.state.press_delete(node, id) {
            cx.notify();
            return;
        }
        self.state.busy = true;
        let (at, gone) = (node.to_string(), id.to_string());
        let request = Self::write(node, "DELETE", &format!("{PATH}/{id}"), None);
        self.ask(
            request,
            move |page, status, body, cx| {
                let sc = &mut page.state;
                sc.busy = false;
                match data::<Value>(status, &body) {
                    Ok(_) => {
                        sc.notice = Some("Task deleted.".into());
                        if sc.form.as_ref().and_then(|f| f.id.as_deref()) == Some(gone.as_str()) {
                            sc.form = None;
                            sc.dirty = false;
                        }
                        if let Some(list) = sc.nodes.get_mut(&at).and_then(|n| n.list.as_mut()) {
                            list.retain(|t| t.id != gone);
                        }
                        page.read(&at, cx);
                    }
                    Err(e) => sc.error = Some(e),
                }
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }
}
