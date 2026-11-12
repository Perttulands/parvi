//! The services page: read each node's catalog and control its services.
use crate::{
    services_store::{self, Change, Ended, NO_ANSWER, Services, Step},
    tokens,
};
use gpui::{
    App, Context, Div, FocusHandle, IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle,
    Styled, Window, canvas, div, prelude::*, px,
};
use parvi_app_kit::{
    Ask, Host, Page,
    host::{Request, Status},
    link::client::LinkState,
    theme::{self, size},
    widgets::kit,
};
use parvi_core::NodeId;
use std::{cell::Cell, rc::Rc, time::Duration};

const ACTION_WAIT: Duration = Duration::from_secs(50);
pub struct ServicesPage {
    host: Host,
    state: Services,
    shown: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    reveal: Rc<Cell<bool>>,
}
impl ServicesPage {
    pub fn new(host: Host, _: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Services::default(),
            shown: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            reveal: Rc::new(Cell::new(false)),
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
                s.state.nodes.entry(at).or_default().listed(
                    status,
                    &body,
                    parvi_app_kit::hub::now_ms(),
                );
                cx.notify();
            },
            cx,
        );
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
                cx.notify();
            },
            cx,
        );
        cx.notify();
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
            let order: Vec<_> = self
                .host
                .nodes(cx)
                .iter()
                .flat_map(|node| {
                    self.state
                        .nodes
                        .get(node.id.as_str())
                        .and_then(|n| n.list.as_ref())
                        .into_iter()
                        .flatten()
                        .map(|s| (node.id.to_string(), s.id.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();
            self.state.step(&order, step);
            self.reveal.set(true);
        } else if event.keystroke.key == "escape" && self.state.selected.is_some() {
            self.state.selected = None;
        } else if event.keystroke.key == "enter" {
            if let Some((node, id, change)) = self.state.enter() {
                self.act(&node, &id, change, cx);
            } else {
                return;
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
                        let clock = parvi_app_kit::clock::hhmm(parvi_app_kit::hub::now_ms());
                        if s.state
                            .nodes
                            .entry(at.clone())
                            .or_default()
                            .followed(&id, status, &body, last, &clock)
                        {
                            s.follow(at, id, read + 1, cx);
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
impl Page for ServicesPage {
    const ID: &'static str = "services";
    const TITLE: &'static str = "Services";
    const KEY: Option<char> = Some('E');
    const AREA: Option<&'static str> = Some("host");
    fn focus(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }
    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.read_all(cx);
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
        let mut body = div().flex().flex_col().gap(px(tokens::GAP));
        let (mut count, mut running, mut partial, mut failed) = (0, 0, 0, 0);
        let mut refreshed = None;
        for node in self.host.nodes(cx) {
            let id = node.id.as_str();
            let state = self.state.nodes.get(id);
            let mut room = div()
                .flex()
                .flex_col()
                .border(px(tokens::WALL))
                .border_color(t.identity_of(&node.device))
                .rounded(px(size::RADIUS))
                .bg(t.surface)
                .child(div().p(px(tokens::PAD)).child(kit::device_label(
                    &t,
                    &node.device,
                    &node.environment,
                )));
            if let Some(error) = state.and_then(|s| s.error.as_ref()) {
                room = room.child(
                    div()
                        .px(px(tokens::PAD))
                        .pb(px(tokens::PAD))
                        .child(kit::note(&t, error.clone(), Some(t.error))),
                );
            }
            if let Some(n) = state {
                if let Some(list) = &n.list {
                    count += list.len();
                    running += list.iter().filter(|s| s.state == "running").count();
                    partial += list.iter().filter(|s| s.state == "partial").count();
                    failed += list.iter().filter(|s| s.state == "failed").count();
                    refreshed = refreshed.max(n.read_at);
                    if list.is_empty() {
                        room =
                            room.child(div().px(px(tokens::PAD)).pb(px(tokens::PAD)).child(
                                kit::note(&t, "The catalog lists no services.", Some(t.dim)),
                            ));
                    }
                    for service in list {
                        let (node_id, service_id) = (id.to_owned(), service.id.clone());
                        let selected = self.state.selected.as_ref()
                            == Some(&(node_id.clone(), service_id.clone()));
                        let (pick_node, pick_id) = (node_id.clone(), service_id.clone());
                        let selector = format!("svc-row-{id}-{}", service.id);
                        let status_selector =
                            format!("svc-state-{id}-{}-{}", service.id, n.shown(service));
                        let failure_selector = n
                            .failures
                            .get(&service.id)
                            .map(|_| format!("svc-failure-{id}-{}", service.id));
                        let mut row = div()
                            .debug_selector(move || selector)
                            .id(gpui::SharedString::from(format!(
                                "svc-row-{id}-{}",
                                service.id
                            )))
                            .flex()
                            .items_center()
                            .gap(px(tokens::SMALL_GAP))
                            .p(px(tokens::PAD))
                            .border_t_1()
                            .border_color(t.divider)
                            .relative()
                            .when(selected, |d| {
                                d.bg(theme::alpha(t.accent, tokens::SELECTED))
                                    .child(reveal(self.scroll.clone(), self.reveal.clone()))
                            })
                            .on_click(cx.listener(move |s, _, window, cx| {
                                s.state.selected = Some((pick_node.clone(), pick_id.clone()));
                                window.focus(&s.focus, cx);
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(tokens::COL))
                                    .flex_none()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(service.name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(tokens::COL))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(tokens::SMALL_GAP))
                                    .child(glyph(&t, &service.state))
                                    .child(
                                        div()
                                            .debug_selector(move || status_selector)
                                            .child(n.shown(service).to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .when_some(failure_selector, |d, selector| {
                                        d.debug_selector(move || selector)
                                    })
                                    .child(
                                        n.failures
                                            .get(&service.id)
                                            .or_else(|| n.unsettled.get(&service.id))
                                            .cloned()
                                            .unwrap_or_else(|| service.description.clone()),
                                    ),
                            );
                        let mut primary = div().w(px(tokens::ACTION_COL)).flex_none();
                        let mut restart = div().w(px(tokens::ACTION_COL)).flex_none();
                        if service.controllable && !n.changing.contains_key(&service.id) {
                            if let Some(change) = services_store::primary(&service.state) {
                                let (node, row_id) = (node_id.clone(), service_id.clone());
                                let selector = format!("svc-{}-{id}-{}", change.verb(), service.id);
                                primary = primary.child(
                                    kit::button(
                                        &t,
                                        gpui::SharedString::from(format!(
                                            "svc-{}-{id}-{}",
                                            change.verb(),
                                            service.id
                                        )),
                                        if change == Change::Stop {
                                            "Stop"
                                        } else {
                                            "Start"
                                        },
                                        kit::Kind::Plain,
                                    )
                                    .debug_selector(move || selector)
                                    .on_click(cx.listener(
                                        move |s, _, _, cx| s.act(&node, &row_id, change, cx),
                                    )),
                                );
                            }
                            let (node, row_id) = (node_id.clone(), service_id.clone());
                            let selector = format!("svc-restart-{id}-{}", service.id);
                            restart = restart.child(
                                kit::button(
                                    &t,
                                    gpui::SharedString::from(selector.clone()),
                                    "Restart",
                                    kit::Kind::Plain,
                                )
                                .debug_selector(move || selector)
                                .on_click(cx.listener(
                                    move |s, _, _, cx| s.act(&node, &row_id, Change::Restart, cx),
                                )),
                            );
                        } else if !service.controllable {
                            primary = primary.child(kit::note(&t, "read-only", Some(t.dim)));
                        }
                        row = row.child(primary).child(restart);
                        let mut link_column =
                            div().w(px(tokens::LINK_COL)).flex_none().overflow_hidden();
                        if let Some(label) = services_store::link_label(&service.link) {
                            let link = service.link.clone();
                            let (host, port) = match label.rsplit_once(':') {
                                Some((host, port)) => (host.to_owned(), format!(":{port}")),
                                None => (label, String::new()),
                            };
                            link_column = link_column.child(
                                kit::button(
                                    &t,
                                    gpui::SharedString::from(format!(
                                        "svc-link-{id}-{}",
                                        service.id
                                    )),
                                    "",
                                    kit::Kind::Quiet,
                                )
                                .w_full()
                                .justify_start()
                                .whitespace_nowrap()
                                .tooltip(kit::tip(link.clone()))
                                // Links on one host differ in the port: a
                                // long host gives way, the port shows whole.
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_shrink_1()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(host),
                                )
                                .child(div().flex_none().child(port))
                                .on_click(move |_, _, cx| cx.open_url(&link)),
                            );
                        }
                        row = row.child(link_column);
                        let reference = services_store::reference(service);
                        let send_selector = format!("svc-send-{id}-{}", service.id);
                        row = row.child(
                            div().w(px(tokens::SEND_COL)).flex_none().child(
                                kit::button(
                                    &t,
                                    gpui::SharedString::from(format!(
                                        "svc-send-{id}-{}",
                                        service.id
                                    )),
                                    "Send",
                                    kit::Kind::Quiet,
                                )
                                .debug_selector(move || send_selector)
                                .on_click(cx.listener(
                                    move |s, _, _, cx| {
                                        let _ = s.host.ask(
                                            Ask::Send {
                                                reference: reference.clone(),
                                                target: None,
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            ),
                        );
                        room = room.child(row);
                    }
                } else if n.error.is_none() {
                    room = room.child(kit::note(&t, "Reading services…", Some(t.dim)));
                }
            }
            body = body.child(room);
        }
        div()
            .id("services")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| s.key(e, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                kit::page_bar(&t)
                    .child(kit::page_title(&t, "Services"))
                    .child(div().flex_1())
                    .child(
                        kit::button(&t, "services-refresh", "Refresh", kit::Kind::Quiet)
                            .debug_selector(|| "services-refresh".into())
                            .on_click(cx.listener(|s, _, _, cx| s.read_all(cx))),
                    ),
            )
            .child(
                div()
                    .id("services-list")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(tokens::PAD))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .gap(px(tokens::GAP))
                    .p(px(tokens::PAD))
                    .border_t_1()
                    .border_color(t.divider)
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .child(format!("{count} services"))
                    .child(format!("{running} running"))
                    .when(partial > 0, |d| d.child(format!("{partial} partial")))
                    .when(failed > 0, |d| {
                        d.child(div().text_color(t.error).child(format!("{failed} failed")))
                    })
                    .when_some(refreshed, |d, at| {
                        d.child(format!("refreshed {}", parvi_app_kit::clock::hhmm(at)))
                    }),
            )
    }
}

fn glyph(t: &theme::Theme, state: &str) -> Div {
    let dot = div()
        .size(px(tokens::GLYPH))
        .flex_none()
        .rounded_full()
        .overflow_hidden();
    match state {
        "running" => dot.bg(t.accent),
        "starting" => dot.bg(theme::alpha(t.accent, tokens::GRID_MID)),
        "partial" => dot
            .border_1()
            .border_color(t.accent)
            .child(div().w(px(tokens::GLYPH / 2.)).h_full().bg(t.accent)),
        "failed" => dot.bg(t.error),
        "unknown" => dot.border_1().border_dashed().border_color(t.dim),
        "stopping" => dot.border_1().border_color(t.accent),
        _ => dot.border_1().border_color(t.dim),
    }
}

/// Reveal the chosen keyboard row without moving a row already visible.
fn reveal(scroll: ScrollHandle, pending: Rc<Cell<bool>>) -> impl IntoElement {
    canvas(
        move |row, window, cx| {
            if !pending.take() {
                return;
            }
            let view = scroll.bounds();
            let by = into_view(
                (row.top().into(), row.bottom().into()),
                (view.top().into(), view.bottom().into()),
            );
            if by != 0. {
                let at = scroll.offset();
                scroll.set_offset(gpui::point(at.x, at.y + px(by)));
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}
fn into_view(row: (f32, f32), view: (f32, f32)) -> f32 {
    if row.0 < view.0 {
        view.0 - row.0
    } else if row.1 > view.1 {
        (view.1 - row.1).max(view.0 - row.0)
    } else {
        0.
    }
}
#[cfg(test)]
mod tests {
    use super::into_view;
    #[test]
    fn a_chosen_row_scrolls_just_into_view() {
        let view = (100., 400.);
        assert_eq!(into_view((150., 180.), view), 0.);
        assert_eq!(into_view((60., 90.), view), 40.);
        assert_eq!(into_view((390., 420.), view), -20.);
        assert_eq!(into_view((350., 800.), view), -250.);
    }
}
