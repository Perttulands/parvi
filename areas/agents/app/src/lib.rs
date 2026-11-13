//! Agents: whether the harnesses can run with their skills current, and
//! what to do where they cannot, across nodes; and the roles library.
mod library;
mod role_state;
pub mod state;
mod view;
mod tokens {
    /// An off line's name column and its fix's.
    pub const SUBJECT_W: f32 = 160.;
    pub const FIX_W: f32 = 220.;
    /// Above and below an off line, which may wrap.
    pub const OFF_PAD: f32 = 6.;
    /// A sync on its way.
    pub const DIMMED: f32 = 0.5;
    /// The role library's measures.
    pub const P4: f32 = 4.;
    pub const P8: f32 = 8.;
}
use gpui::{App, Context, IntoElement, Render, Window};
use parvi_app_kit::host::Status;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_core::NodeId;
use state::{Agents, Synced, WATCH};
use std::collections::BTreeMap;

pub struct AgentsPage {
    host: Host,
    library: library::Library,
    state: Agents,
    places: BTreeMap<String, String>,
}
impl AgentsPage {
    fn new(host: Host, _: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            host,
            library: library::Library::default(),
            state: Agents::default(),
            places: BTreeMap::new(),
        }
    }
    fn place_name(&self, node: &str) -> String {
        self.places
            .get(node)
            .cloned()
            .unwrap_or_else(|| node.to_owned())
    }
    fn agents_reports(&self) -> Vec<(&str, &state::Report)> {
        self.state
            .reports
            .iter()
            .map(|(n, r)| (n.as_str(), r))
            .collect()
    }
    fn agents_refresh(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            self.host.send(&node.id, vec![WATCH, 1], cx);
        }
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
    const KEY: Option<char> = Some('A');
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
            Some(Some(Synced::Refused { code, .. }))
                if code == parvi_agents_contract::FETCH_FAILED => {}
            Some(Some(Synced::Refused { checkout, why, .. })) => self.host.report(
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
