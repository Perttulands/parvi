//! The Host page: is this machine all right now. The list has each node's
//! verdict and every figure now with its last hour; the detail has what is
//! wrong and what is heavy, or one figure's day.

use crate::chart::{self, Line};
use crate::host_store::{
    self as store, DAY, Figure, HOUR, Hosts, State, Trouble, figures, peak, percent,
};
use crate::tokens;
use gpui::{Context, Div, Entity, Hsla, SharedString, Window, div, prelude::*, px};
use parvi_app_kit::{
    Ask, Host, Page,
    host::{Node, Request, Status},
    hub::now_ms,
    link::client::LinkState,
    theme::{self, Theme, size},
    widgets::{
        kit,
        pane::{self, Col},
    },
};
use parvi_core::NodeId;
use parvi_host_contract::{HOST, HOST_EVENT, HOST_STATE, MINUTE, Service, ServiceList};
use parvi_terminals_contract::SessionList;
use std::collections::HashMap;

pub fn hhmm(ms: u64) -> String {
    parvi_app_kit::clock::hhmm(ms)
}

/// What the detail shows for a node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Overview,
    Figure(Figure),
}

/// What is heavy: the program, where it runs, its CPU and memory.
const HEAVY: [Col; 4] = [
    Col::fill("Program"),
    Col::fill("Where"),
    Col::fixed("CPU", tokens::FIGURE_COL),
    Col::fixed("Memory", tokens::FIGURE_COL),
];
const EVENTS: [Col; 2] = [Col::fixed("At", tokens::TIME_COL), Col::fill("What")];
/// Events the overview shows: today's, newest first.
const EVENTS_SHOWN: usize = 5;

pub struct ReadingsPage {
    pub(crate) host: Host,
    hosts: Hosts,
    chosen: Option<(String, Choice)>,
    /// The moment under the mouse on a figure's chart.
    pub(crate) hover: Option<u64>,
    shown: bool,
    sessions: Option<Entity<SessionList>>,
    /// Each node's services that need the operator, read when the page
    /// comes forward.
    failed: HashMap<String, Vec<Service>>,
}

impl ReadingsPage {
    pub fn new(host: Host, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let sessions = host.model::<SessionList>(cx);
        if let Some(s) = &sessions {
            cx.observe(s, |page, _, cx| {
                if page.shown {
                    cx.notify()
                }
            })
            .detach();
        }
        Self {
            host,
            hosts: Hosts::default(),
            chosen: None,
            hover: None,
            shown: false,
            sessions,
            failed: HashMap::new(),
        }
    }

    fn choose(&mut self, node: &str, choice: Choice, cx: &mut Context<Self>) {
        self.chosen = Some((node.to_owned(), choice));
        self.hover = None;
        cx.notify();
    }

    /// Reads each node's services for the ones that failed. Once, as the
    /// page comes forward: Services is where they are watched.
    fn read_services(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            self.read_failed(node.id, cx);
        }
    }

    fn read_failed(&mut self, node: NodeId, cx: &mut Context<Self>) {
        let asked = self
            .host
            .request(Request::get(node.clone(), "/api/services"), cx);
        cx.spawn(async move |this, cx| {
            let Ok(answer) = asked.await else { return };
            let Ok(list) = serde_json::from_slice::<ServiceList>(&answer.body) else {
                return;
            };
            let failed = list
                .services
                .into_iter()
                .filter(|s| matches!(s.state.as_str(), "failed" | "partial"))
                .collect();
            let _ = this.update(cx, |page, cx| {
                page.failed.insert(node.to_string(), failed);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Page for ReadingsPage {
    const ID: &'static str = "host";
    const TITLE: &'static str = "Host";
    const KEY: Option<char> = Some('H');
    const AREA: Option<&'static str> = Some("host");
    const KINDS: parvi_core::frame::Kinds = &[HOST_STATE..=HOST_EVENT];
    const CHANNELS: &'static [u8] = &[HOST];

    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.read_services(cx);
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
            self.read_failed(node.clone(), cx);
        }
    }

    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(event) = self.hosts.take(node.as_str(), kind, body) else {
            return;
        };
        if let Some(e) = event
            && self.shown
        {
            let status = if e.failure {
                Status::failure(e.text)
            } else {
                Status::new(e.text)
            };
            self.host.report(status, cx);
        }
        if self.shown {
            cx.notify();
        }
    }
}

/// A figure's key in selectors: `cpu`, `disk-C:`.
fn key(figure: Figure, s: &State) -> String {
    match figure {
        Figure::Cpu => "cpu".into(),
        Figure::Memory => "memory".into(),
        Figure::Gpu => "gpu".into(),
        Figure::Vram => "vram".into(),
        Figure::Disk(i) => {
            let name = s
                .latest
                .as_ref()
                .and_then(|r| r.disks.get(i))
                .map_or("", |d| d.name.as_str());
            format!("disk-{name}")
        }
    }
}

fn local_offset() -> i64 {
    time::UtcOffset::current_local_offset()
        .unwrap_or(time::UtcOffset::UTC)
        .whole_seconds()
        .into()
}

impl Render for ReadingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme(cx).clone();
        let nodes = self.host.nodes(cx);
        let chosen = self
            .chosen
            .clone()
            .filter(|(n, _)| nodes.iter().any(|x| x.id.as_str() == n))
            .or_else(|| nodes.first().map(|n| (n.id.to_string(), Choice::Overview)));
        let mut rows = div().flex().flex_col();
        for node in &nodes {
            rows = rows.child(self.node_rows(&t, node, nodes.len() > 1, chosen.as_ref(), cx));
        }
        let list = pane::list(&t, "host-list", pane::list_head(&t, None), rows);
        let node = chosen
            .as_ref()
            .and_then(|(n, _)| nodes.iter().find(|x| x.id.as_str() == n));
        let detail = match (node, chosen.as_ref().map(|c| c.1)) {
            (Some(node), Some(choice)) => self.detail(&t, node, choice, cx),
            _ => pane::detail(
                &t,
                "host-detail",
                pane::head(&t),
                pane::empty(&t, "Waiting for a node…", None).p_0(),
            ),
        };
        pane::page(&t, list, detail).text_color(t.text)
    }
}

impl ReadingsPage {
    fn troubles(&self, node: &str, s: &State) -> (Vec<Trouble>, &[Service]) {
        let failed = self.failed.get(node).map_or(&[][..], Vec::as_slice);
        (store::troubles(s, now_ms()), failed)
    }

    fn node_rows(
        &self,
        t: &Theme,
        node: &Node,
        several: bool,
        chosen: Option<&(String, Choice)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = node.id.as_str();
        let mut out = div().flex().flex_col();
        if several {
            out = out.child(
                div()
                    .px(px(size::ROW_PAD))
                    .pt(px(size::LABEL_TOP))
                    .pb(px(size::LABEL_BOTTOM))
                    .child(kit::device_label(t, &node.device, &node.environment)),
            );
        }
        let is = |c: Choice| chosen.is_some_and(|(n, x)| n == id && *x == c);
        let state = self.hosts.nodes.get(id).map(|n| &n.state);
        let (verdict, colour) = match state {
            Some(s) if s.latest.is_some() => {
                let (troubles, failed) = self.troubles(id, s);
                match troubles.len() + failed.len() {
                    0 => ("all right".to_string(), None),
                    n => (format!("{n} to look at"), Some(t.error)),
                }
            }
            _ => ("waiting".to_string(), None),
        };
        out = out.child(
            self.row(t, id, "overview", "Overview", Choice::Overview, is, cx)
                .child(meta(
                    t,
                    format!("host-verdict-{id}-{verdict}"),
                    verdict,
                    colour,
                )),
        );
        let Some(s) = state else { return out };
        let latest = s.latest.as_ref();
        let troubles = store::troubles(s, now_ms());
        let end = latest.map_or_else(now_ms, |r| r.at);
        for figure in figures(latest) {
            let k = key(figure, s);
            let warn = store::warns(&troubles, figure);
            let now = percent(latest.and_then(|r| figure.now(r)));
            let points = store::line(&s.minutes, figure, end.saturating_sub(HOUR));
            let spark = Line {
                top: store::ceiling(figure, &points),
                runs: store::runs(&points),
                start: end.saturating_sub(HOUR),
                end,
                ticks: vec![],
                hover: None,
                ink: if warn { t.error } else { t.accent },
                grid: t.divider,
            };
            out = out.child(
                self.row(
                    t,
                    id,
                    &k,
                    &figure.name(latest),
                    Choice::Figure(figure),
                    is,
                    cx,
                )
                .child(
                    div()
                        .w(px(tokens::SPARK_W))
                        .h(px(tokens::SPARK_H))
                        .flex_none()
                        .child(chart::chart(spark, None)),
                )
                .child(
                    meta(
                        t,
                        format!("host-now-{id}-{k}-{now}"),
                        now,
                        warn.then_some(t.error),
                    )
                    .w(px(tokens::FIGURE_COL / 2.))
                    .flex()
                    .justify_end(),
                ),
            );
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        t: &Theme,
        node: &str,
        key: &str,
        name: &str,
        choice: Choice,
        is: impl Fn(Choice) -> bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let selector = format!("host-row-{node}-{key}");
        let at = node.to_owned();
        pane::row(t, SharedString::from(selector.clone()), is(choice))
            .debug_selector(move || selector)
            .child(pane::cell(name.to_owned()))
            .on_click(cx.listener(move |page, _, _, cx| page.choose(&at, choice, cx)))
    }

    fn detail(&self, t: &Theme, node: &Node, choice: Choice, cx: &mut Context<Self>) -> Div {
        let id = node.id.as_str();
        let state = self.hosts.nodes.get(id).map(|n| &n.state);
        let Some(s) = state.filter(|s| s.latest.is_some()) else {
            return pane::detail(
                t,
                "host-detail",
                pane::head(t).child(pane::title(t, title(node))),
                pane::empty(t, "Waiting for the first reading…", None).p_0(),
            );
        };
        match choice {
            Choice::Overview => self.overview(t, node, s, cx),
            Choice::Figure(f) => self.figure(t, node, s, f, cx),
        }
    }

    fn overview(&self, t: &Theme, node: &Node, s: &State, cx: &mut Context<Self>) -> Div {
        let id = node.id.as_str();
        let now = now_ms();
        let reference = store::reference(&node.device, &node.environment, s, now);
        let head = pane::head(t)
            .child(pane::title(t, title(node)))
            .child(pane::send(t, "host-send", &self.host, reference));
        let r = s.latest.as_ref().expect("a reading");
        let (troubles, failed) = self.troubles(id, s);
        let up = r.uptime.map(|u| format!("Up {}", store::uptime(u)));
        let mut verdict = div().flex().flex_col();
        if troubles.is_empty() && failed.is_empty() {
            verdict = verdict.child(
                div()
                    .debug_selector(|| "host-all-right".into())
                    .flex()
                    .gap(px(size::HEAD_GAP))
                    .child("All right")
                    .when_some(up.clone(), |d, up| {
                        d.child(div().text_color(t.dim).child(up))
                    }),
            );
        } else {
            for trouble in troubles {
                let selector = format!("host-trouble-{}", trouble.text);
                let at = id.to_owned();
                let figure = trouble.figure;
                verdict = verdict.child(
                    pane::row(t, SharedString::from(selector.clone()), false)
                        .mx_0()
                        .px_0()
                        .debug_selector(move || selector)
                        .text_color(t.error)
                        .child(pane::cell(trouble.text))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if let Some(f) = figure {
                                page.choose(&at, Choice::Figure(f), cx)
                            }
                        })),
                );
            }
            for service in failed {
                let text = format!("Service {} {}", service.name, service.state);
                let selector = format!("host-trouble-{text}");
                let ask = Ask::Reveal {
                    what: "service".into(),
                    id: service.id.clone(),
                    node: Some(node.id.clone()),
                    part: None,
                    beside: false,
                };
                verdict = verdict.child(
                    pane::row(t, SharedString::from(selector.clone()), false)
                        .mx_0()
                        .px_0()
                        .debug_selector(move || selector)
                        .text_color(t.error)
                        .child(pane::cell(text))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if let Err(e) = page.host.ask(ask.clone(), cx) {
                                page.host.report(Status::failure(e.message), cx);
                            }
                        })),
                );
            }
            if let Some(up) = up {
                verdict =
                    verdict.child(div().pt(px(size::LABEL_BOTTOM)).text_color(t.dim).child(up));
            }
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP))
            .child(verdict)
            .child(self.heavy(t, id, s, Figure::Memory, "Heavy now", cx));
        let today: Vec<_> = s
            .events
            .iter()
            .rev()
            .filter(|e| e.at + DAY > now)
            .take(EVENTS_SHOWN)
            .collect();
        if !today.is_empty() {
            let rows = today
                .into_iter()
                .map(|e| {
                    let selector = format!("host-event-{}", e.text);
                    pane::table_row(
                        t,
                        &EVENTS,
                        vec![
                            div().text_color(t.dim).child(hhmm(e.at)).into_any_element(),
                            div()
                                .debug_selector(move || selector)
                                .when(e.failure, |d| d.text_color(t.error))
                                .child(e.text.clone())
                                .into_any_element(),
                        ],
                    )
                    .into_any_element()
                })
                .collect();
            body = body.child(pane::section(t, "Today").child(pane::table(t, &EVENTS, rows)));
        }
        pane::detail(t, "host-detail", head, body)
    }

    /// What is heavy now, heaviest by `figure` first.
    fn heavy(
        &self,
        t: &Theme,
        node: &str,
        s: &State,
        figure: Figure,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let r = s.latest.as_ref().expect("a reading");
        let home = s.facts.user.as_deref().map(|u| match u {
            "root" => "/root".to_string(),
            u => format!("/home/{u}"),
        });
        let sessions: Vec<(String, String)> = self
            .sessions
            .as_ref()
            .map(|m| {
                m.read(cx)
                    .sessions
                    .iter()
                    .filter(|x| x.node == node)
                    .filter_map(|x| Some((x.name.clone(), x.cwd.clone()?)))
                    .collect()
            })
            .unwrap_or_default();
        let sessions: Vec<(&str, &str)> = sessions
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_str()))
            .collect();
        let section = pane::section(t, label.to_owned());
        if r.heavy.is_empty() {
            return section.child(div().text_color(t.dim).child("Reading what runs…"));
        }
        let rows = store::sorted(&r.heavy, figure)
            .into_iter()
            .map(|h| {
                let selector = format!("host-heavy-{}", store::program(h));
                let place = store::place(h.cwd.as_deref(), &sessions, home.as_deref());
                pane::table_row(
                    t,
                    &HEAVY,
                    vec![
                        pane::cell(store::program(h))
                            .debug_selector(move || selector)
                            .into_any_element(),
                        pane::cell(place).text_color(t.text2).into_any_element(),
                        div()
                            .child(h.cpu.map_or_else(String::new, |c| percent(Some(c))))
                            .into_any_element(),
                        div().child(store::bytes(h.memory)).into_any_element(),
                    ],
                )
                .into_any_element()
            })
            .collect();
        section.child(pane::table(t, &HEAVY, rows))
    }

    fn figure(
        &self,
        t: &Theme,
        node: &Node,
        s: &State,
        figure: Figure,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = node.id.as_str();
        let r = s.latest.as_ref().expect("a reading");
        let end = r.at;
        let oldest = s.minutes.first().map_or(end, |p| p.at);
        let start = oldest
            .max(end.saturating_sub(DAY))
            .min(end.saturating_sub(HOUR));
        let points = store::line(&s.minutes, figure, start);
        let reference = store::figure_reference(&node.device, &node.environment, s, figure, start);
        let name = figure.name(Some(r));
        let head = pane::head(t)
            .child(pane::title(t, format!("{name} · {}", title(node))))
            .child(pane::send(t, "host-send", &self.host, reference));
        let troubles = store::troubles(s, now_ms());
        let warn = store::warns(&troubles, figure);
        let mut lines = div().flex().flex_col();
        let line = |name: &str, value: String, colour: Option<Hsla>| {
            let selector = format!("host-line-{name}-{value}");
            pane::line(
                t,
                name.to_owned(),
                div()
                    .debug_selector(move || selector)
                    .when_some(colour, |d, c| d.text_color(c))
                    .child(value),
            )
        };
        lines = lines.child(line(
            "Now",
            store::reading_words(figure, r),
            warn.then_some(t.error),
        ));
        if let Some((at, v)) = peak(&points) {
            lines = lines.child(line(
                "Highest",
                format!("{} at {}", percent(Some(v)), hhmm(at)),
                None,
            ));
        }
        match figure {
            Figure::Memory => {
                if let Some(swap) = r.swap.filter(|u| u.total > 0) {
                    let mut words = store::of(swap);
                    if let Some(bps) = r.swap_activity_bps.filter(|b| *b >= 1.) {
                        words.push_str(&format!(", moving {}/s", store::bytes(bps as u64)));
                    }
                    lines = lines.child(line("Swap", words, None));
                }
            }
            Figure::Gpu | Figure::Vram => {
                if let Some(g) = r.gpus.first() {
                    lines = lines.child(line("Card", g.name.clone(), None));
                    if let Some(c) = g.temp {
                        lines = lines.child(line("Temperature", format!("{c:.0} °C"), None));
                    }
                } else if let Some(why) = &r.gpu_reason {
                    lines = lines.child(line("Not read", why.clone(), Some(t.error)));
                }
            }
            Figure::Disk(i) => {
                if let Some(d) = r.disks.get(i) {
                    let free = d.total.saturating_sub(d.used);
                    lines = lines.child(line("Free", store::bytes(free), warn.then_some(t.error)));
                    if let Some((grew, at)) = store::grew(s, i, start) {
                        lines = lines.child(line(
                            &format!("Since {}", hhmm(at)),
                            store::change(grew, d.total),
                            None,
                        ));
                    }
                    lines = lines.child(line("Mount", d.mount.clone(), None));
                }
            }
            Figure::Cpu => {
                if s.facts.threads > 0 {
                    lines = lines.child(line("Threads", s.facts.threads.to_string(), None));
                }
            }
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP))
            .child(lines)
            .child(self.big_chart(t, figure, &points, start, end, warn, cx));
        if matches!(figure, Figure::Cpu | Figure::Memory) {
            body = body.child(self.heavy(t, id, s, figure, "Heaviest now", cx));
        }
        pane::detail(t, "host-detail", head, body)
    }

    #[allow(clippy::too_many_arguments)]
    fn big_chart(
        &self,
        t: &Theme,
        figure: Figure,
        points: &[(u64, f32)],
        start: u64,
        end: u64,
        warn: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let top = store::ceiling(figure, points);
        let ticks = store::ticks(start, end, local_offset());
        let hours = (end - start + HOUR / 2) / HOUR;
        let reading = self.hover.map(|at| {
            let near = points
                .iter()
                .min_by_key(|(p, _)| p.abs_diff(at))
                .filter(|(p, _)| p.abs_diff(at) <= 2 * MINUTE);
            match near {
                Some((p, v)) => format!("{} at {}", percent(Some(*v)), hhmm(*p)),
                None => format!("nothing read at {}", hhmm(at)),
            }
        });
        let label = div()
            .flex()
            .child(
                pane::label(t, format!("Last {hours} h, up to {}", percent(Some(top))))
                    .px_0()
                    .pt_0()
                    .flex_1(),
            )
            .when_some(reading, |d, r| {
                d.child(
                    pane::label(t, r)
                        .px_0()
                        .pt_0()
                        .text_color(t.accent)
                        .debug_selector(|| "host-hover".into()),
                )
            });
        let line = Line {
            runs: store::runs(points),
            top,
            start,
            end,
            ticks: ticks.clone(),
            hover: self.hover,
            ink: if warn { t.error } else { t.accent },
            grid: t.divider,
        };
        let width = end.saturating_sub(start).max(1) as f32;
        let mut axis = div().relative().w_full().h(px(tokens::AXIS_H));
        for at in ticks {
            let share = at.saturating_sub(start) as f32 / width;
            axis = axis.child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::relative(share))
                    .ml(px(-tokens::TICK_W / 2.))
                    .w(px(tokens::TICK_W))
                    .flex()
                    .justify_center()
                    .text_size(px(size::SMALL))
                    .text_color(t.dim)
                    .child(hhmm(at)),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(label)
            .child(
                div()
                    .debug_selector(|| "host-chart".into())
                    .w_full()
                    .h(px(tokens::CHART_H))
                    .child(chart::chart(line, Some(cx.entity().downgrade()))),
            )
            .child(axis)
    }
}

fn title(node: &Node) -> String {
    if node.environment.is_empty() {
        node.device.clone()
    } else {
        format!("{} · {}", node.device, node.environment)
    }
}

fn meta(t: &Theme, selector: String, text: String, colour: Option<Hsla>) -> Div {
    pane::meta(t, text, colour).debug_selector(move || selector)
}
