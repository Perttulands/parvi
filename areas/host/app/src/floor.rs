//! One environment's floor in its device's room: the environment as its
//! material, the facts line, the storage meter, the time axis, CHROTE's
//! instrument rows and recent events.

use crate::chart::{self, Line};
use crate::host_store::{self as host, Metric, StateExt};
use crate::readings::{HostState, ReadingsPage, Span, hhmm, is_vm};
use crate::tokens as board;
use gpui::{AnyElement, Context, Div, FontWeight, Hsla, div, prelude::*, px, relative};
use parvi_app_kit::host::Node;
use parvi_app_kit::theme::{Theme, size};
use parvi_app_kit::widgets::kit;
use parvi_host_contract::State;

/// CHROTE's warn lines: CPU at 85%, memory at 90%, a disk at 85%, load at
/// one task per logical CPU.
const WARN_CPU: f32 = 85.;
const WARN_MEMORY: f32 = 90.;
const WARN_DISK: f32 = 85.;
const WARN_LOAD: f32 = 100.;

/// What a metric row shows besides its chart.
struct Row {
    id: String,
    label: &'static str,
    metric: Metric,
    /// The big reading now, in percent.
    now: Option<f32>,
    value: String,
    meta: String,
    warn: bool,
    /// Whether the row has a chart: Windows keeps no load to draw.
    charted: bool,
}

impl ReadingsPage {
    pub(super) fn render_floor(
        &self,
        t: &Theme,
        n: &Node,
        state: HostState,
        wall: bool,
        span: &Span,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(board::SMALL_GAP))
            .child(kit::device_label(t, &n.device, &n.environment));
        if let Some(latest) = state.and_then(|s| s.latest.as_ref()) {
            let reference = host::reference(&n.device, &n.environment, latest);
            head = head.child(div().flex_1()).child(
                kit::button(
                    t,
                    gpui::SharedString::from(format!("host-send-{}", n.id)),
                    "Send",
                    kit::Kind::Quiet,
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    let _ = page.host.ask(
                        parvi_app_kit::Ask::Send {
                            reference: reference.clone(),
                            target: None,
                        },
                        cx,
                    );
                })),
            );
        }
        let mut floor = div()
            .flex_1()
            .min_w(px(board::FLOOR_MIN))
            .p(px(board::PAD))
            .flex()
            .flex_col()
            .gap(px(board::GAP))
            .when(wall, |d| {
                d.border_l_1().border_dashed().border_color(t.divider)
            })
            .child(head);
        match state {
            None => {
                floor = floor.child(kit::note(
                    t,
                    "Waiting for this environment's first reading…",
                    Some(t.dim),
                ))
            }
            Some(s) => {
                floor = floor
                    .child(facts_line(t, s, &n.environment))
                    .child(storage(t, s))
                    .child(axis(t, span));
                for row in rows(n.id.as_str(), s, &n.environment) {
                    floor = floor.child(self.render_metric(t, &row, s, span, cx));
                }
            }
        }
        floor
            .when_some(state, |f, s| f.child(floor_events(t, s)))
            .into_any_element()
    }

    fn render_metric(
        &self,
        t: &Theme,
        row: &Row,
        s: &State,
        span: &Span,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let window = s.since(span.start);
        let values = || window.clone().filter_map(|p| row.metric.of(p));
        let top = host::ceiling(values());
        let stats = host::stats(values());
        let hover = self.hover.as_ref().map(|(_, at)| *at);
        // Scrubbing: every row reads the moment under the crosshair.
        let reading = match hover {
            Some(at) if row.charted => {
                host::nearest(window.clone(), at).and_then(|p| row.metric.of(p))
            }
            _ => row.now,
        };
        let tone = if row.warn { t.error } else { t.text };
        let label = div()
            .w(px(board::METRIC_LABEL))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(board::HAIRLINE))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(board::SMALL_GAP))
                    .child(kit::label(t, row.label))
                    .child(
                        div()
                            .text_size(px(board::READING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if hover.is_some() { t.accent } else { tone })
                            .child(host::percent(reading)),
                    ),
            )
            .child(wrapping(t, &row.value, t.text2))
            .when(!row.meta.is_empty(), |d| {
                d.child(wrapping(t, &row.meta, t.dim))
            })
            .when(row.charted, |d| {
                d.child(small(
                    t,
                    &match stats {
                        Some((peak, avg)) => format!(
                            "peak {} · avg {}",
                            host::percent(Some(peak)),
                            host::percent(Some(avg))
                        ),
                        None => "no history".into(),
                    },
                    t.dim,
                ))
            });
        if !row.charted {
            return div().flex().items_center().child(label).into_any_element();
        }
        let scale = div()
            .w(px(board::SCALE_COL))
            .flex_none()
            .h(px(board::CHART_H))
            .flex()
            .flex_col()
            .justify_between()
            .items_end()
            .pr(px(board::GAP_SMALL))
            .child(small(t, &format!("{top:.0}%"), t.dim))
            .child(small(t, "0", t.dim));
        let line = Line {
            id: row.id.clone(),
            runs: s.runs(row.metric, span.start),
            top,
            start: span.start,
            end: span.end,
            ticks: span.ticks.clone(),
            hover,
            ink: if row.warn { t.error } else { t.accent },
            grid: t.divider,
        };
        let selector = format!("metric-{}", row.id);
        div()
            .id(gpui::SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .child(label)
            .child(scale)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(board::CHART_H))
                    .child(chart::chart(line, cx.entity().downgrade())),
            )
            .into_any_element()
    }
}

/// Events shown on a floor, newest first; the rest stay in the store.
const EVENTS_SHOWN: usize = 6;
/// What happened to this environment's node lately, newest first, a
/// failure in the error colour.
fn floor_events(t: &Theme, s: &State) -> Div {
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(board::THIN))
        .pt(px(board::GAP_SMALL))
        .child(kit::label(t, format!("Events · {}", s.events.len())));
    if s.events.is_empty() {
        return list.child(small(
            t,
            "Nothing has happened since the node started.",
            t.dim,
        ));
    }
    for e in s.events.iter().rev().take(EVENTS_SHOWN) {
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(board::SMALL_GAP))
                .max_w(px(board::EVENT_W))
                .child(small(t, &hhmm(e.at), t.dim).flex_none())
                .child(wrapping(
                    t,
                    &e.text,
                    if e.failure { t.error } else { t.text2 },
                )),
        );
    }
    list
}

fn small(t: &Theme, text: &str, c: Hsla) -> Div {
    div()
        .text_size(px(size::SMALL))
        .text_color(c)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text.to_string())
        .when(text.is_empty(), |d| d.text_color(t.dim))
}

/// A line that wraps instead of cutting off: hardware names (a GPU's, a
/// CPU's) are longer than any column can promise.
fn wrapping(t: &Theme, text: &str, c: Hsla) -> Div {
    small(t, text, c).whitespace_normal()
}

/// The time axis over the charts: how far back they reach, the local time
/// at each tick, and the now edge.
fn axis(t: &Theme, span: &Span) -> Div {
    let width = span.end.saturating_sub(span.start).max(1) as f32;
    let mut track = div().relative().flex_1().min_w_0().h(px(board::AXIS_H));
    for at in &span.ticks {
        let share = at.saturating_sub(span.start) as f32 / width;
        track = track.child(
            div()
                .absolute()
                .top_0()
                .left(relative(share))
                .ml(px(-board::TICK_W / 2.))
                .w(px(board::TICK_W))
                .flex()
                .justify_center()
                .child(small(t, &hhmm(*at), t.dim)),
        );
    }
    let hours = (span.end - span.start + host::HOUR / 2) / host::HOUR;
    div()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(board::METRIC_LABEL + board::SCALE_COL))
                .flex_none()
                .child(small(t, &format!("last {hours} h"), t.dim)),
        )
        .child(
            track.child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .child(small(t, "now", t.dim)),
            ),
        )
}

/// `cpu · os · up 3d 5h`; on WSL, a word that the figures are the VM's.
fn facts_line(t: &Theme, s: &State, environment: &str) -> Div {
    let mut parts: Vec<String> = vec![];
    parts.extend(s.facts.cpu.clone());
    parts.extend(s.facts.os.clone());
    if let Some(up) = s.latest.as_ref().and_then(|r| r.uptime) {
        parts.push(format!("up {}", host::uptime(up)));
    }
    if is_vm(environment) {
        parts.push("figures are the WSL VM's".into());
    }
    wrapping(t, &parts.join(" · "), t.text2)
}

/// CHROTE keeps storage a meter in the head, not a chart.
fn storage(t: &Theme, s: &State) -> Div {
    let disk = s.latest.as_ref().and_then(|r| r.disk.clone());
    let Some(d) = disk else {
        return small(t, "storage unavailable", t.dim);
    };
    let share = host::Usage {
        used: d.used,
        total: d.total,
    }
    .percent();
    let warn = share.is_some_and(|p| p >= WARN_DISK);
    let tone = if warn { t.error } else { t.accent };
    let fill = share.unwrap_or(0.).clamp(0., 100.) / 100.;
    div()
        .flex()
        .flex_col()
        .gap(px(board::GAP_TIGHT))
        .child(
            div()
                .flex()
                .gap(px(board::SMALL_GAP))
                .child(kit::label(t, format!("Storage {}", d.mount)))
                .child(small(
                    t,
                    &host::usage(host::Usage {
                        used: d.used,
                        total: d.total,
                    }),
                    t.text2,
                ))
                .child(div().flex_1())
                .child(small(
                    t,
                    &host::percent(share),
                    if warn { t.error } else { t.text2 },
                )),
        )
        .child(
            div()
                .h(px(board::METER_H))
                .w_full()
                .rounded(px(board::THIN))
                .bg(t.divider)
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative(fill))
                        .rounded(px(board::THIN))
                        .bg(tone),
                ),
        )
}

/// CHROTE's rows: CPU, load, RAM, swap, and the first GPU and its memory.
/// Windows has no load average, and its swap is the commit charge.
fn rows(node: &str, s: &State, environment: &str) -> Vec<Row> {
    let windows = environment == "windows";
    let latest = s.latest.as_ref();
    let threads = s.facts.threads;
    let newest = s.history.last();
    let unread = |what: &str| match latest {
        None => "loading".to_string(),
        Some(_) => format!("{what} unavailable"),
    };
    let row = |key: &str, label: &'static str, metric: Metric| Row {
        id: format!("{node}-{key}"),
        label,
        metric,
        now: newest.and_then(|p| metric.of(p)),
        value: String::new(),
        meta: String::new(),
        warn: false,
        charted: true,
    };
    let cpu = latest.and_then(|r| r.cpu);
    let memory = latest.and_then(|r| r.memory);
    let mem_share = memory.and_then(|m| m.percent());
    let load = latest.and_then(|r| r.load);
    let swap = latest.and_then(|r| r.swap);
    let gpu = latest.and_then(|r| r.gpus.first());
    let vram = gpu.and_then(|g| g.memory);
    vec![
        Row {
            now: cpu,
            value: match (cpu, threads) {
                (Some(_), n) if n > 0 => format!("{n} threads"),
                (Some(_), _) => String::new(),
                (None, _) => unread("CPU"),
            },
            warn: cpu.is_some_and(|c| c >= WARN_CPU),
            ..row("cpu", "CPU", Metric::Cpu)
        },
        if windows {
            Row {
                now: None,
                value: "Windows keeps no load average".into(),
                charted: false,
                ..row("load", "Load", Metric::Load)
            }
        } else {
            let r = row("load", "Load", Metric::Load);
            Row {
                value: match load {
                    Some([one, five, fifteen]) => format!("{one:.2} / {five:.2} / {fifteen:.2}"),
                    None => unread("load"),
                },
                meta: if load.is_some() && threads > 0 {
                    format!("1m / 5m / 15m across {threads} threads")
                } else {
                    String::new()
                },
                warn: r.now.is_some_and(|l| l >= WARN_LOAD),
                ..r
            }
        },
        Row {
            now: mem_share,
            value: memory.map_or_else(|| unread("memory"), host::usage),
            warn: mem_share.is_some_and(|m| m >= WARN_MEMORY),
            ..row("memory", "RAM", Metric::Memory)
        },
        Row {
            now: swap.and_then(|u| u.percent()),
            value: match swap {
                Some(u) if u.total > 0 => host::usage(u),
                Some(_) => "swap not configured".into(),
                None if windows => unread("commit"),
                None => unread("swap"),
            },
            meta: if windows {
                "commit charge: memory plus page files".into()
            } else {
                String::new()
            },
            ..row(
                "swap",
                if windows { "Commit" } else { "Swap" },
                Metric::Swap,
            )
        },
        Row {
            now: gpu.and_then(|g| g.util),
            value: gpu
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "No GPU data".into()),
            meta: gpu
                .and_then(|g| g.temp)
                .map(|c| format!("{c:.0}\u{a0}°C"))
                .unwrap_or_default(),
            ..row("gpu", "GPU", Metric::Gpu)
        },
        Row {
            now: vram.and_then(|u| u.percent()),
            value: vram.map_or_else(|| "VRAM unavailable".into(), host::usage),
            ..row("vram", "VRAM", Metric::Vram)
        },
    ]
}
