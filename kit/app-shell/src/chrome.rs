//! What the shell draws around and over the page: the tab bar, the status
//! line with its toast, the layers and menus on the stack, and the keys a
//! chord was typed with.

use crate::shell::Shell;
use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, Window, anchored, deferred, div, point,
    prelude::*, px,
};
use parvi_app_kit::host::{Cover, Status};
use parvi_app_kit::hub::{Surface, Toast};
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::menu::{Menu, MenuRow, Pick};
use parvi_app_kit::theme::{self, Theme, alpha, motion, size, theme};
use parvi_app_kit::widgets::{boundary, kit};
use std::time::Duration;

/// The padding of the words at the bar's right end (Keys, a link that is
/// not open).
const END_PAD: f32 = 12.;
/// Room kept on the bar for each page that draws its own tabs (the
/// terminals' workspace tabs and their `+`): they shrink and scroll to it.
const OWN_TABS_LEAST: f32 = 130.;
/// The tab that holds the pages the bar has no room for.
const MORE: &str = "More";
/// How long words that are not a failure stay up.
const NOTICE_SHOWN_S: u64 = 8;
/// What the leader says with keys off.
pub(crate) const KEYS_OFF_HINT: &str =
    "Shortcuts are off: Keys off, at the tab bar's right, turns them on.";

/// Which pages' tabs stand on the bar and which wait under More.
struct Fit {
    compact: bool,
    more: Vec<usize>,
}

impl Shell {
    /// The width of words on the bar, at the bar's text size.
    fn words_w(window: &mut Window, words: &str) -> f32 {
        let text = window.text_system();
        let font = text.resolve_font(&gpui::font(theme::FAMILY));
        let cell = text
            .advance(font, px(size::TEXT), 'M')
            .map_or(size::TEXT * 0.6, |s| f32::from(s.width));
        cell * words.chars().count() as f32
    }

    /// Spaced tabs where every page's fits; else close together; else as
    /// many as fit in order, the shown page's always among them, and the
    /// rest under More.
    fn fit_tabs(&self, visible: &[(usize, bool)], end_w: f32, window: &mut Window) -> Fit {
        let room = f32::from(window.viewport_size().width) - end_w;
        let own = visible.iter().filter(|(_, own)| *own).count() as f32 * OWN_TABS_LEAST;
        let width = |i: usize, pad: f32, window: &mut Window| {
            Self::words_w(window, self.pages[i].meta.title) + pad * 2.
        };
        let need = |pad: f32, window: &mut Window| -> f32 {
            own + visible
                .iter()
                .filter(|(_, own)| !own)
                .map(|(i, _)| width(*i, pad, window))
                .sum::<f32>()
        };
        if need(kit::TAB_PAD, window) <= room {
            return Fit {
                compact: false,
                more: vec![],
            };
        }
        if need(kit::TAB_PAD_COMPACT, window) <= room {
            return Fit {
                compact: true,
                more: vec![],
            };
        }
        let pad = kit::TAB_PAD_COMPACT;
        let mut left = room - own - (Self::words_w(window, MORE) + pad * 2.);
        let shown = self
            .shown
            .filter(|s| visible.iter().any(|(i, own)| i == s && !own));
        if let Some(s) = shown {
            left -= width(s, pad, window);
        }
        let mut more = vec![];
        for &(i, own) in visible {
            if own || Some(i) == shown {
                continue;
            }
            let w = width(i, pad, window);
            if more.is_empty() && w <= left {
                left -= w;
            } else {
                more.push(i);
            }
        }
        Fit {
            compact: true,
            more,
        }
    }

    fn render_tab_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let off = self.hub.read(cx).areas_off();
        let hub = self.hub.read(cx);
        let keys_on = hub.settings.keys;
        let here = hub.fleet.here();
        let link = match here.state {
            LinkState::Open => None,
            LinkState::Connecting => Some(("Connecting…", t.dim)),
            LinkState::Lost => Some(("Offline, retrying", t.error)),
            LinkState::Refused => Some(("Refused by the node", t.error)),
        };
        let address = hub.fleet.base().to_string();
        let keys_words = if keys_on { "Keys" } else { "Keys off" };
        let end_w = Self::words_w(window, keys_words)
            + END_PAD * 2.
            + link.map_or(0., |(w, _)| Self::words_w(window, w) + END_PAD * 2.);
        // The pages with a tab, and whether each draws its own.
        let mut drawn: Vec<(usize, Option<gpui::AnyElement>)> = vec![];
        for (i, page) in self.pages.iter().enumerate() {
            if page.meta.area.is_some_and(|a| off.iter().any(|o| o == a)) {
                continue;
            }
            drawn.push((i, page.tabs(self.shown == Some(i), window, cx)));
        }
        let visible: Vec<(usize, bool)> = drawn.iter().map(|(i, e)| (*i, e.is_some())).collect();
        let fit = self.fit_tabs(&visible, end_w, window);
        let style = kit::TabStyle {
            compact: fit.compact,
        };
        if cx.try_global::<kit::TabStyle>() != Some(&style) {
            cx.set_global(style);
        }
        let mut bar = div()
            .id("page-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_stretch();
        for (i, own) in drawn {
            if let Some(tabs) = own {
                bar = bar.child(tabs);
                continue;
            }
            if fit.more.contains(&i) {
                continue;
            }
            let page = &self.pages[i];
            bar = bar.child(
                kit::tab(&t, ("tab", i), self.shown == Some(i), cx)
                    .debug_selector(|| format!("tab-{}", page.meta.id))
                    .child(kit::tab_label(page.meta.title))
                    .on_click(cx.listener(move |s, _, window, cx| s.show(i, window, cx))),
            );
        }
        if !fit.more.is_empty() {
            let more = fit.more.clone();
            bar = bar.child(
                kit::tab(&t, "more-pages", false, cx)
                    .debug_selector(|| "tab-more".into())
                    .child(kit::tab_label(MORE))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |s, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let rows = more
                                .iter()
                                .map(|&i| {
                                    let shell = cx.entity().downgrade();
                                    MenuRow::new(s.pages[i].meta.title, move |window, cx| {
                                        let _ = shell.update(cx, |s, cx| s.show(i, window, cx));
                                    })
                                })
                                .collect();
                            // Under the bar, so the tabs stay in sight.
                            let at = point(e.position.x, px(size::TAB_BAR));
                            s.host.menu(rows, at, cx);
                        }),
                    ),
            );
        }
        let bar = bar
            .child(div().flex_1())
            .children(link.map(|(words, colour)| {
                // Said only while the link is not open: what the app shows
                // is then old.
                kit::tab(&t, "connection", false, cx)
                    .debug_selector(|| "connection".into())
                    .flex_none()
                    .px(px(END_PAD))
                    .text_color(colour)
                    .tooltip(kit::tip(format!(
                        "{address}: what this window shows may be out of date"
                    )))
                    .child(words)
            }))
            .child(
                kit::tab(&t, "keys-toggle", false, cx)
                    .debug_selector(|| "keys-toggle".into())
                    .flex_none()
                    .px(px(END_PAD))
                    .text_color(if keys_on { t.text2 } else { t.accent })
                    .tooltip(kit::tip(if keys_on {
                        "Every shortcut  Alt+K"
                    } else {
                        "Parvi's shortcuts are off: every key goes to the terminal. Click to turn them on."
                    }))
                    .child(keys_words)
                    .on_click(cx.listener(move |s, _, window, cx| {
                        if keys_on {
                            s.toggle_keys(window, cx);
                        } else {
                            s.set_keys(Some(true), cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|s, e: &MouseDownEvent, _, cx| {
                            let shell = cx.entity().downgrade();
                            let shell2 = shell.clone();
                            let on = s.hub.read(cx).settings.keys;
                            let rows = vec![
                                MenuRow::new("Every shortcut", move |window, cx| {
                                    let _ = shell.update(cx, |s, cx| s.toggle_keys(window, cx));
                                })
                                .chord("Alt+K"),
                                MenuRow::rule(),
                                MenuRow::new("Shortcuts on", move |_, cx| {
                                    let _ = shell2.update(cx, |s, cx| s.set_keys(Some(!on), cx));
                                })
                                .checked(on),
                            ];
                            s.host.menu(rows, e.position, cx);
                        }),
                    ),
            );
        kit::tab_bar(&t).flex_none().h(px(size::TAB_BAR)).child(bar)
    }

    /// Turns the keys on or off; `None` flips them.
    pub(crate) fn set_keys(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        self.hub.update(cx, |h, cx| {
            let on = on.unwrap_or(!h.settings.keys);
            h.settings.keys = on;
            if on && h.status.as_ref().is_some_and(|s| s.text == KEYS_OFF_HINT) {
                h.status = None;
            }
            h.save(cx);
            h.toast(if on { "Shortcuts on" } else { "Shortcuts off" }, cx);
        });
    }

    /// What the app has to say, at the window's bottom right over the
    /// page: the last report (a failure stays until it is closed or
    /// replaced, other words go after a while) and a toast. Nothing when
    /// all is well.
    fn render_notices(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let hub = self.hub.read(cx);
        let status = hub.status.clone().filter(|s| {
            s.failure
                || s.at
                    .elapsed()
                    .is_ok_and(|age| age < Duration::from_secs(NOTICE_SHOWN_S))
        });
        let toast = hub.toast.as_ref().map(|toast| render_toast(&t, toast));
        if let Some(s) = &status
            && !s.failure
            && self.status_expiry.as_ref().map(|(at, _)| *at) != Some(s.at)
        {
            let left = Duration::from_secs(NOTICE_SHOWN_S)
                .saturating_sub(s.at.elapsed().unwrap_or_default());
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(left).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            });
            self.status_expiry = Some((s.at, task));
        }
        if status.is_none() && toast.is_none() {
            return None;
        }
        let mut stack = div()
            .absolute()
            .right(px(size::HEAD_PAD))
            .bottom(px(size::HEAD_PAD))
            .max_w(px(size::NOTICE))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(size::HEAD_GAP));
        if let Some(Status {
            at,
            text,
            failure,
            action,
        }) = status
        {
            let mut notice = kit::panel(&t)
                .id("status")
                .px(px(size::HEAD_PAD))
                .py(px(size::HEAD_GAP))
                .flex()
                .items_center()
                .gap(px(size::HEAD_GAP))
                .child(
                    div()
                        .id("status-text")
                        .debug_selector(|| "status-text".into())
                        .min_w_0()
                        .text_color(if failure { t.error } else { t.text })
                        .child(text),
                );
            if let Some((label, run)) = action {
                notice = notice.child(
                    kit::button(&t, "status-action", label, kit::Kind::Plain).on_click(
                        move |_, window, cx| {
                            let run = run.clone();
                            window.defer(cx, move |window, cx| run(window, cx));
                        },
                    ),
                );
            }
            if failure {
                notice = notice.child(
                    kit::button(&t, "status-close", "×", kit::Kind::Quiet)
                        .tooltip(kit::tip("Close"))
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.hub.update(cx, |h, cx| h.withdraw(at, cx));
                        })),
                );
            }
            stack = stack.child(notice);
        }
        stack = stack.children(toast);
        Some(deferred(stack).with_priority(25).into_any_element())
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
                    .bottom(px(size::PAD))
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

/// A toast: a word that something happened, fading on its own.
fn render_toast(t: &Theme, toast: &Toast) -> AnyElement {
    let fade_at = Duration::from_millis(motion::TOAST_IN + motion::TOAST_SHOWN);
    let opacity = if toast.at.elapsed() >= fade_at {
        0.0
    } else {
        1.0
    };
    kit::panel(t)
        .id("toast")
        .debug_selector(|| "toast".into())
        .opacity(opacity)
        .px(px(size::HEAD_PAD))
        .py(px(size::HEAD_GAP))
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
            .children(self.render_notices(cx))
            .children(self.render_layers(Cover::Window, cx))
            .children(self.render_echo(cx))
    }
}
