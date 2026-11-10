//! What the shell draws around and over the page: the tab bar, the status
//! line with its toast, the layers and menus on the stack, and the keys a
//! chord was typed with.

use crate::shell::Shell;
use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, Window, anchored, deferred, div,
    point, prelude::*, px,
};
use parvi_app_kit::clock;
use parvi_app_kit::host::{Cover, Status};
use parvi_app_kit::hub::{Surface, Toast};
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::menu::{Menu, MenuRow, Pick};
use parvi_app_kit::theme::{self, Theme, alpha, motion, size, theme};
use parvi_app_kit::widgets::{boundary, kit};
use std::time::Duration;

/// A tab's title, spaced out in capitals as CHROTE's tabs read.
fn spaced(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.to_uppercase().chars().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

impl Shell {
    fn render_tab_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let off = self.hub.read(cx).areas_off();
        let mut bar = div()
            .flex_none()
            .h(px(size::TAB_BAR))
            .flex()
            .items_stretch()
            .bg(t.surface)
            .border_b_1()
            .border_color(t.divider);
        for (i, page) in self.pages.iter().enumerate() {
            if page.meta.area.is_some_and(|a| off.iter().any(|o| o == a)) {
                continue;
            }
            let active = self.shown == Some(i);
            if let Some(tabs) = page.tabs(active, window, cx) {
                bar = bar.child(tabs);
                continue;
            }
            bar = bar.child(
                div()
                    .id(("tab", i))
                    .debug_selector(|| format!("tab-{}", page.meta.id))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(size::TEXT))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active { t.text } else { t.text2 })
                    .when(active, |d| d.border_b_2().border_color(t.accent))
                    .hover(|d| d.text_color(t.text))
                    .child(spaced(page.meta.title))
                    .on_click(cx.listener(move |s, _, window, cx| s.show(i, window, cx))),
            );
        }
        let on = self.hub.read(cx).settings.keys;
        bar.child(
            div()
                .id("keys-toggle")
                .ml_auto()
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(if on { t.text2 } else { t.dim })
                .hover(|d| d.text_color(t.text))
                .child(if on { "Keys on" } else { "Keys off" })
                .on_click(cx.listener(|s, _, _, cx| s.set_keys(None, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|s, e: &MouseDownEvent, _, cx| {
                        let shell = cx.entity().downgrade();
                        let shell2 = shell.clone();
                        let on = s.hub.read(cx).settings.keys;
                        let rows = vec![
                            MenuRow::new("Keybindings", move |window, cx| {
                                let _ = shell.update(cx, |s, cx| s.toggle_keys(window, cx));
                            })
                            .chord("Alt+K"),
                            MenuRow::rule(),
                            MenuRow::new(if on { "Keys off" } else { "Keys on" }, move |_, cx| {
                                let _ = shell2.update(cx, |s, cx| s.set_keys(Some(!on), cx));
                            }),
                        ];
                        s.host.menu(rows, e.position, cx);
                    }),
                ),
        )
    }

    /// Turns the keys on or off; `None` flips them.
    fn set_keys(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        self.hub.update(cx, |h, cx| {
            let on = on.unwrap_or(!h.settings.keys);
            h.settings.keys = on;
            h.save(cx);
            h.toast(if on { "Keys on" } else { "Keys off" }, cx);
        });
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let hub = self.hub.read(cx);
        let mut line = div()
            .flex_none()
            .h(px(size::STATUS))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(t.surface)
            .border_t_1()
            .border_color(t.divider)
            .text_size(px(12.))
            .text_color(t.text2);
        if let Some(Status {
            at,
            text,
            failure,
            action,
        }) = hub.status.clone()
        {
            line = line
                .child(div().flex_none().text_color(t.dim).child(clock::at(at)))
                .child(
                    div()
                        .id("status-text")
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(if failure { t.error } else { t.text2 })
                        .child(text),
                );
            if let Some((label, run)) = action {
                line = line.child(
                    div()
                        .id("status-action")
                        .flex_none()
                        .px(px(8.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t.divider)
                        .cursor_pointer()
                        .hover(|d| d.border_color(t.accent))
                        .child(label)
                        .on_click(move |_, window, cx| {
                            let run = run.clone();
                            window.defer(cx, move |window, cx| run(window, cx));
                        }),
                );
            }
        }
        let here = hub.fleet.here();
        let (word, colour) = match here.state {
            LinkState::Open => ("connected", t.text2),
            LinkState::Connecting => ("connecting…", t.dim),
            LinkState::Lost => ("offline, retrying", t.error),
            LinkState::Refused => ("refused", t.error),
        };
        let node = here
            .hello
            .as_ref()
            .map(|h| format!("{} · {}", h.device, h.environment))
            .unwrap_or_else(|| hub.fleet.base().to_string());
        line.child(div().flex_1())
            .children(hub.toast.as_ref().map(|toast| render_toast(&t, toast)))
            .child(
                div()
                    .id("connection")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(6.)).rounded_full().bg(colour))
                    .child(div().text_color(t.dim).child(node))
                    .child(div().text_color(colour).child(word)),
            )
    }

    /// The page shown, inside its error boundary: a page that fails to
    /// draw shows why, with Try again, and nothing else stops.
    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(i) = self.shown else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(kit::note(&t, "No pages are registered.", Some(t.dim)))
                .into_any_element();
        };
        let meta = self.pages[i].meta;
        if let Some(why) = boundary::take_late() {
            self.failed.insert(meta.id, why);
        }
        if !self.failed.contains_key(meta.id) {
            let page = &self.pages[i];
            match boundary::guard(|| page.draw(window, cx)) {
                Ok(el) => return el,
                Err(why) => {
                    self.failed.insert(meta.id, why);
                }
            }
        }
        let why = self.failed.get(meta.id).cloned().unwrap_or_default();
        let retry = kit::button(&t, "retry", "Try again", kit::Kind::Primary).on_click(
            cx.listener(move |s, _, _, cx| {
                s.failed.remove(meta.id);
                cx.notify();
            }),
        );
        boundary::failed(&t, meta.title, &why, retry)
    }

    /// The docks pages keep open beside every page, at the window's side;
    /// each view sets its own width.
    fn render_docks(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let docks = self.hub.read(cx).docks();
        docks
            .into_iter()
            .map(|(page, view)| {
                div()
                    .debug_selector(move || format!("dock-{page}"))
                    .flex_none()
                    .h_full()
                    .border_l_1()
                    .border_color(t.divider)
                    .child(view)
                    .into_any_element()
            })
            .collect()
    }

    /// The layers on the stack that cover `cover`, bottom first.
    fn render_layers(&mut self, cover: Cover, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let stack: Vec<Surface> = self.hub.read(cx).stack.iter().cloned().collect();
        let mut out = vec![];
        for (depth, s) in stack.into_iter().enumerate() {
            match s {
                Surface::Layer(l) if l.cover == cover => out.push(
                    deferred(div().absolute().inset_0().child(l.view))
                        .with_priority(5 + depth)
                        .into_any_element(),
                ),
                Surface::Menu(m) if cover == Cover::Window => {
                    out.push(self.render_menu(depth, &m, cx))
                }
                _ => {}
            }
        }
        out
    }

    fn render_menu(&mut self, depth: usize, menu: &Menu, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let mut list = kit::panel(&t)
            .min_w(px(size::MENU))
            .py(px(4.))
            .flex()
            .flex_col();
        for (i, row) in menu.rows.iter().enumerate() {
            if row.is_rule() {
                list = list.child(div().my(px(4.)).child(kit::rule(&t)));
                continue;
            }
            let armed = menu.armed == Some(i);
            let label = if armed {
                format!("{}? Click again", row.label)
            } else {
                row.label.clone()
            };
            let inert = row.inert();
            let colour = if inert {
                t.dim
            } else if row.danger {
                t.error
            } else {
                t.text
            };
            let line = div()
                .h(px(size::ROW))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().w(px(12.)).text_color(t.accent).child(if row.checked {
                    "✓"
                } else {
                    ""
                }))
                .child(div().flex_1().child(label))
                .children(
                    row.chord
                        .clone()
                        .map(|c| div().text_size(px(size::SMALL)).text_color(t.dim).child(c)),
                );
            list = list.child(
                div()
                    .id(("menu", depth * 100 + i))
                    .debug_selector(|| format!("menu-row-{i}"))
                    .px(px(12.))
                    .flex()
                    .flex_col()
                    .text_color(colour)
                    .when(!inert, |d| {
                        d.cursor_pointer().hover(|d| d.bg(t.accent_light()))
                    })
                    .when(armed, |d| d.bg(alpha(t.error, 0.15)))
                    .child(line)
                    .children(row.hint.clone().map(|h| {
                        div()
                            .max_w(px(280.))
                            .pl(px(24.))
                            .pb(px(6.))
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .child(h)
                    }))
                    .on_click(cx.listener(move |s, _, window, cx| s.menu_pick(i, window, cx))),
            );
        }
        let close = cx.listener(|s, _, _, cx| s.hub.update(cx, |h, cx| h.close_menu(cx)));
        let close_right = cx.listener(|s, _, _, cx| s.hub.update(cx, |h, cx| h.close_menu(cx)));
        deferred(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, close)
                .on_mouse_down(MouseButton::Right, close_right)
                .child(
                    anchored()
                        .position(point(menu.at.x, menu.at.y))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(list),
                        ),
                ),
        )
        .with_priority(20)
        .into_any_element()
    }

    /// A click on row `i` of the open menu.
    fn menu_pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pick = self.hub.update(cx, |h, cx| {
            let pick = match h.stack.top_mut() {
                Some(Surface::Menu(m)) => m.pick(i),
                _ => Pick::Nothing,
            };
            match pick {
                Pick::Run(_) => h.close_menu(cx),
                _ => cx.notify(),
            }
            pick
        });
        if let Pick::Run(run) = pick {
            window.defer(cx, move |window, cx| run(window, cx));
        }
    }

    fn render_echo(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx);
        let caps = self.echo.as_ref()?;
        Some(
            deferred(
                div()
                    .absolute()
                    .bottom(px(size::STATUS + 20.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        kit::panel(t)
                            .px(px(10.))
                            .py(px(6.))
                            .child(kit::caps(t, caps)),
                    ),
            )
            .with_priority(30)
            .into_any_element(),
        )
    }
}

/// A toast sits in the status line, left of the connection, where it
/// covers nothing.
fn render_toast(t: &Theme, toast: &Toast) -> AnyElement {
    let fade_at = Duration::from_millis(motion::TOAST_IN + motion::TOAST_SHOWN);
    let opacity = if toast.at.elapsed() >= fade_at {
        0.0
    } else {
        1.0
    };
    div()
        .id("toast")
        .flex_none()
        .opacity(opacity)
        .px(px(10.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .border_1()
        .border_color(t.divider)
        .bg(t.raised)
        .text_color(t.text)
        .child(toast.text.clone())
        .into_any_element()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        self.view_changed(cx);
        let page = self.render_page(window, cx);
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(theme::FAMILY)
            .text_size(px(size::TEXT))
            .capture_key_down(cx.listener(Self::capture_key))
            .child(self.render_tab_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .id("page")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .relative()
                            .child(page)
                            .children(self.render_layers(Cover::Page, cx)),
                    )
                    .children(self.render_docks(&t, cx)),
            )
            .child(self.render_status(cx))
            .children(self.render_layers(Cover::Window, cx))
            .children(self.render_echo(cx))
    }
}
