//! The Services page: what runs, what failed and what is stopped, on each
//! node; beside the list, the chosen service's address, units, actions and
//! latest log lines.
use crate::{
    services_store::{self, Change, Ended, Group, NO_ANSWER, Service, Services, Step},
    tokens,
};
use gpui::{
    App, ClipboardItem, Context, Div, FocusHandle, IntoElement, KeyDownEvent, ParentElement,
    Render, SharedString, Styled, Window, div, prelude::*, px,
};
use parvi_app_kit::{
    Ask, Host, Page,
    ask::Answers,
    host::{Node, Request, Status},
    hub::now_ms,
    link::client::LinkState,
    theme::{self, Theme, size},
    widgets::{kit, pane},
};
use parvi_core::NodeId;
use std::time::Duration;

const ACTION_WAIT: Duration = Duration::from_secs(50);
/// A running service started less than this long ago says so in the list.
const LATELY: u64 = 3_600_000;

pub struct ServicesPage {
    host: Host,
    state: Services,
    shown: bool,
    focus: FocusHandle,
    /// The list's scroll, and whether the row the keys reached is to be
    /// brought into view at the next draw.
    list_scroll: gpui::ScrollHandle,
    reveal: bool,
}

impl ServicesPage {
    pub fn new(host: Host, _: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Services::default(),
            shown: false,
            focus: cx.focus_handle(),
            list_scroll: gpui::ScrollHandle::new(),
            reveal: false,
        }
    }

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
                Err(e) if e.code == "unanswered" => {
                    (0, format!("{NO_ANSWER}: {}", e.message).into_bytes())
                }
                Err(e) => (0, e.message.into_bytes()),
            };
            let _ = this.update(cx, |s, cx| done(s, status, body, cx));
        })
        .detach();
    }

    fn read_all(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            self.read(node.id.as_str(), cx);
        }
        self.read_log(cx);
    }

    fn read(&mut self, node: &str, cx: &mut Context<Self>) {
        let state = self.state.nodes.entry(node.to_string()).or_default();
        if state.reading {
            return;
        }
        state.reading = true;
        let at = node.to_owned();
        self.ask(
            Request::get(NodeId(at.clone()), "/api/services"),
            move |s, status, body, cx| {
                s.state
                    .nodes
                    .entry(at)
                    .or_default()
                    .listed(status, &body, now_ms());
                if s.state.selected.is_none() {
                    s.state.selected = s.order(cx).into_iter().next();
                    s.read_log(cx);
                }
                cx.notify();
            },
            cx,
        );
    }

    /// Reads the chosen service's latest log lines.
    fn read_log(&mut self, cx: &mut Context<Self>) {
        let Some((node, id)) = self.state.selected.clone() else {
            return;
        };
        let n = self.state.nodes.entry(node.clone()).or_default();
        let log = n.logs.entry(id.clone()).or_default();
        if log.reading {
            return;
        }
        log.reading = true;
        self.ask(
            Request::get(
                NodeId(node.clone()),
                format!("/api/services/{id}/log?lines={}", LOG_LINES),
            ),
            move |s, status, body, cx| {
                s.state
                    .nodes
                    .entry(node)
                    .or_default()
                    .logs
                    .entry(id)
                    .or_default()
                    .read(status, &body);
                cx.notify();
            },
            cx,
        );
    }

    fn choose(&mut self, node: &str, id: &str, cx: &mut Context<Self>) {
        let key = (node.to_owned(), id.to_owned());
        if self.state.selected.as_ref() != Some(&key) {
            self.state.selected = Some(key);
            self.read_log(cx);
        }
        cx.notify();
    }

    fn act(&mut self, node: &str, id: &str, change: Change, cx: &mut Context<Self>) {
        let n = self.state.nodes.entry(node.to_owned()).or_default();
        if n.changing.contains_key(id) {
            return;
        }
        n.changing.insert(id.to_owned(), change);
        n.failures.remove(id);
        n.unsettled.remove(id);
        let (at, row) = (node.to_owned(), id.to_owned());
        let request = Request::new(
            "POST",
            NodeId(at.clone()),
            format!("/api/services/{id}/{}", change.verb()),
        )
        .within(ACTION_WAIT);
        self.ask(
            request,
            move |s, status, body, cx| {
                let n = s.state.nodes.entry(at.clone()).or_default();
                match n.answered(&row, change, status, &body) {
                    Ended::Failed(message) => s.host.report(Status::failure(message), cx),
                    Ended::Done => s.host.toast(&format!("{row} {}", change.past()), cx),
                    Ended::Follow => s.follow(at, row, 0, cx),
                    Ended::Elsewhere => {}
                }
                s.read_log(cx);
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }

    /// Every row, in the list's order: each node's, by group then name.
    fn order(&self, cx: &App) -> Vec<(String, String)> {
        self.host
            .nodes(cx)
            .iter()
            .flat_map(|node| {
                self.sorted(node.id.as_str())
                    .into_iter()
                    .map(|s| (node.id.to_string(), s.id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn sorted(&self, node: &str) -> Vec<&Service> {
        let mut list: Vec<&Service> = self
            .state
            .nodes
            .get(node)
            .and_then(|n| n.list.as_ref())
            .into_iter()
            .flatten()
            .collect();
        list.sort_by_key(|s| (Group::of(&s.state), s.name.to_lowercase()));
        list
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let step = match event.keystroke.key.as_str() {
            "down" => Some(Step::Down),
            "up" => Some(Step::Up),
            "home" => Some(Step::Home),
            "end" => Some(Step::End),
            _ => None,
        };
        if let Some(step) = step {
            let order = self.order(cx);
            let before = self.state.selected.clone();
            self.state.step(&order, step);
            if self.state.selected != before {
                self.read_log(cx);
                self.reveal = true;
            }
        } else if event.keystroke.key == "enter" {
            match self.state.enter() {
                Some(link) => cx.open_url(&link),
                None => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn follow(&mut self, node: String, id: String, read: usize, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(services_store::FOLLOW[read]))
                .await;
            let _ = this.update(cx, |s, cx| {
                let path = format!("/api/services/{id}");
                let at = node.clone();
                s.ask(
                    Request::get(NodeId(node), path),
                    move |s, status, body, cx| {
                        let last = read + 1 == services_store::FOLLOW.len();
                        let clock = parvi_app_kit::clock::hhmm(now_ms());
                        if s.state
                            .nodes
                            .entry(at.clone())
                            .or_default()
                            .followed(&id, status, &body, last, &clock)
                        {
                            s.follow(at, id, read + 1, cx);
                        } else {
                            s.read_log(cx);
                        }
                        cx.notify();
                    },
                    cx,
                );
            });
        })
        .detach();
    }
}

/// Log lines the detail shows: the latest, oldest first, as many as the
/// first screen holds.
const LOG_LINES: usize = 20;

impl Page for ServicesPage {
    const ID: &'static str = "services";
    const TITLE: &'static str = "Services";
    const KEY: Option<char> = Some('E');
    const AREA: Option<&'static str> = Some("host");
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("service")];

    fn focus(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.read_all(cx);
        }
    }

    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        let Ask::Reveal { id, node, .. } = ask else {
            return;
        };
        let node = node.map(|n| n.to_string()).or_else(|| {
            self.state
                .nodes
                .iter()
                .find(|(_, n)| n.row(&id).is_some())
                .map(|(n, _)| n.clone())
        });
        match node {
            Some(node) => {
                self.choose(&node, &id, cx);
            }
            None => self
                .host
                .report(Status::failure(format!("No service {id} is listed")), cx),
        }
    }

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
}

impl Render for ServicesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme(cx).clone();
        let nodes = self.host.nodes(cx);
        let several = nodes.len() > 1;
        let (mut rows, mut chosen_at) = (Vec::new(), None);
        for node in &nodes {
            self.node_rows(&t, node, several, &mut rows, &mut chosen_at, cx);
        }
        // The row the keys reached stays in view.
        if std::mem::take(&mut self.reveal)
            && let Some(at) = chosen_at
        {
            self.list_scroll.scroll_to_item(at);
        }
        let rows = div()
            .id("services-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .flex()
            .flex_col()
            .children(rows);
        let head = pane::list_head(&t, None).child(
            pane::act(&t, "services-refresh", "Refresh")
                .debug_selector(|| "services-refresh".into())
                .on_click(cx.listener(|s, _, _, cx| s.read_all(cx))),
        );
        let list = pane::list_frame(&t, head, rows);
        let chosen = self
            .state
            .selected
            .clone()
            .and_then(|(n, id)| Some((n.clone(), self.state.service(&n, &id)?.clone())));
        let detail = match chosen {
            Some((node, service)) => self.detail(&t, &node, &service, cx),
            None => {
                let reading = self.state.nodes.values().any(|n| n.list.is_none());
                let words = if nodes.is_empty() {
                    "Waiting for a node…"
                } else if reading {
                    "Reading services…"
                } else {
                    "The catalog lists no services."
                };
                pane::detail(
                    &t,
                    "services-detail",
                    pane::head(&t),
                    pane::empty(&t, words, None).p_0(),
                )
            }
        };
        div()
            .id("services")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| s.key(e, cx)))
            .child(pane::page(&t, list, detail).text_color(t.text))
    }
}

impl ServicesPage {
    /// One node's lines join `out`, each a child of the list so the keys
    /// can scroll to it; `chosen_at` is where the chosen row stands.
    fn node_rows(
        &self,
        t: &Theme,
        node: &Node,
        several: bool,
        out: &mut Vec<gpui::AnyElement>,
        chosen_at: &mut Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let id = node.id.as_str();
        let n = self.state.nodes.get(id);
        if several {
            out.push(
                div()
                    .flex_none()
                    .px(px(size::ROW_PAD))
                    .pt(px(size::LABEL_TOP))
                    .child(kit::device_label(t, &node.device, &node.environment))
                    .into_any_element(),
            );
        }
        if let Some(e) = n.and_then(|n| n.error.as_ref()) {
            out.push(
                div()
                    .flex_none()
                    .px(px(size::ROW_PAD))
                    .py(px(size::LABEL_BOTTOM))
                    .child(pane::meta(t, format!("Not read: {e}"), Some(t.error)))
                    .into_any_element(),
            );
        }
        let Some(n) = n else { return };
        let now = now_ms();
        let mut group = None;
        for service in self.sorted(id) {
            let g = Group::of(&service.state);
            if group != Some(g) {
                group = Some(g);
                out.push(pane::label(t, g.label()).into_any_element());
            }
            let chosen = self.state.selected.as_ref() == Some(&(id.to_owned(), service.id.clone()));
            let changing = n.changing.get(&service.id).map(|c| c.word());
            let (meta, colour) = match (changing, g) {
                (Some(w), _) => (w.to_string(), None),
                (None, Group::Failed) => (
                    services_store::state_word(service).to_string(),
                    Some(t.error),
                ),
                (None, Group::Running) => match service.since() {
                    _ if service.state != "running" => {
                        (services_store::word(&service.state).to_string(), None)
                    }
                    Some(since) if now.saturating_sub(since) < LATELY => {
                        (format!("up {}", services_store::ago(since, now)), None)
                    }
                    _ => (String::new(), None),
                },
                (None, Group::Stopped) => (String::new(), None),
            };
            let shown = n.shown(service).to_string();
            let state_selector = format!("svc-state-{id}-{}-{shown}", service.id);
            let selector = format!("svc-row-{id}-{}", service.id);
            let (at, sid) = (id.to_owned(), service.id.clone());
            if chosen {
                *chosen_at = Some(out.len());
            }
            out.push(
                pane::row(t, SharedString::from(selector.clone()), chosen)
                    .debug_selector(move || selector)
                    .child(glyph(t, &service.state))
                    .child(pane::cell(service.name.clone()))
                    .child(
                        div()
                            .debug_selector(move || state_selector)
                            .child(pane::meta(t, meta, colour)),
                    )
                    .on_click(cx.listener(move |s, _, window, cx| {
                        window.focus(&s.focus, cx);
                        s.choose(&at, &sid, cx);
                    }))
                    .into_any_element(),
            );
        }
        if n.list.as_ref().is_some_and(Vec::is_empty) {
            out.push(
                div()
                    .flex_none()
                    .px(px(size::ROW_PAD))
                    .child(pane::meta(t, "The catalog lists no services.", None))
                    .into_any_element(),
            );
        }
    }

    fn detail(&self, t: &Theme, node: &str, s: &Service, cx: &mut Context<Self>) -> Div {
        let n = self.state.nodes.get(node).expect("listed");
        let now = now_ms();
        let mut head = pane::head(t)
            .child(pane::title(t, s.name.clone()))
            .child(pane::send(
                t,
                "svc-send",
                &self.host,
                services_store::reference(s, now),
            ));
        if services_store::opens(s) {
            let link = s.link.clone();
            head = head.child(
                pane::act(t, "svc-open", "Open")
                    .debug_selector(|| "svc-open".into())
                    .tooltip(kit::tip(s.link.clone()))
                    .on_click(move |_, _, cx| cx.open_url(&link)),
            );
        }
        // One action per state in the head; what else the state allows
        // waits under More.
        if s.controllable && !n.changing.contains_key(&s.id) {
            let mut offered = services_store::actions(&s.state, services_store::looping(s))
                .iter()
                .copied();
            if let Some(change) = offered.next() {
                let selector = format!("svc-{}-{node}-{}", change.verb(), s.id);
                let (at, id) = (node.to_owned(), s.id.clone());
                head = head.child(
                    pane::act(t, SharedString::from(selector.clone()), change.label())
                        .debug_selector(move || selector)
                        .on_click(cx.listener(move |p, _, _, cx| p.act(&at, &id, change, cx))),
                );
            }
            let more: Vec<Change> = offered.collect();
            if !more.is_empty() {
                let (at, id) = (node.to_owned(), s.id.clone());
                head = head.child(
                    pane::act(t, "svc-more", "…")
                        .debug_selector(|| "svc-more".into())
                        .on_click(cx.listener(move |p, e: &gpui::ClickEvent, _, cx| {
                            let me = cx.weak_entity();
                            let rows = more
                                .iter()
                                .map(|&change| {
                                    let (me, at, id) = (me.clone(), at.clone(), id.clone());
                                    parvi_app_kit::menu::MenuRow::new(
                                        change.label(),
                                        move |_, cx| {
                                            let _ =
                                                me.update(cx, |p, cx| p.act(&at, &id, change, cx));
                                        },
                                    )
                                })
                                .collect();
                            p.host.menu(rows, e.position(), cx);
                        })),
                );
            }
        }

        let group = Group::of(&s.state);
        let mut state = n.shown(s).to_string();
        if let Some(since) = s.since() {
            state.push_str(&format!(" for {}", services_store::ago(since, now)));
        }
        if s.state == "failed" && services_store::looping(s) {
            state.push_str(": systemd keeps restarting it");
            if s.restarts() > 0 {
                state.push_str(&format!(", {} so far", times(s.restarts())));
            }
        } else if s.restarts() > 0 {
            state.push_str(&format!(", restarted {} by systemd", times(s.restarts())));
        }
        let state_selector = format!("svc-detail-state-{state}");
        let mut lines = div().flex().flex_col().child(pane::line(
            t,
            "State",
            div()
                .debug_selector(move || state_selector)
                .when(group == Group::Failed, |d| d.text_color(t.error))
                .child(state),
        ));
        if let Some(failure) = n.failures.get(&s.id) {
            let selector = format!("svc-failure-{node}-{}", s.id);
            lines = lines.child(pane::line(
                t,
                "Last action",
                div()
                    .debug_selector(move || selector)
                    .text_color(t.error)
                    .child(failure.clone()),
            ));
        } else if let Some(note) = n.unsettled.get(&s.id) {
            lines = lines.child(pane::line(
                t,
                "Last action",
                div().text_color(t.text2).child(note.clone()),
            ));
        }
        if !s.link.is_empty() {
            let (open, copy) = (s.link.clone(), s.link.clone());
            lines = lines.child(pane::line(
                t,
                "Address",
                div()
                    .flex()
                    .items_center()
                    .gap(px(size::HEAD_GAP))
                    .child(
                        div()
                            .id("svc-address")
                            .debug_selector(|| "svc-address".into())
                            .min_w_0()
                            .child(s.link.clone())
                            // A link while something may answer there.
                            .map(|d| match services_store::opens(s) {
                                true => d
                                    .text_color(t.accent)
                                    .cursor_pointer()
                                    .hover(|d| d.underline())
                                    .on_click(move |_, _, cx| cx.open_url(&open)),
                                false => d.text_color(t.text2),
                            }),
                    )
                    .child(
                        pane::act(t, "svc-copy", "Copy")
                            .debug_selector(|| "svc-copy".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                p.host.toast("Address copied", cx);
                            })),
                    ),
            ));
        }
        if !s.description.is_empty() {
            lines = lines.child(pane::line(
                t,
                "What",
                div().text_color(t.text2).child(s.description.clone()),
            ));
        }
        if !s.components.is_empty() {
            let mut units = div().flex().flex_col();
            for c in &s.components {
                let state = if c.sub.is_empty() || c.sub == c.state {
                    c.state.clone()
                } else {
                    format!("{} ({})", c.state, c.sub)
                };
                units = units.child(
                    div()
                        .min_h(px(size::ROW))
                        .flex()
                        .items_center()
                        .gap(px(size::HEAD_GAP))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(t.text2)
                                .child(c.unit.clone()),
                        )
                        .child(pane::meta(
                            t,
                            state,
                            (c.state == "failed").then_some(t.error),
                        )),
                );
            }
            let name = if s.components.len() == 1 {
                "Unit"
            } else {
                "Units"
            };
            lines = lines.child(pane::line(t, name, units));
        }
        if !s.controllable {
            lines = lines.child(pane::line(
                t,
                "Control",
                div()
                    .text_color(t.dim)
                    .child("Read-only: the catalog does not let Parvi start or stop it"),
            ));
        }
        let body = div()
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP))
            .child(lines)
            .child(self.log(
                t,
                n.logs.get(&s.id),
                s.since().filter(|_| s.state == "running"),
            ));
        pane::detail(t, "services-detail", head, body)
    }

    /// The latest log lines. A running service's lines from before this
    /// run (it began at `running_since`) stand under their own label; a
    /// line from another day says its day.
    fn log(&self, t: &Theme, log: Option<&services_store::Log>, running_since: Option<u64>) -> Div {
        let mut section = pane::section(t, "Log");
        let Some(log) = log else {
            return section.child(div().text_color(t.dim).child("Reading the log…"));
        };
        for e in &log.errors {
            section = section.child(div().text_color(t.dim).child(e.clone()));
        }
        if log.lines.is_empty() {
            let words = if log.reading {
                "Reading the log…"
            } else {
                "Nothing logged."
            };
            if log.errors.is_empty() || log.reading {
                section = section.child(div().text_color(t.dim).child(words));
            }
            return section;
        }
        let now = now_ms();
        let stamps: Vec<String> = log
            .lines
            .iter()
            .map(|l| parvi_app_kit::clock::stamp(l.at, now))
            .collect();
        let time_col = match stamps.iter().any(|s| s.len() > 5) {
            true => tokens::DAY_TIME_COL,
            false => tokens::TIME_COL,
        };
        let earlier = services_store::earlier(&log.lines, running_since);
        if earlier > 0 {
            section = section.child(
                pane::meta(t, "Earlier runs", None).debug_selector(|| "svc-log-earlier".into()),
            );
        }
        for (at, (line, stamp)) in log.lines.iter().zip(stamps).enumerate() {
            if earlier > 0 && at == earlier {
                section = section.child(
                    pane::meta(t, "This run", None)
                        .pt(px(size::LABEL_TOP))
                        .debug_selector(|| "svc-log-this-run".into()),
                );
            }
            let selector = format!("svc-log-{}", line.text);
            section = section.child(
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .items_start()
                    .gap(px(size::HEAD_GAP))
                    .text_size(px(size::SMALL))
                    .child(
                        div()
                            .flex_none()
                            .w(px(time_col))
                            .text_color(t.dim)
                            .child(stamp),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if line.error { t.error } else { t.text2 })
                            .child(line.text.clone()),
                    ),
            );
        }
        section
    }
}

fn glyph(t: &Theme, state: &str) -> Div {
    let dot = div()
        .size(px(tokens::GLYPH))
        .flex_none()
        .rounded_full()
        .overflow_hidden();
    match state {
        "running" => dot.bg(t.accent),
        "starting" => dot.bg(theme::alpha(t.accent, tokens::FADED)),
        "partial" => dot
            .border_1()
            .border_color(t.error)
            .child(div().w(px(tokens::GLYPH / 2.)).h_full().bg(t.error)),
        "failed" => dot.bg(t.error),
        "unknown" => dot.border_1().border_dashed().border_color(t.error),
        "stopping" => dot.border_1().border_color(t.accent),
        _ => dot.border_1().border_color(t.dim),
    }
}

/// A count of times in words: "once", "3 times".
fn times(n: u32) -> String {
    if n == 1 {
        "once".into()
    } else {
        format!("{n} times")
    }
}
