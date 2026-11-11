//! Agents: harnesses, tools and their skills checkouts across nodes.
pub mod state;
mod view;
mod tokens {
    pub const RAIL_W: f32 = 300.;
    pub const DETAIL_MAX: f32 = 1100.;
    pub const AVATAR: f32 = 28.;
    pub const TITLE: f32 = 24.;
    pub const DIMMED: f32 = 0.5;
    pub const P2: f32 = 2.;
    pub const P4: f32 = 4.;
    pub const P6: f32 = 6.;
    pub const P8: f32 = 8.;
    pub const P10: f32 = 10.;
    pub const P12: f32 = 12.;
    pub const P20: f32 = 20.;
    pub const LARGE_AVATAR: f32 = 42.;
    pub const TABLE_ROW: f32 = 32.;
    pub const SELECTED: f32 = 0.1;
    pub const HOVER: f32 = 0.04;
    pub const LINE: f32 = 0.5;
}
use gpui::{App, AppContext, Context, Entity, IntoElement, Render, Window};
use parvi_app_kit::host::Status;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_core::NodeId;
use state::{Agents, Synced, WATCH};
use std::collections::BTreeMap;

pub struct AgentsPage {
    host: Host,
    state: Agents,
    filter: Option<Entity<TextField>>,
    places: BTreeMap<String, String>,
}
impl AgentsPage {
    fn new(host: Host, _: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Agents::default(),
            filter: None,
            places: BTreeMap::new(),
        }
    }
    fn agents_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(field) = &self.filter {
            return field.clone();
        }
        let field = cx.new(|cx| TextField::new(cx, "filter agents"));
        cx.subscribe_in(&field, window, |_, _, event, _, cx| {
            if let FieldEvent::Changed = event {
                cx.notify();
            }
        })
        .detach();
        self.filter = Some(field.clone());
        field
    }
    fn place_name(&self, node: &str) -> String {
        self.places
            .get(node)
            .cloned()
            .unwrap_or_else(|| node.to_owned())
    }
    fn place_tag(&self, t: &parvi_app_kit::theme::Theme, node: &str) -> gpui::Div {
        parvi_app_kit::widgets::kit::note(t, self.place_name(node), Some(t.text2))
    }
    fn agents_reports(&self) -> Vec<(&str, &state::Report)> {
        self.state
            .reports
            .iter()
            .map(|(n, r)| (n.as_str(), r))
            .collect()
    }
    fn agents_refresh(&mut self, cx: &mut Context<Self>) {
        self.host.channel(WATCH, true, cx);
    }
    fn agents_sync(&mut self, node: &str, checkout: &str, cx: &mut Context<Self>) {
        let frame = self.state.sync(node, checkout);
        if !self.host.send(&NodeId::from(node), frame, cx) {
            self.state.dropped(node);
            self.host.report(
                Status::failure(format!("Sync of {checkout} did not start: not connected")),
                cx,
            );
        }
        cx.notify();
    }
}
impl Page for AgentsPage {
    const ID: &'static str = "agents";
    const TITLE: &'static str = "Agents";
    const AREA: Option<&'static str> = Some("agents");
    const KINDS: parvi_core::frame::Kinds = &[0x80..=0x8f];
    const CHANNELS: &'static [u8] = &[WATCH];
    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.state.frame(node.as_str(), kind, body) {
            Some(Some(Synced::Started { checkout, session })) => self.host.toast(
                &format!("Syncing {checkout} in {session}; refresh when done"),
                cx,
            ),
            Some(Some(Synced::Refused { checkout, why })) => self.host.report(
                Status::failure(format!("Sync of {checkout} did not start: {why}")),
                cx,
            ),
            _ => {}
        }
        cx.notify();
    }
    fn link(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state != Some(LinkState::Open) {
            let pending = self.state.dropped(node.as_str());
            if !pending.is_empty() {
                self.host.report(
                    Status::failure(format!(
                        "Sync outcome unknown after disconnect: {}",
                        pending.join(", ")
                    )),
                    cx,
                );
            }
        }
    }
}
impl Render for AgentsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.places = self
            .host
            .nodes(cx)
            .into_iter()
            .map(|n| {
                (
                    n.id.to_string(),
                    format!("{} · {}", n.device, n.environment),
                )
            })
            .collect();
        self.render_agents(window, cx)
    }
}
pub fn build(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, AgentsPage::new)
}
