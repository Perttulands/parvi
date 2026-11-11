//! Mission canvas interaction, adapted from the original Missions page.
use super::*;
use gpui::{
    Bounds, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels,
    Point, ScrollEvent, SharedString, Stateful, canvas, point,
};
use parvi_app_kit::widgets::boundary;
use parvi_missions_contract::{LayoutNode, Mission};
use std::{cell::Cell, rc::Rc};

const ZOOM_MIN: f32 = 0.3;
const ZOOM_MAX: f32 = 1.6;
const ZOOM_STEP: f32 = 1.2;
const FIT_MARGIN: f32 = 80.;
const DRAG_SLOP: f32 = 3.;
const CURVE_REACH: f32 = 40.;
const CURVE_DOWN: f32 = 60.;
const DASH_SHORT: f32 = 3.;
const DASH_LONG: f32 = 5.;
const TITLE_SIZE: f32 = 16.;
const RING_SIZE: f32 = 30.;
const GAP: f32 = 8.;
const SLOT_PAD: f32 = 10.;
const POOL_LABEL: f32 = 14.;
const CHIP_PAD: f32 = 6.;
const GATE_MARK: f32 = 46.;
const END_MARK: f32 = 22.;
const CHIP_ALPHA: f32 = 0.14;
#[derive(Clone, Debug)]
enum Drag {
    Pan(Point<Pixels>, (f32, f32)),
    Card {
        id: String,
        at: Point<Pixels>,
        from: (f32, f32),
        now: (f32, f32),
        moved: bool,
    },
    Wire {
        from: String,
        start: (f32, f32),
        lifted: Option<String>,
        now: (f32, f32),
    },
}
#[derive(Clone, Debug)]
pub(super) struct View {
    zoom: f32,
    pan: (f32, f32),
    drag: Option<Drag>,
    menu: Option<(f32, f32)>,
    bounds: Rc<Cell<(f32, f32, f32, f32)>>,
    fitted: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.,
            pan: (PAD, PAD),
            drag: None,
            menu: None,
            bounds: Rc::new(Cell::new((0., 0., 0., 0.))),
            fitted: false,
        }
    }
}
impl View {
    pub(super) fn position(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.zoom + self.pan.0, y * self.zoom + self.pan.1)
    }
    pub(super) fn zoom(&self) -> f32 {
        self.zoom
    }
    pub(super) fn invalidate_fit(&mut self) {
        self.fitted = false;
    }
    pub(super) fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }
    fn zoom_at(&mut self, factor: f32, at: (f32, f32)) {
        let z = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let k = z / self.zoom;
        self.pan = (
            at.0 - (at.0 - self.pan.0) * k,
            at.1 - (at.1 - self.pan.1) * k,
        );
        self.zoom = z;
    }
    fn world(&self, p: Point<Pixels>) -> (f32, f32) {
        let (x, y, _, _) = self.bounds.get();
        (
            (f32::from(p.x) - x - self.pan.0) / self.zoom,
            (f32::from(p.y) - y - self.pan.1) / self.zoom,
        )
    }
    fn center(&self) -> (f32, f32) {
        let (_, _, w, h) = self.bounds.get();
        (w / 2., h / 2.)
    }
    fn fit(&mut self, board: &board::Board) {
        let (_, _, w, h) = self.bounds.get();
        if w <= 0. || h <= 0. {
            return;
        }
        let (x, y, bw, bh) = board::extent(board);
        let z = ((w - FIT_MARGIN) / bw)
            .min((h - FIT_MARGIN) / bh)
            .clamp(ZOOM_MIN, 1.);
        self.zoom = z;
        self.pan = ((w - bw * z) / 2. - x * z, (h - bh * z) / 2. - y * z);
        self.fitted = true;
    }
}

impl MissionsPage {
    pub(super) fn render_canvas(
        &mut self,
        t: &parvi_app_kit::theme::Theme,
        mission: &Mission,
        running: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut layout = self
            .state
            .layout
            .as_ref()
            .map(|l| l.nodes.clone())
            .unwrap_or_default();
        if let Some(Drag::Card { id, now, .. }) = &self.canvas.drag {
            layout.retain(|p| p.id != *id);
            layout.push(LayoutNode {
                id: id.clone(),
                x: now.0.round() as i64,
                y: now.1.round() as i64,
            });
        }
        let board = board::board(mission, &flow::build(mission), &layout, running);
        let states = self
            .selected_run()
            .map(|run| crate::store::run_state::project(&run.events));
        let ports = board::ports(mission, &board);
        let view = self.canvas.clone();
        let drag_ports = ports.clone();
        let mut layer = div()
            .id("mission-canvas")
            .debug_selector(|| "mission-canvas".into())
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(t.bg)
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, e: &MouseDownEvent, _, cx| {
                    p.canvas.menu = None;
                    p.canvas.drag = Some(Drag::Pan(e.position, p.canvas.pan));
                    cx.notify();
                }),
            )
            .when(!running, |d| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|p, e: &MouseDownEvent, _, cx| {
                        p.canvas.menu = Some(p.canvas.world(e.position));
                        cx.notify();
                    }),
                )
            })
            .on_mouse_move(cx.listener(|p, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    p.canvas.drag = None;
                    return;
                }
                let world = p.canvas.world(e.position);
                let z = p.canvas.zoom;
                match &mut p.canvas.drag {
                    None => return,
                    Some(Drag::Pan(from, pan)) => {
                        let d = e.position - *from;
                        p.canvas.pan = (pan.0 + f32::from(d.x), pan.1 + f32::from(d.y));
                    }
                    Some(Drag::Card {
                        at,
                        from,
                        now,
                        moved,
                        ..
                    }) => {
                        let d = e.position - *at;
                        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                        *moved |= dx.hypot(dy) > DRAG_SLOP;
                        *now = (from.0 + dx / z, from.1 + dy / z);
                    }
                    Some(Drag::Wire { now, .. }) => *now = world,
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |p, e: &MouseUpEvent, _, cx| {
                    let at = p.canvas.world(e.position);
                    match p.canvas.drag.take() {
                        Some(Drag::Card {
                            id,
                            from,
                            now,
                            moved,
                            ..
                        }) => {
                            if moved {
                                p.canvas_move(&id, from, now, cx);
                            } else {
                                p.edit_step(&id, cx);
                            }
                        }
                        Some(Drag::Wire { from, lifted, .. }) => {
                            let target = board::drop_target(&drag_ports, &from, at)
                                .map(|port| port.endpoint.clone());
                            p.canvas_wire(from, lifted, target, cx);
                        }
                        _ => (),
                    }
                    cx.notify();
                }),
            )
            .on_scroll(cx.listener(|p, e: &ScrollEvent, _, cx| {
                let delta = e.delta.pixel_delta(px(size::ROW));
                if e.modifiers.control || e.modifiers.platform {
                    let factor = if f32::from(delta.y) > 0. {
                        ZOOM_STEP
                    } else {
                        1. / ZOOM_STEP
                    };
                    let center = p.canvas.center();
                    p.canvas.zoom_at(factor, center);
                } else {
                    p.canvas.pan.0 += f32::from(delta.x);
                    p.canvas.pan.1 += f32::from(delta.y);
                }
                p.canvas.menu = None;
                cx.notify();
            }));
        layer = layer.child(wires(
            t,
            &board,
            view.clone(),
            cx.weak_entity(),
            self.generation,
        ));
        for card in &board.cards {
            let state = states
                .as_ref()
                .and_then(|s| s.states.get(&card.id))
                .copied();
            let attempts = states
                .as_ref()
                .and_then(|s| s.attempts.get(&card.id))
                .copied();
            layer = layer
                .child(self.canvas_card(t, mission, card, &view, running, state, attempts, cx));
        }
        for note in self.canvas_notes(t, mission, &board, &view, cx) {
            layer = layer.child(note);
        }
        if !running {
            for port in &ports {
                layer = layer.child(self.canvas_port(t, mission, port, &view, cx));
            }
        }
        if !running && let Some(at) = view.menu {
            layer = layer.child(self.canvas_menu(t, mission, at, &view, cx));
        }
        let fit = board.clone();
        let controls = div()
            .absolute()
            .occlude()
            .right(px(PAD))
            .bottom(px(PAD))
            .flex()
            .flex_col()
            .gap(px(size::RADIUS))
            .child(
                kit::button(t, "canvas-in", "+", Kind::Plain)
                    .debug_selector(|| "canvas-in".into())
                    .on_click(cx.listener(|p, _, _, cx| {
                        let center = p.canvas.center();
                        p.canvas.zoom_at(ZOOM_STEP, center);
                        cx.notify();
                    })),
            )
            .child(
                kit::button(t, "canvas-out", "−", Kind::Plain)
                    .debug_selector(|| "canvas-out".into())
                    .on_click(cx.listener(|p, _, _, cx| {
                        let center = p.canvas.center();
                        p.canvas.zoom_at(1. / ZOOM_STEP, center);
                        cx.notify();
                    })),
            )
            .child(
                kit::button(t, "canvas-fit", "Fit", Kind::Plain)
                    .debug_selector(|| "canvas-fit".into())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.canvas.fit(&fit);
                        cx.notify();
                    })),
            )
            .when(!running, |d| {
                d.child(
                    kit::button(t, "canvas-arrange", "Arrange", Kind::Plain)
                        .on_click(cx.listener(|p, _, _, cx| p.arrange(cx))),
                )
            })
            .child(
                div()
                    .text_size(px(size::SMALL))
                    .child(format!("{:.0}%", view.zoom * 100.)),
            );
        if let Some(id) = &self.reveal_card {
            let selector = format!("mission-reveal-{id}");
            layer = layer.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .debug_selector(move || selector.clone())
                    .child(format!("Card: {}", mission.node_name(id))),
            );
        }
        div().size_full().child(layer.child(controls))
    }
    #[allow(clippy::too_many_arguments)]
    fn canvas_card(
        &self,
        t: &parvi_app_kit::theme::Theme,
        mission: &Mission,
        card: &board::Card,
        view: &View,
        running: bool,
        state: Option<crate::store::run_state::NodeState>,
        attempts: Option<i64>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let z = view.zoom;
        let id = card.id.clone();
        let from = (card.x, card.y);
        let selector = format!("mission-card-{id}");
        let face = card_face(t, mission, card, z, running, state);
        let color = match state {
            Some(crate::store::run_state::NodeState::Done) => parvi_app_kit::theme::rgb(t.ansi[2]),
            Some(
                crate::store::run_state::NodeState::Failed
                | crate::store::run_state::NodeState::Blocked,
            ) => t.error,
            Some(
                crate::store::run_state::NodeState::Running
                | crate::store::run_state::NodeState::Waiting,
            ) => t.accent,
            _ if card.kind == board::Kind::End => {
                if mission
                    .ends
                    .iter()
                    .any(|e| e.id == card.id && e.outcome == "rejected")
                {
                    t.error
                } else {
                    parvi_app_kit::theme::rgb(t.ansi[2])
                }
            }
            _ => t.divider,
        };
        let rendered = div()
            .id(SharedString::from(format!("card-{id}")))
            .absolute()
            .left(px(card.x * z + view.pan.0))
            .top(px(card.y * z + view.pan.1))
            .w(px(card.w * z))
            .h(px(card.h * z))
            .overflow_hidden()
            .border_1()
            .border_color(
                if self.reveal_card.as_deref() == Some(&id) || card.kind == board::Kind::Input {
                    t.accent
                } else {
                    color
                },
            )
            .bg(t.surface)
            .shadow_md()
            .when(card.kind == board::Kind::End, |d| {
                d.rounded(px(board::END_H * z / 2.))
            })
            .when(card.kind != board::Kind::End, |d| {
                d.rounded(px(size::RADIUS * z))
            })
            .cursor_pointer()
            .debug_selector(move || selector.clone())
            .child(face);
        let rendered = if card.kind == board::Kind::Input && !running {
            let card_id = card.id.clone();
            let runnable = self.mission_input_runnable(&card_id);
            rendered.child(
                div()
                    .id(SharedString::from(format!("canvas-play-{card_id}")))
                    .debug_selector({
                        let card_id = card_id.clone();
                        move || format!("canvas-play-{card_id}")
                    })
                    .absolute()
                    .top(px(PAD * z))
                    .right(px(PAD * z))
                    .size(px(RING_SIZE * z))
                    .rounded(px(size::RADIUS * z))
                    .bg(if runnable { t.accent } else { t.raised })
                    .text_color(if runnable { t.on_accent() } else { t.dim })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("▶")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |page, _, _, cx| {
                        cx.stop_propagation();
                        page.prepare_card_run(&card_id, cx);
                    })),
            )
        } else {
            rendered
        };
        let rendered = if running && let Some(count) = attempts.filter(|n| *n > 0) {
            let step = card.id.clone();
            let offset = mission
                .formations
                .iter()
                .find(|f| f.id == step)
                .map(|f| {
                    f.inputs.len() as f32 * board::IO_ROW + board::step_head(f, true)
                        - size::CONTROL
                })
                .unwrap_or(0.);
            rendered.child(
                div()
                    .id(SharedString::from(format!("card-attempts-{step}")))
                    .debug_selector({
                        let step = step.clone();
                        move || format!("card-attempts-{step}")
                    })
                    .absolute()
                    .left(px(PAD * z))
                    .top(px(offset * z))
                    .px(px(GAP * z))
                    .rounded(px(size::RADIUS * z))
                    .bg(t.raised)
                    .text_color(t.text2)
                    .text_size(px(size::SMALL * z))
                    .cursor_pointer()
                    .child(format!(
                        "{count} attempt{}",
                        if count == 1 { "" } else { "s" }
                    ))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |page, _, window, cx| {
                        cx.stop_propagation();
                        page.open_canvas_attempts(&step, window, cx);
                    })),
            )
        } else {
            rendered
        };
        if running {
            return rendered.cursor_default();
        }
        rendered.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |p, e: &MouseDownEvent, _, cx| {
                p.canvas.menu = None;
                p.canvas.drag = Some(Drag::Card {
                    id: id.clone(),
                    at: e.position,
                    from,
                    now: from,
                    moved: false,
                });
                cx.stop_propagation();
            }),
        )
    }
    fn canvas_port(
        &self,
        t: &parvi_app_kit::theme::Theme,
        mission: &Mission,
        port: &board::Port,
        view: &View,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let z = view.zoom;
        let diameter = board::PORT * z;
        let colour = if port.endpoint.ends_with(":pass") {
            parvi_app_kit::theme::rgb(t.ansi[2])
        } else if port.endpoint.ends_with(":fail") {
            t.error
        } else if port.out {
            t.accent
        } else {
            t.text2
        };
        let endpoint = port.endpoint.clone();
        let out = port.out;
        let at = port.at;
        let wired = port.wired;
        let existing = mission
            .connections
            .iter()
            .rev()
            .find(|c| c.to == endpoint)
            .map(|c| c.from.clone());
        let selector = format!("mission-port-{endpoint}");
        let accent = t.accent;
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(px(at.0 * z + view.pan.0 - diameter / 2.))
            .top(px(at.1 * z + view.pan.1 - diameter / 2.))
            .size(px(diameter))
            .rounded_full()
            .border_2()
            .border_color(colour)
            .bg(if wired { colour } else { t.surface })
            .cursor_crosshair()
            .hover(move |d| d.border_color(accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |p, _, _, cx| {
                    cx.stop_propagation();
                    p.canvas.menu = None;
                    p.canvas.drag = if out {
                        Some(Drag::Wire {
                            from: endpoint.clone(),
                            start: at,
                            lifted: None,
                            now: at,
                        })
                    } else {
                        existing.clone().map(|from| Drag::Wire {
                            from,
                            start: (at.0 - CURVE_DOWN, at.1),
                            lifted: Some(endpoint.clone()),
                            now: at,
                        })
                    };
                    cx.notify();
                }),
            )
    }
    fn canvas_wire(
        &mut self,
        from: String,
        lifted: Option<String>,
        to: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let patch = match (lifted, to) {
            (None, None) => return,
            (Some(old), Some(to)) if old == to => return,
            (None, Some(to)) => {
                json!({"wireConnection":{"from":from,"to":to,"joinIfOccupied":true}})
            }
            (Some(old), Some(to)) => {
                json!({"rewireConnection":{"from":from,"previousTo":old,"to":to,"joinIfOccupied":true}})
            }
            (Some(old), None) => json!({"unwireConnection":{"from":from,"to":old}}),
        };
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn canvas_move(&mut self, id: &str, from: (f32, f32), to: (f32, f32), cx: &mut Context<Self>) {
        let before = LayoutNode {
            id: id.into(),
            x: from.0.round() as i64,
            y: from.1.round() as i64,
        };
        let after = LayoutNode {
            id: id.into(),
            x: to.0.round() as i64,
            y: to.1.round() as i64,
        };
        self.canvas_layout(vec![after], vec![before], cx);
    }
    fn canvas_layout(
        &mut self,
        nodes: Vec<LayoutNode>,
        reverse: Vec<LayoutNode>,
        cx: &mut Context<Self>,
    ) {
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        let (Some((node, slug)), Some(etag), Some(mission)) = (
            self.state.selected.clone(),
            self.state.layout_etag.clone(),
            self.state.mission.as_ref(),
        ) else {
            return;
        };
        let rev = mission.rev;
        let generation = self.generation;
        let request = Request::new(
            "PATCH",
            NodeId(node),
            format!("{}/layout", mission_path(&slug)),
        )
        .header("If-Match", etag)
        .json(&json!({"nodes":nodes}));
        self.writing = true;
        let asked = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |p, cx| {
                if p.generation != generation {
                    return;
                }
                p.writing = false;
                match result {
                    Ok(answer) if answer.ok() => {
                        match serde_json::from_slice::<LayoutResponse>(&answer.body) {
                            Ok(v) => {
                                let etag = v.layout.etag;
                                p.state.layout = Some(v.layout.layout);
                                p.state.layout_etag = Some(etag.clone());
                                let entry = undo::Entry {
                                    label: "move".into(),
                                    steps: vec![undo::Step::Layout(reverse)],
                                };
                                p.history.wrote(undo::Version {
                                    rev,
                                    layout: Some(etag),
                                });
                                p.history.record(entry);
                            }
                            Err(e) => {
                                p.state.error = Some(e.to_string());
                            }
                        }
                    }
                    Ok(answer) => {
                        p.state.error = Some(answer.error_text());
                    }
                    Err(error) => {
                        p.state.error = Some(error.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn canvas_menu(
        &self,
        t: &parvi_app_kit::theme::Theme,
        mission: &Mission,
        at: (f32, f32),
        view: &View,
        cx: &mut Context<Self>,
    ) -> Div {
        let item = |id: &'static str, label: &'static str, patch: serde_json::Value| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px(px(PAD))
                .py(px(size::RADIUS))
                .cursor_pointer()
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |p, _, _, cx| {
                    p.canvas.menu = None;
                    p.write_board(patch.clone(), WriteMode::Edit, cx);
                }))
        };
        let x = at.0.round() as i64;
        let y = at.1.round() as i64;
        div().absolute().occlude().left(px(at.0*view.zoom+view.pan.0)).top(px(at.1*view.zoom+view.pan.1)).w(px(size::MENU)).bg(t.raised).border_1().border_color(t.divider).rounded(px(size::RADIUS)).shadow_lg().child(kit::label(t,"New"))
            .when(mission.input_cards.is_empty(),|d| d.child(item("canvas-new-input","Input card",json!({"createInputCard":{"title":"Input","goal":"","x":x,"y":y}}))))
            .child(item("canvas-new-solo","Solo formation",json!({"createFormation":{"type":"solo","title":"New formation","x":x,"y":y}})))
            .child(item("canvas-new-orch","Orchestrated formation",json!({"createFormation":{"type":"orchestrated","title":"New formation","x":x,"y":y}})))
            .child(item("canvas-new-gate","Gate",json!({"createGate":{"title":"Review gate","kinds":["human"],"criterion":"","x":x,"y":y}})))
            .child(item("canvas-new-done","End · done",json!({"createEnd":{"outcome":"done","title":"Done","x":x,"y":y}})))
            .child(item("canvas-new-rejected","End · rejected",json!({"createEnd":{"outcome":"rejected","title":"Rejected","x":x,"y":y}})))
    }
}

fn wires(
    t: &parvi_app_kit::theme::Theme,
    board: &board::Board,
    view: View,
    entity: gpui::WeakEntity<MissionsPage>,
    generation: u64,
) -> impl IntoElement {
    let fit_board = board.clone();
    let list = board.wires.clone();
    let accent = t.accent;
    let colors = (
        t.text2,
        t.error,
        t.dim,
        parvi_app_kit::theme::rgb(t.ansi[2]),
    );
    let bounds = view.bounds.clone();
    let loose = match &view.drag {
        Some(Drag::Wire { start, now, .. }) => Some((*start, *now)),
        _ => None,
    };
    canvas(
        |_, _, _| (),
        boundary::paint(move |b: Bounds<Pixels>, _, window, cx| {
            bounds.set((
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ));
            if !view.fitted {
                let entity = entity.clone();
                let fit_board = fit_board.clone();
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |page, cx| {
                        if page.generation == generation && !page.canvas.fitted {
                            page.canvas.fit(&fit_board);
                            if page.canvas.fitted {
                                cx.notify();
                            }
                        }
                    });
                });
            }
            let z = view.zoom;
            let at = |p: (f32, f32)| {
                point(
                    b.origin.x + px(p.0 * z + view.pan.0),
                    b.origin.y + px(p.1 * z + view.pan.1),
                )
            };
            for w in &list {
                let colour = match w.route.as_str() {
                    "pass" => colors.3,
                    "fail" => colors.1,
                    "judge" => colors.2,
                    _ => colors.0,
                };
                let (a, end) = (at(w.from), at(w.to));
                let mut path = PathBuilder::stroke(px(EDGE));
                if w.route == "judge" {
                    path = path.dash_array(&[px(DASH_SHORT), px(DASH_LONG)]);
                }
                path.move_to(a);
                if let Some(lane) = w.lane {
                    let out = px(board::LOOP_OUT * z);
                    let y = at((0., lane)).y;
                    for p in [
                        point(a.x + out, a.y),
                        point(a.x + out, y),
                        point(end.x - out, y),
                        point(end.x - out, end.y),
                        end,
                    ] {
                        path.line_to(p);
                    }
                } else {
                    let reach = px(((w.to.0 - w.from.0).abs() / 2.).max(CURVE_REACH) * z);
                    let down = px(CURVE_DOWN * z);
                    let (ca, cb) = if w.route == "judge" {
                        (
                            if w.to.1 > w.from.1 {
                                point(a.x, a.y + down)
                            } else {
                                point(a.x + reach, a.y)
                            },
                            if w.to.1 < w.from.1 {
                                point(end.x, end.y + down)
                            } else {
                                point(end.x - reach, end.y)
                            },
                        )
                    } else {
                        (point(a.x + reach, a.y), point(end.x - reach, end.y))
                    };
                    path.cubic_bezier_to(end, ca, cb);
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, colour);
                }
            }
            if let Some((from, to)) = loose {
                let (a, end) = (at(from), at(to));
                let reach = px(((to.0 - from.0).abs() / 2.).max(CURVE_REACH) * z);
                let mut path =
                    PathBuilder::stroke(px(EDGE)).dash_array(&[px(DASH_LONG), px(DASH_LONG)]);
                path.move_to(a);
                path.cubic_bezier_to(end, point(a.x + reach, a.y), point(end.x - reach, end.y));
                if let Ok(path) = path.build() {
                    window.paint_path(path, accent);
                }
            }
        }),
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn card_face(
    t: &parvi_app_kit::theme::Theme,
    mission: &Mission,
    card: &board::Card,
    z: f32,
    running: bool,
    state: Option<crate::store::run_state::NodeState>,
) -> Div {
    let label = |text: String, colour: Hsla| {
        div()
            .text_size(px(size::SMALL * z))
            .text_color(colour)
            .truncate()
            .child(text)
    };
    match card.kind {
        board::Kind::Formation => {
            let Some(f) = mission.formations.iter().find(|f| f.id == card.id) else {
                return div();
            };
            let mut face = div().size_full().flex().flex_col();
            for p in &f.inputs {
                let to = format!("{}:{}", f.id, p.id);
                let from: Vec<String> = mission
                    .connections
                    .iter()
                    .filter(|c| c.to == to)
                    .map(|c| {
                        let (id, port) = c.from.split_once(':').unwrap_or((&c.from, ""));
                        if matches!(port, "pass" | "fail" | "judge") {
                            format!("{} ({port})", mission.node_name(id))
                        } else {
                            mission.node_name(id).into()
                        }
                    })
                    .collect();
                let source = if from.is_empty() {
                    "nothing wired".into()
                } else {
                    format!("from {}", from.join(", "))
                };
                face = face.child(
                    div()
                        .h(px(board::IO_ROW * z))
                        .flex_none()
                        .px(px(PAD * z))
                        .flex()
                        .items_center()
                        .gap(px(PAD * z))
                        .border_b_1()
                        .border_color(t.divider)
                        .child(label("IN".into(), t.dim))
                        .child(label(source, t.text2)),
                );
            }
            let number = flow::build(mission).numbers.get(&f.id).copied();
            let brief = f
                .brief
                .as_ref()
                .map(|b| b.goal.trim())
                .filter(|s| !s.is_empty());
            let mut titles = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(size::RADIUS * z))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(GAP * z))
                        .when_some(number, |d, n| {
                            d.child(
                                div()
                                    .flex_none()
                                    .size(px(END_MARK * z))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(t.dim)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(size::SMALL * z))
                                    .text_color(t.text2)
                                    .child(n.to_string()),
                            )
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TITLE_SIZE * z))
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text)
                                .child(f.title.clone()),
                        ),
                )
                .when_some(brief, |d, brief| {
                    d.child(
                        div()
                            .text_size(px(size::SMALL * z))
                            .text_color(t.text2)
                            .line_clamp(2)
                            .child(brief.to_owned()),
                    )
                });
            if running {
                let (word, colour) = state_words(t, state);
                titles = titles.child(chip(z, word, colour));
            }
            let head = div()
                .h(px(board::step_head(f, running) * z))
                .flex_none()
                .px(px(PAD * z))
                .py(px(PAD * z))
                .border_b_1()
                .border_color(t.divider)
                .flex()
                .gap(px(SLOT_PAD * z))
                .child(titles)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(CHIP_PAD * z))
                        .child(chip(z, &f.kind, t.text2)),
                );
            face = face.child(head);
            let mut body = div()
                .h(px(board::body_h(&f.kind, f.slots.len()) * z))
                .flex_none()
                .px(px(SLOT_PAD * z))
                .py(px(board::BODY_PAD * z))
                .flex()
                .flex_col()
                .gap(px(board::SLOT_GAP * z));
            if board::pooled(&f.kind) {
                for slot in f.slots.iter().filter(|slot| slot.controller) {
                    body = body.child(slot_row(t, slot, z));
                }
                let mut pool = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(board::SLOT_GAP * z))
                    .pt(px(POOL_LABEL * z))
                    .pb(px((board::POOL_PAD - POOL_LABEL) * z))
                    .px(px(size::RADIUS * z))
                    .border_1()
                    .border_dashed()
                    .border_color(t.divider)
                    .rounded(px(GAP * z))
                    .child(
                        label(
                            if f.kind == "peer" { "PEERS" } else { "WORKERS" }.into(),
                            t.dim,
                        )
                        .absolute()
                        .top(px(-GAP * z))
                        .left(px(POOL_LABEL * z))
                        .px(px(CHIP_PAD * z))
                        .bg(t.surface),
                    );
                for slot in f.slots.iter().filter(|slot| !slot.controller) {
                    pool = pool.child(slot_row(t, slot, z));
                }
                body = body.child(pool);
            } else {
                for slot in &f.slots {
                    body = body.child(slot_row(t, slot, z));
                }
            }
            if f.slots.is_empty() {
                body = body.child(label("No slots".into(), t.dim));
            }
            face = face.child(body);
            for p in &f.outputs {
                let output = match state {
                    Some(crate::store::run_state::NodeState::Done) => "output ready",
                    Some(crate::store::run_state::NodeState::Running) => "running",
                    Some(crate::store::run_state::NodeState::Waiting) => "needs you",
                    Some(
                        crate::store::run_state::NodeState::Blocked
                        | crate::store::run_state::NodeState::Failed,
                    ) => "blocked",
                    _ => "no output yet",
                };
                face = face.child(
                    div()
                        .h(px(board::IO_ROW * z))
                        .flex_none()
                        .px(px(PAD * z))
                        .flex()
                        .items_center()
                        .gap(px(PAD * z))
                        .border_t_1()
                        .border_color(t.divider)
                        .child(label("OUT".into(), t.dim))
                        .child(
                            label(
                                if p.label.is_empty() {
                                    p.id.clone()
                                } else {
                                    p.label.clone()
                                },
                                t.text2,
                            )
                            .flex_1(),
                        )
                        .child(chip(z, output, state_words(t, state).1)),
                );
            }
            face
        }
        board::Kind::Input => {
            let Some(c) = mission.input_cards.iter().find(|c| c.id == card.id) else {
                return div();
            };
            let names: Vec<_> = c.inputs.iter().map(|i| i.name.as_str()).collect();
            let line = |text: String, colour: Hsla| {
                div()
                    .h(px(board::INPUT_LINE * z))
                    .px(px(POOL_LABEL * z))
                    .flex()
                    .items_center()
                    .text_size(px(size::SMALL * z))
                    .text_color(colour)
                    .child(div().truncate().child(text))
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(board::INPUT_HEAD * z))
                        .px(px(POOL_LABEL * z))
                        .pt(px(PAD * z))
                        .flex()
                        .flex_col()
                        .gap(px(CHIP_PAD * z))
                        .child(label("◆ INPUT".into(), t.accent))
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(TITLE_SIZE * z))
                                .truncate()
                                .child(c.title.clone()),
                        ),
                )
                .child(
                    div()
                        .h(px(board::INPUT_GOAL * z))
                        .px(px(POOL_LABEL * z))
                        .overflow_hidden()
                        .line_clamp(2)
                        .text_size(px(size::SMALL * z))
                        .text_color(t.text2)
                        .child(c.goal.clone()),
                )
                .when(!names.is_empty(), |d| {
                    d.child(line(format!("Inputs · {}", names.join(" · ")), t.text2))
                })
                .child(line("Human gates · notify me".into(), t.dim))
        }
        board::Kind::Gate => {
            let Some(g) = mission.gates.iter().find(|g| g.id == card.id) else {
                return div();
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .gap(px(PAD * z))
                .p(px(PAD * z))
                .child(
                    div()
                        .size(px(GATE_MARK * z))
                        .flex_none()
                        .rounded_full()
                        .border_2()
                        .border_color(state_words(t, state).1)
                        .bg(t.raised)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("⛩"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .truncate()
                                .child(format!(
                                    "{}{}",
                                    flow::build(mission)
                                        .numbers
                                        .get(&g.id)
                                        .map(|n| format!("{n}  "))
                                        .unwrap_or_default(),
                                    g.title
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(size::RADIUS * z))
                                .children(g.kinds.iter().map(|kind| {
                                    let word = match kind.as_str() {
                                        "formation" => "judge",
                                        "human" => "you",
                                        other => other,
                                    };
                                    label(word.to_owned(), t.text2)
                                        .px(px(size::RADIUS * z))
                                        .border_1()
                                        .border_color(t.divider)
                                })),
                        )
                        .child(
                            div()
                                .line_clamp(2)
                                .text_size(px(size::SMALL * z))
                                .child(g.criterion.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(label("✓ PASS".into(), parvi_app_kit::theme::rgb(t.ansi[2])))
                        .child(label("× FAIL".into(), t.error)),
                )
        }
        board::Kind::End => {
            let Some(e) = mission.ends.iter().find(|e| e.id == card.id) else {
                return div();
            };
            let colour = if e.outcome == "rejected" {
                t.error
            } else {
                parvi_app_kit::theme::rgb(t.ansi[2])
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .gap(px(PAD * z))
                .px(px(PAD * z))
                .child(
                    div()
                        .size(px(END_MARK * z))
                        .flex_none()
                        .rounded_full()
                        .border_2()
                        .border_color(colour)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(GAP * z)).rounded_full().bg(colour)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(label(format!("END · {}", e.outcome), colour))
                        .child(div().font_weight(FontWeight::BOLD).child(e.title.clone())),
                )
        }
    }
}

fn state_words(
    t: &parvi_app_kit::theme::Theme,
    state: Option<crate::store::run_state::NodeState>,
) -> (&'static str, Hsla) {
    use crate::store::run_state::NodeState;
    match state {
        Some(NodeState::Done) => ("done", parvi_app_kit::theme::rgb(t.ansi[2])),
        Some(NodeState::Running) => ("running", t.accent),
        Some(NodeState::Waiting) => ("needs you", t.accent),
        Some(NodeState::Blocked) => ("blocked", t.error),
        Some(NodeState::Failed) => ("failed", t.error),
        Some(NodeState::Canceled) => ("canceled", t.dim),
        None => ("not reached", t.dim),
    }
}
fn chip(z: f32, text: &str, colour: Hsla) -> Div {
    div()
        .flex_none()
        .px(px(GAP * z))
        .py(px(EDGE * z))
        .rounded(px(size::RADIUS * z))
        .bg(parvi_app_kit::theme::alpha(colour, CHIP_ALPHA))
        .text_size(px(size::SMALL * z))
        .text_color(colour)
        .child(text.to_uppercase())
}

fn slot_row(t: &parvi_app_kit::theme::Theme, slot: &parvi_missions_contract::Slot, z: f32) -> Div {
    let mark = harness_mark(&slot.harness, t.text, t.text2, z);
    let selector = format!(
        "canvas-seat-{}-{}-{}",
        crate::store::words::slot_label(slot),
        crate::store::words::role_name(&slot.agent_id),
        crate::store::words::harness_name(&slot.harness)
    );
    let ring = div()
        .size(px(RING_SIZE * z))
        .flex_none()
        .rounded_full()
        .border_2()
        .border_color(if slot.controller {
            parvi_app_kit::theme::rgb(t.ansi[3])
        } else {
            t.text2
        })
        .bg(t.raised)
        .flex()
        .items_center()
        .justify_center()
        .children(mark);
    let model = if slot.model.is_empty() {
        "default"
    } else {
        &slot.model
    };
    div()
        .h(px(board::SLOT_H * z))
        .debug_selector(move || selector.clone())
        .flex_none()
        .flex()
        .items_center()
        .gap(px(GAP * z))
        .px(px(size::RADIUS * z))
        .child(ring)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(size::SMALL * z))
                        .text_color(t.dim)
                        .truncate()
                        .child(crate::store::words::slot_label(slot).to_uppercase()),
                )
                .child(
                    div()
                        .text_size(px(size::TEXT * z))
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(crate::store::words::role_name(&slot.agent_id)),
                )
                .child(
                    div()
                        .text_size(px(size::SMALL * z))
                        .text_color(t.text2)
                        .truncate()
                        .child(format!(
                            "{} · {} · {}",
                            crate::store::words::harness_name(&slot.harness),
                            model,
                            slot.effort
                        )),
                ),
        )
}

const CLAUDE: &str = "M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z";
const CODEX: &str = "M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z";
thread_local! { static MARKS: std::cell::RefCell<std::collections::HashMap<String, std::sync::Arc<gpui::Image>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }
fn harness_mark(harness: &str, text: Hsla, secondary: Hsla, z: f32) -> Option<gpui::AnyElement> {
    let (path, fill) = match harness {
        "claude-code" | "claude" => (CLAUDE, "#D97757".to_owned()),
        "openai-codex" | "codex" => {
            let c = gpui::Rgba::from(text);
            (
                CODEX,
                format!(
                    "#{:02x}{:02x}{:02x}",
                    (c.r * 255.).round() as u8,
                    (c.g * 255.).round() as u8,
                    (c.b * 255.).round() as u8
                ),
            )
        }
        "shell" => {
            return Some(
                div()
                    .text_size(px(PAD * z))
                    .text_color(secondary)
                    .child(">_")
                    .into_any_element(),
            );
        }
        _ => return None,
    };
    let key = format!("{harness}:{fill}");
    let image=MARKS.with(|marks|marks.borrow_mut().entry(key).or_insert_with(|| {
        let svg=format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96" viewBox="0 0 24 24"><path fill="{fill}" fill-rule="evenodd" clip-rule="evenodd" d="{path}"/></svg>"#);
        std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg,svg.into_bytes()))
    }).clone());
    Some(
        gpui::img(image)
            .size(px(TITLE_SIZE * z))
            .flex_none()
            .into_any_element(),
    )
}
