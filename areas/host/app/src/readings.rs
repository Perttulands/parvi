//! The host readings page: a node's facts, charts, history and events.
use crate::{
    host_store::{self, Hosts, StateExt},
    tokens,
};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px};
use parvi_app_kit::{
    Host, Page,
    theme::{self, size},
    widgets::kit,
};
use parvi_core::NodeId;
use parvi_host_contract::{HOST, HOST_EVENT, HOST_STATE, State};

pub type HostState<'a> = Option<&'a State>;
pub struct Span {
    pub start: u64,
    pub end: u64,
    pub ticks: Vec<u64>,
}
pub fn hhmm(ms: u64) -> String {
    parvi_app_kit::clock::hhmm(ms)
}
pub fn is_vm(environment: &str) -> bool {
    environment.eq_ignore_ascii_case("wsl")
}

pub struct ReadingsPage {
    pub(crate) host: Host,
    state: Hosts,
    pub(crate) hover: Option<(String, u64)>,
    paused: bool,
    shown: bool,
}
impl ReadingsPage {
    pub fn new(host: Host, _: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Hosts::default(),
            hover: None,
            paused: false,
            shown: false,
        }
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
        if shown && self.paused {
            self.host.channel(HOST, false, cx);
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
        if self.shown
            && !self.paused
            && let Some(event) = self.state.take(node.as_str(), kind, body)
        {
            if let Some(event) = event {
                let status = if event.failure {
                    parvi_app_kit::host::Status::failure(event.text)
                } else {
                    parvi_app_kit::host::Status::new(event.text)
                };
                self.host.report(status, cx);
            }
            cx.notify();
        }
    }
}
impl Render for ReadingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme(cx).clone();
        let nodes = self.host.nodes(cx);
        let end = self
            .state
            .nodes
            .values()
            .filter_map(|s| s.latest.as_ref().map(|r| r.at))
            .max()
            .unwrap_or_else(parvi_app_kit::hub::now_ms);
        let oldest = self.state.nodes.values().filter_map(StateExt::oldest).min();
        let start = end.saturating_sub(host_store::span(oldest, end));
        let span = Span {
            start,
            end,
            ticks: host_store::ticks(
                start,
                end,
                time::UtcOffset::current_local_offset()
                    .unwrap_or(time::UtcOffset::UTC)
                    .whole_seconds()
                    .into(),
            ),
        };
        let mut body = div().flex().flex_col().gap(px(tokens::GAP));
        for node in &nodes {
            let state = self.state.nodes.get(node.id.as_str());
            body = body.child(
                div()
                    .border_1()
                    .border_color(t.divider)
                    .rounded(px(size::RADIUS))
                    .bg(t.surface)
                    .child(self.render_floor(&t, node, state, false, &span, cx)),
            );
        }
        if nodes.is_empty() {
            body = body.child(kit::note(&t, "Waiting for a node…", Some(t.dim)));
        }
        div()
            .id("host-readings")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                kit::page_bar(&t)
                    .child(kit::page_title(&t, "Host"))
                    .child(div().flex_1())
                    .child(
                        kit::button(
                            &t,
                            "host-pause",
                            if self.paused { "Resume" } else { "Pause" },
                            kit::Kind::Quiet,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.paused = !s.paused;
                            s.host.channel(HOST, !s.paused, cx);
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("host-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(tokens::PAD))
                    .child(body),
            )
    }
}
