use crate::store::{Store, canvas as board, flow};
use gpui::{
    Bounds, Context, Div, IntoElement, PathBuilder, Pixels, Render, Window, canvas, div, point,
    prelude::*, px,
};
use parvi_app_kit::{
    Host, Page,
    host::Request,
    theme::{board::LIST_W, size, theme},
    widgets::boundary,
};
use parvi_core::NodeId;
use parvi_missions_contract::{LayoutResponse, MissionResponse, WATCH, WATCH_RUNS};

const PAD: f32 = 12.;
const EDGE: f32 = 2.;
pub struct MissionsPage {
    host: Host,
    state: Store,
    generation: u64,
}
impl MissionsPage {
    pub fn new(host: Host, _: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Store::default(),
            generation: 0,
        }
    }
    fn open(&mut self, node: String, slug: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state.selected = Some((node.clone(), slug.clone()));
        self.state.mission = None;
        self.state.layout = None;
        self.state.error = None;
        // Encode a path segment, including non-ASCII bytes.
        let encoded: String = slug
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        for layout in [false, true] {
            let path = format!(
                "/api/missions/{encoded}{}",
                if layout { "/layout" } else { "" }
            );
            let asked = self
                .host
                .request(Request::get(NodeId(node.clone()), path), cx);
            let key = (node.clone(), slug.clone());
            cx.spawn(async move |this, cx| {
                let answer = asked.await;
                let _ = this.update(cx, |page, cx| {
                    if page.generation != generation || page.state.selected.as_ref() != Some(&key) {
                        return;
                    }
                    match answer {
                        Ok(answer) if answer.ok() => {
                            if layout {
                                match serde_json::from_slice::<LayoutResponse>(&answer.body) {
                                    Ok(v) => page.state.layout = Some(v.layout.layout),
                                    Err(e) => page.state.error = Some(e.to_string()),
                                }
                            } else {
                                match serde_json::from_slice::<MissionResponse>(&answer.body) {
                                    Ok(v) => page.state.mission = Some(v.mission.mission),
                                    Err(e) => page.state.error = Some(e.to_string()),
                                }
                            }
                        }
                        Ok(answer) => page.state.error = Some(answer.error_text()),
                        Err(e) => page.state.error = Some(e.message),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }
}
impl Page for MissionsPage {
    const ID: &'static str = "missions";
    const TITLE: &'static str = "Missions";
    const AREA: Option<&'static str> = Some("missions");
    const KINDS: parvi_core::frame::Kinds = parvi_missions_contract::KINDS;
    const CHANNELS: &'static [u8] = &[WATCH, WATCH_RUNS];
    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.frame(node.as_str(), kind, body);
        if kind == parvi_missions_contract::SNAPSHOT
            && let Some((selected_node, slug)) = self.state.selected.clone()
            && selected_node == node.as_str()
            && let Some(mission) = &self.state.mission
            && let Some(listing) = self
                .state
                .nodes
                .get(&selected_node)
                .and_then(|n| n.missions.as_ref())
                .and_then(|list| list.iter().find(|m| m.slug == slug))
            && listing.rev.is_some_and(|rev| rev != mission.rev)
        {
            self.open(selected_node, slug, cx);
        }
        cx.notify();
    }
}
impl Render for MissionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut list = div().flex().flex_col().gap(px(PAD));
        for (node, feed) in &self.state.nodes {
            if let Some(missions) = &feed.missions {
                for mission in missions {
                    let (node, slug) = (node.clone(), mission.slug.clone());
                    let label = if mission.title.is_empty() {
                        slug.clone()
                    } else {
                        mission.title.clone()
                    };
                    let selector = format!("mission-{node}-{slug}");
                    list = list.child(
                        div()
                            .id(gpui::SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .p(px(PAD))
                            .cursor_pointer()
                            .border_1()
                            .border_color(t.divider)
                            .child(label)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.open(node.clone(), slug.clone(), cx)
                            })),
                    );
                    let mut runs: Vec<_> = feed
                        .runs
                        .values()
                        .filter(|r| r.mission_slug == mission.slug)
                        .collect();
                    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                    for run in runs {
                        list = list.child(
                            div()
                                .px(px(PAD))
                                .text_color(t.text2)
                                .child(format!("{} · {}", run.run_id, run.status)),
                        );
                    }
                }
            }
        }
        let content: Div = if let Some(m) = &self.state.mission {
            let layout = self
                .state
                .layout
                .as_ref()
                .map(|l| l.nodes.as_slice())
                .unwrap_or(&[]);
            let board = board::board(m, &flow::build(m), layout, false);
            let (x, y, width, height) = board::extent(&board);
            let dx = PAD - x.min(0.);
            let dy = PAD - (y - 24.).min(0.);
            let wires = board.wires.clone();
            let ink = t.text2;
            let lines = canvas(
                |_, _, _| (),
                boundary::paint(move |b: Bounds<Pixels>, _: (), window, _| {
                    for wire in &wires {
                        let at = |p: (f32, f32)| {
                            point(b.origin.x + px(p.0 + dx), b.origin.y + px(p.1 + dy))
                        };
                        let mut path = PathBuilder::stroke(px(EDGE));
                        path.move_to(at(wire.from));
                        if let Some(y) = wire.lane {
                            path.line_to(at((wire.from.0, y)));
                            path.line_to(at((wire.to.0, y)));
                        }
                        path.line_to(at(wire.to));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, ink);
                        }
                    }
                }),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut drawing = div()
                .relative()
                .flex_none()
                .w(px(width + x + dx + PAD))
                .h(px(height + y + dy + PAD))
                .child(lines);
            for card in board.cards {
                let selector = format!("mission-card-{}", card.id);
                let details = m
                    .formations
                    .iter()
                    .find(|f| f.id == card.id)
                    .map(|f| {
                        let brief = f.brief.as_ref().map(|b| b.goal.as_str()).unwrap_or("");
                        let slots = f
                            .slots
                            .iter()
                            .map(|s| s.name())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("{}\n{}\n{}", f.kind, brief, slots)
                    })
                    .or_else(|| {
                        m.input_cards
                            .iter()
                            .find(|c| c.id == card.id)
                            .map(|c| c.goal.clone())
                    })
                    .unwrap_or_default();
                drawing = drawing.child(
                    div()
                        .absolute()
                        .left(px(card.x + dx))
                        .top(px(card.y + dy))
                        .w(px(card.w))
                        .h(px(card.h))
                        .p(px(PAD))
                        .bg(t.bg)
                        .border_1()
                        .border_color(t.divider)
                        .rounded(px(size::RADIUS))
                        .debug_selector(move || selector.clone())
                        .child(m.node_name(&card.id).to_owned())
                        .child(div().text_color(t.text2).child(details)),
                );
            }
            drawing
        } else {
            div().p(px(PAD)).child(
                self.state
                    .error
                    .clone()
                    .unwrap_or_else(|| "Select a mission".into()),
            )
        };
        div()
            .size_full()
            .flex()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                div()
                    .id("missions-list")
                    .w(px(LIST_W))
                    .flex_none()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                div()
                    .id("missions-canvas")
                    .flex_1()
                    .min_w_0()
                    .overflow_scroll()
                    .child(content),
            )
    }
}
