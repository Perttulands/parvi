//! The Sessions rail and Peek. The rail lists every session the node
//! follows, by device and environment, then in CHROTE's name groups; a
//! click peeks, Ctrl+ and Shift+click pick rows to kill together, a drag
//! puts the session in a window, the menu does the rest. Peek shows a
//! session without sizing it.

use super::page::{Act, MenuItem, Terminals};
use super::place;
use super::session_acts::Gesture;
use super::tiles::{DragPreview, Dragged};
use crate::store::devices;
use crate::store::sessions::{self, Key};
use crate::store::workspace::LinkState;
use crate::term::paint::{self, Look};
use crate::theme::{Theme, size, theme};
use gpui::{
    AnyElement, ContentMask, Context, FontWeight, MouseButton, MouseDownEvent, Window, canvas, div,
    prelude::*, px,
};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_terminals_contract::{AgentEvent, EventKind, Session};

impl Terminals {
    pub fn render_rail(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let header = div()
            .flex_none()
            .h(px(size::STRIP))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(t.divider)
            .child(
                div()
                    .mr_auto()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("SESSIONS"),
            )
            .child(
                kit::button(&t, "rail-new", "New", Kind::Quiet).on_click(cx.listener(
                    |s, _, window, cx| {
                        s.run(super::chords::Action::Launcher, window, cx);
                    },
                )),
            )
            .child(
                kit::button(&t, "rail-close", "Close", Kind::Quiet).on_click(cx.listener(
                    |s, _, window, cx| {
                        s.run(super::chords::Action::Sessions, window, cx);
                    },
                )),
            );
        let search = div()
            .flex_none()
            .p(px(8.))
            .border_b_1()
            .border_color(t.divider)
            .child(self.rail_search.clone());
        let query = self.rail_search.read(cx).text.clone();
        let fleet = &self.fleet;
        let devices =
            devices::devices(
                &self.places,
                &self.settings.kept,
                fleet.here_id(),
                |n| match fleet.get(n) {
                    Some(c) => (
                        c.state == LinkState::Open,
                        c.known.then_some(&c.sessions[..]),
                    ),
                    None => (false, None),
                },
            );
        let mut list = div()
            .id("rail-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(8.))
            .flex()
            .flex_col();
        // The app's link says why the serving node does not answer.
        let here = self.fleet.here_id().and_then(|n| self.fleet.get(n));
        let note = match here.map(|c| c.state) {
            Some(LinkState::Open) => None,
            Some(LinkState::Lost | LinkState::Refused) => {
                Some("The node does not answer. Trying again by itself.".to_string())
            }
            Some(LinkState::Connecting) | None => Some("Connecting…".to_string()),
        };
        let quiet = |text: String| {
            div()
                .px(px(16.))
                .pb(px(8.))
                .text_size(px(12.))
                .text_color(t.dim)
                .child(text)
        };
        // Once devices are listed, an away one says so in its own place and
        // the status line names it: nothing above them moves.
        if let Some(n) = note.filter(|_| devices.is_empty()) {
            list = list.child(quiet(n));
        }
        let mut shown = 0;
        // What a pick counts from: the rows as drawn, collapsed groups out.
        self.rail_shown.clear();
        let mut existing = std::collections::HashSet::new();
        for d in &devices {
            for n in &d.nodes {
                existing.extend(n.sessions.iter().map(|s| Key::of(s).text()));
            }
        }
        self.pick.retain_existing(|k| existing.contains(k));
        for d in &devices {
            if !devices::visible_in_search(d, &query) {
                continue;
            }
            let mut block = div()
                .flex()
                .flex_col()
                .pb(px(6.))
                .child(self.device_heading(&t, &d.device, d.live));
            for n in &d.nodes {
                let found = sessions::search(&n.sessions, &query);
                if found.is_empty() && !query.trim().is_empty() {
                    continue;
                }
                shown += found.len();
                block =
                    block.child(self.environment_heading(&t, &n.environment, n.live || !d.live));
                for (group, members) in sessions::groups(&n.sessions, &found) {
                    if !self.settings.collapsed_groups.contains(&group) {
                        self.rail_shown.extend(members.iter().map(|s| Key::of(s)));
                    }
                    block = block.child(self.group(&t, &group, &members, &d.device, n.live, cx));
                }
            }
            list = list.child(block.when(!d.live, |b| b.opacity(crate::theme::fade::DIMMED)));
        }
        if shown == 0 && here.is_some_and(|c| c.known) {
            list = list.child(quiet(if query.trim().is_empty() {
                "No sessions. New starts one.".to_string()
            } else {
                "No session matches.".to_string()
            }));
        }
        div()
            .track_focus(&self.rail_focus)
            .on_key_down(cx.listener(Self::rail_key))
            .flex_none()
            .w(px(size::RAIL))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.divider)
            .child(header)
            .child(search)
            .child(list)
            .into_any_element()
    }

    /// A device: its plate, and "waiting" while none of it answers.
    fn device_heading(&self, t: &Theme, device: &str, live: bool) -> AnyElement {
        div()
            .px(px(12.))
            .pb(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().child(place::plate(device)))
            .child(waiting_word(t, !live))
            .into_any_element()
    }

    /// An environment of a device; "waiting" when it alone is away.
    fn environment_heading(&self, t: &Theme, environment: &str, quiet: bool) -> AnyElement {
        div()
            .pl(px(22.))
            .pr(px(12.))
            .pb(px(2.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().child(place::strip(t, environment, 60.)))
            .child(waiting_word(t, !quiet))
            .into_any_element()
    }

    fn group(
        &self,
        t: &Theme,
        name: &str,
        members: &[&Session],
        device: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.settings.collapsed_groups.iter().any(|g| g == name);
        let g = name.to_string();
        let header = div()
            .id(gpui::SharedString::from(format!("group-{name}")))
            .px(px(12.))
            .pt(px(8.))
            .pb(px(3.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(t.text2)
            .hover(|s| s.text_color(t.text))
            .child(
                div()
                    .w(px(10.))
                    .text_size(px(8.))
                    .text_color(t.dim)
                    .child(if collapsed { "▶" } else { "▼" }),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name.to_uppercase()),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(t.dim)
                    .child(members.len().to_string()),
            )
            .on_click(cx.listener(move |s, _, _, cx| {
                let list = &mut s.settings.collapsed_groups;
                if let Some(i) = list.iter().position(|x| *x == g) {
                    list.remove(i);
                } else {
                    list.push(g.clone());
                }
                s.save(cx);
                cx.notify();
            }));
        let mut items = div().pl(px(12.)).flex().flex_col();
        if !collapsed {
            for s in members {
                items = items.child(self.row(t, s, device, live, cx));
            }
        }
        div()
            .mb(px(4.))
            .child(header)
            .child(items)
            .into_any_element()
    }

    fn row(
        &self,
        t: &Theme,
        s: &Session,
        device: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Key::of(s);
        let shown = self.settings.workspace.bound(&key);
        let in_focused = shown.is_some_and(|(tab, w)| {
            tab == self.settings.workspace.active && w == self.settings.workspace.focus
        });
        let picked = self.pick.has(&key.text());
        let waiting = sessions::waiting(s);
        let mark = self.marked(s).cloned();
        let activity = if live {
            sessions::activity(s, super::page::now_ms())
        } else {
            "waiting".to_string()
        };
        let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
        let name = s.name.clone();
        let id = gpui::SharedString::from(format!("row-{}", key.text()));
        let selector = id.to_string();
        place::edge(div().id(id).debug_selector(move || selector), device)
            .min_h(px(size::ROW))
            .pl(px(2.))
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_color(if shown.is_some() { t.text } else { t.text2 })
            // CHROTE: the focused tile's row on the raised surface, a picked
            // row on the accent wash; both carry the accent bar, inside the
            // device edge and over the row, so nothing in it moves.
            .relative()
            .when(in_focused || picked, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(t.accent),
                )
            })
            .when(in_focused, |d| d.bg(t.raised))
            .when(picked, |d| d.bg(t.accent_soft()))
            .hover(|d| d.bg(t.accent_light()))
            // The news dot keeps its room on every row, so the names stand
            // in one column; what a session runs is said on the right.
            .child(
                div()
                    .flex_none()
                    .w(px(size::DOT_SLOT))
                    .flex()
                    .justify_center()
                    .map(|d| match &mark {
                        Some(e) => d.child(event_dot(t, &key, e)),
                        None if waiting.is_some() => d.child(kit::waiting_dot(t)),
                        None => d,
                    }),
            )
            .when_some(s.user.clone(), |d, u| d.child(kit::user_square(t, &u)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(3.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(name_label(&s.name))
                    .children(mark.as_ref().filter(|e| !e.summary.is_empty()).map(|e| {
                        div()
                            .text_size(px(11.))
                            .text_color(t.dim)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(e.summary.clone())
                    })),
            )
            .children(running_mark(t, s, cx).filter(
                |_| !matches!(sessions::running(s), sessions::Running::Command(c) if c == s.name),
            ))
            .children(
                glyphs(s)
                    .into_iter()
                    .enumerate()
                    .map(|(n, (mark, why))| glyph(t, format!("{}-{n}", key.text()), mark, why)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(if activity == "active" {
                        t.accent
                    } else {
                        t.dim
                    })
                    .child(activity),
            )
            .child(
                div()
                    .id(gpui::SharedString::from(format!("rowmenu-{}", key.text())))
                    .flex_none()
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .text_color(t.dim)
                    .hover(|d| d.text_color(t.text))
                    .child("…")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |sh, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let items = sh.menu_for(&k1);
                            sh.menu(items, e.position, window, cx);
                        }),
                    ),
            )
            .on_click(cx.listener(move |sh, e: &gpui::ClickEvent, window, cx| {
                sh.rail_click(&k2, Gesture::of(&e.modifiers()), window, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |sh, e: &MouseDownEvent, window, cx| {
                    let items = sh.menu_for(&k3);
                    sh.menu(items, e.position, window, cx);
                }),
            )
            .on_drag(
                Dragged {
                    key: key.clone(),
                    from: None,
                },
                move |_, _, _, cx| cx.new(|_| DragPreview::named(name.clone())),
            )
            .into_any_element()
    }

    /// A picked row's menu is the pick's, as in CHROTE; any other row's
    /// is its own.
    fn menu_for(&self, key: &Key) -> Vec<MenuItem> {
        if self.pick.has(&key.text()) && !self.pick_targets().is_empty() {
            self.pick_menu()
        } else {
            self.row_menu(key)
        }
    }

    pub fn row_menu(&self, key: &Key) -> Vec<MenuItem> {
        let mut items = vec![
            MenuItem::new("Peek", Act::Peek(key.clone())),
            MenuItem::new("Send…", Act::Send(key.clone())).chord("Alt+S"),
            MenuItem::new("Rename…", Act::Rename(key.clone())),
            self.copy_row(key),
        ];
        let n = self.settings.workspace.windows().len();
        for w in 0..n {
            items.push(MenuItem::new(
                format!("Attach to window {}", w + 1),
                Act::Attach(key.clone(), w),
            ));
        }
        if self.settings.workspace.bound(key).is_some() {
            items.push(MenuItem::new("Unassign", Act::Unassign(key.clone())));
        }
        items.push(MenuItem::rule());
        items.push(MenuItem::new("Kill session", Act::Kill(key.clone())).danger());
        items
    }

    pub fn render_peek(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let p = self.peek.as_ref()?;
        let key = p.key.clone();
        let session = self.fleet.session(&key).cloned();
        let font = self.settings.font_size;
        let (screen, from_history) = match (&p.history, p.pane) {
            (Some(h), _) => (Some(h.clone()), true),
            (None, Some(pane)) => (self.fleet.screen(&key.node, pane), false),
            _ => (None, false),
        };
        // A device that went keeps its last screen here, saying since when.
        let away = (!self
            .fleet
            .get(&key.node)
            .is_some_and(|c| c.state == LinkState::Open))
        .then(|| self.away_since(&key.node));
        let word = match (&away, from_history) {
            (_, true) => format!("history, line {}", p.line.unwrap_or(0) + 1),
            (Some(Some(at)), false) => format!("offline since {at}, last screen"),
            (Some(None), false) => "offline, last screen".to_string(),
            (None, false) => "peek".to_string(),
        };
        let device = self
            .where_is(&key.node)
            .map(|p| p.device.to_uppercase())
            .unwrap_or_else(|| "the node".into());
        let (k1, k2) = (key.clone(), key.clone());
        let header = div()
            .flex_none()
            .h(px(size::PEEK_HEAD))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_b_1()
            .border_color(t.divider)
            .children(
                self.where_is(&key.node)
                    .map(|p| place::passport(&t, &p.device, &p.environment)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(key.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(t.dim)
                    .child(word),
            )
            .child(
                kit::button(&t, "peek-attach", "Attach", Kind::Plain)
                    .debug_selector(|| "peek-attach".into())
                    .on_click(cx.listener(move |s, _, window, cx| {
                        s.close(super::surface::PEEK, cx);
                        s.place(k1.clone(), window, cx);
                    })),
            )
            .child(
                kit::button(&t, "peek-send", "Send", Kind::Primary).on_click(cx.listener(
                    move |s, _, window, cx| {
                        s.close(super::surface::PEEK, cx);
                        s.open_send(Some(k2.clone()), window, cx);
                    },
                )),
            )
            .child(
                kit::button(&t, "peek-close", "Close  Esc", Kind::Quiet)
                    .on_click(cx.listener(|s, _, _, cx| s.close(super::surface::PEEK, cx))),
            );
        let body: AnyElement = match (screen, session.is_some(), p.loading) {
            (_, false, _) => kit::note(&t, format!("{} is not running.", key.name), None)
                .p(px(16.))
                .into_any_element(),
            (_, true, true) => kit::note(&t, "Reading the history…", None)
                .p(px(16.))
                .into_any_element(),
            (Some(screen), true, false) => {
                let tt = t.clone();
                canvas(
                    |_, _, _| (),
                    move |b, _, window, cx| {
                        let s = screen.lock();
                        let m = paint::fit(window, b.size, s.cols, s.rows, font);
                        let o = paint::origin(b, m, s.rows_to_show());
                        window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                            paint::paint(
                                &s,
                                m,
                                o,
                                Look {
                                    theme: &tt,
                                    cursor: !from_history,
                                    focused: false,
                                    link: &[],
                                },
                                window,
                                cx,
                            );
                        });
                    },
                )
                .size_full()
                .into_any_element()
            }
            (None, true, false) => kit::note(
                &t,
                match away {
                    Some(_) => format!("Waiting for {device}; it comes back by itself."),
                    None => "Connecting…".to_string(),
                },
                None,
            )
            .p(px(16.))
            .into_any_element(),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&self.peek_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(crate::theme::alpha(t.bg, 0.5))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|s, _, _, cx| s.close(super::surface::PEEK, cx)),
                )
                .child(
                    div()
                        .id("peek")
                        .w(gpui::relative(size::PEEK_SHARE))
                        .h(gpui::relative(size::PEEK_SHARE))
                        .flex()
                        .flex_col()
                        .bg(t.surface)
                        .border_1()
                        .border_color(t.divider)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_scroll(cx.listener(|s, e: &gpui::ScrollEvent, _, cx| {
                            if let Some(h) = s.peek.as_ref().and_then(|p| p.history.clone()) {
                                let n = match e.delta {
                                    gpui::ScrollDelta::Lines(l) => (l.y * 3.).round() as i32,
                                    gpui::ScrollDelta::Pixels(p) => {
                                        (f32::from(p.y) / 16.).round() as i32
                                    }
                                };
                                h.lock()
                                    .term
                                    .scroll_display(alacritty_terminal::grid::Scroll::Delta(n));
                                cx.notify();
                            }
                        }))
                        .child(header)
                        .child(div().flex_1().min_h_0().p(px(6.)).bg(t.term_bg).child(body)),
                )
                .into_any_element(),
        )
    }
}

impl DragPreview {
    pub fn named(name: String) -> Self {
        DragPreview { name }
    }
}

/// The quiet word an away device or environment carries. Its room is kept
/// while it answers, so the plate does not move when it goes or comes back.
fn waiting_word(t: &Theme, away: bool) -> gpui::Div {
    div()
        .flex_none()
        .when(!away, |d| d.opacity(0.))
        .text_size(px(10.))
        .text_color(t.dim)
        .child("waiting")
}

/// What the session runs, as CHROTE's row shows it: the harness mark, the
/// program's name, or nothing for a shell.
pub(super) fn running_mark(t: &Theme, s: &Session, cx: &gpui::App) -> Option<AnyElement> {
    match sessions::running(s) {
        sessions::Running::Harness(h) => crate::harness::harness_mark(h, 14., cx),
        sessions::Running::Command(c) => Some(
            div()
                .flex_none()
                .max_w(px(64.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(10.))
                .text_color(t.dim)
                .child(c.to_string())
                .into_any_element(),
        ),
        sessions::Running::Nothing => None,
    }
}

/// A session's name as CHROTE's `SessionLabel` draws it: the head clipped
/// with an ellipsis when the row is narrow, the tail always whole.
fn name_label(name: &str) -> gpui::Div {
    clipped_name(&sessions::rail_name(name, size::RAIL_NAME_COLUMNS))
}

/// A name that gives way from its head when its room is short, its tail
/// (what tells sessions apart) always whole.
pub(super) fn clipped_name(label: &str) -> gpui::Div {
    let (head, tail) = sessions::split_name(label);
    div()
        .min_w_0()
        .flex()
        .when(!head.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(head.to_string()),
            )
        })
        .child(div().flex_none().child(tail.to_string()))
}

/// The badges as CHROTE's row has them: one glyph each, windows and panes
/// sharing one.
fn glyphs(s: &Session) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = vec![];
    for b in sessions::badges(s) {
        match out.iter_mut().find(|(m, _)| *m == b.mark) {
            Some((_, why)) => {
                why.push(' ');
                why.push_str(&b.why);
            }
            None => out.push((b.mark, b.why)),
        }
    }
    out
}

/// A badge as one glyph, its explanation on hover.
/// An agent's report, while it is news: the accent dot, saying which.
fn event_dot(t: &Theme, key: &Key, e: &AgentEvent) -> AnyElement {
    let why = match e.kind {
        EventKind::Finished => "Its agent finished",
        EventKind::NeedsInput => "Its agent needs input",
    };
    div()
        .id(gpui::SharedString::from(format!("event-{}", key.text())))
        .flex_none()
        .child(kit::waiting_dot(t))
        .tooltip(move |_, cx| cx.new(|_| kit::Tip(why.into())).into())
        .into_any_element()
}

fn glyph(t: &Theme, id: String, mark: &'static str, why: String) -> AnyElement {
    div()
        .id(gpui::SharedString::from(format!("glyph-{id}")))
        .flex_none()
        .text_size(px(11.))
        .text_color(t.text2)
        .tooltip(move |_, cx| cx.new(|_| kit::Tip(why.clone().into())).into())
        .child(mark)
        .into_any_element()
}
