//! What the page draws on the app's tab bar, its tabs, and the surfaces
//! that open over it: the rename dialogs, Overview and the scrollback
//! search.

use super::chords::Action;
use super::page::{Act, MenuItem, Terminals};
use super::surface::{self, Surface};
use super::tiles::CLAIM_EXPLANATION;
use crate::store::sessions::{self, Key};
use crate::store::workspace::{MAX_WINDOWS, Workspace, tab_label};
use crate::theme::{alpha, size, theme};
use gpui::{
    AnyElement, Context, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Window, anchored,
    canvas, deferred, div, point, prelude::*, px,
};
use parvi_app_kit::widgets::kit::{self, Kind};

impl Terminals {
    /// The terminal tabs, in the page's place on the tab bar: a click shows
    /// one, a right click opens its menu, and renaming happens in the tab.
    pub fn render_tabs(
        &mut self,
        shown: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let mut bar = div().flex().items_stretch();
        let ws = &self.settings.workspace;
        let renaming = match self.surfaces.last() {
            Some(Surface::RenameTab(i)) => Some(*i),
            _ => None,
        };
        for (i, tb) in ws.shown_tabs().iter().enumerate() {
            if renaming == Some(i) {
                bar = bar.child(
                    div()
                        .px(px(8.))
                        .w(px(size::TAB_FIELD))
                        .flex()
                        .items_center()
                        .child(self.rename_field.clone()),
                );
                continue;
            }
            let active = shown && ws.active == i;
            let marked = self.tab_marked(i);
            bar = bar.child(
                div()
                    .id(("terminal-tab", i))
                    .debug_selector(|| format!("tab-terminals-{i}"))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
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
                    .child(kit::tab_label(&tab_label(tb, i)))
                    .when(marked, |d| d.child(kit::waiting_dot(&t)))
                    .on_click(
                        cx.listener(move |s, _, window, cx| s.run(Action::Tab(i), window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                            let items = s.tab_menu(i);
                            s.menu(items, e.position, window, cx);
                        }),
                    ),
            );
        }
        if ws.shown_tabs().len() < crate::store::workspace::MAX_TABS {
            bar = bar.child(
                div()
                    .id("tab-add")
                    .debug_selector(|| "tab-add".into())
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_color(t.dim)
                    .hover(|d| d.text_color(t.text))
                    .child("+")
                    .on_click(cx.listener(|s, _, window, cx| s.act(Act::NewTab, window, cx))),
            );
        }
        bar.into_any_element()
    }

    /// Every terminal tab's menu, CHROTE's: the tab, its layout and
    /// presets, then what acts on its windows.
    pub fn tab_menu(&self, i: usize) -> Vec<MenuItem> {
        let live = self.window_panes().iter().any(Option::is_some);
        tab_menu(&self.settings.workspace, i, live)
    }
}

/// Tab `i`'s menu, as CHROTE's tab menu reads: `live` is whether any shown
/// window of the active tab shows a live session.
pub(super) fn tab_menu(ws: &Workspace, i: usize, live: bool) -> Vec<MenuItem> {
    let tab = &ws.tabs[i];
    let n = tab.count;
    let windows = (1..=MAX_WINDOWS)
        .map(|w| MenuItem::new(w.to_string(), Act::Windows(i, w)).checked(n == w))
        .collect();
    let restore = if ws.presets.is_empty() {
        vec![MenuItem::note("No presets")]
    } else {
        (ws.presets.iter().enumerate())
            .map(|(k, p)| MenuItem::new(p.name.clone(), Act::RestorePreset(k)))
            .collect()
    };
    let mut items = vec![
        MenuItem::new("Rename tab", Act::RenameTab(i)),
        MenuItem::rule(),
        MenuItem::sub("Windows", Some(n.to_string()), windows),
        MenuItem::new("Save layout as preset", Act::SavePreset),
        MenuItem::sub("Restore preset", None, restore),
    ];
    if !ws.presets.is_empty() {
        let delete = (ws.presets.iter().enumerate())
            .map(|(k, p)| MenuItem::new(p.name.clone(), Act::DeletePreset(k)).danger())
            .collect();
        items.push(MenuItem::sub("Delete preset", None, delete));
    }
    let bound = tab.shown().iter().any(|w| !w.is_empty());
    let claim = MenuItem::new("Claim all", Act::ClaimAll).chord("Alt+C");
    let claim = if ws.active != i {
        claim.refused("Open this tab to claim its visible frames.")
    } else if !live {
        claim.refused("No window here shows a live session.")
    } else {
        claim.hint(CLAIM_EXPLANATION)
    };
    let reconnect = MenuItem::new("Reconnect frames", Act::Reconnect);
    items.extend([
        MenuItem::rule(),
        MenuItem::new("Clear tab assignments", Act::ClearTab(i)).danger(),
        if bound {
            reconnect
        } else {
            reconnect.refused("No window here holds a session.")
        },
        claim,
    ]);
    if ws.shown_tabs().len() > 1 {
        items.push(MenuItem::rule());
        items.push(MenuItem::new("Close tab", Act::CloseTab(i)));
    }
    items
}

impl Terminals {
    /// A transparent layer under a surface: a click outside closes it.
    pub(super) fn scrim(&self, name: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        div().absolute().inset_0().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |s, _, _, cx| s.close(name, cx)),
        )
    }

    /// The rename dialog for the surface of this `kind`: a tab or a session.
    pub(super) fn render_rename(
        &mut self,
        kind: &'static str,
        title: String,
        note: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        deferred(
            self.scrim(kind, cx)
                .flex()
                .justify_center()
                .items_start()
                .pt(px(140.))
                .child(
                    kit::panel(&t)
                        .w(px(360.))
                        .h_auto()
                        .p(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(kit::label(&t, title))
                        .child(self.rename_field.clone())
                        .child(kit::note(&t, note, Some(t.dim))),
                ),
        )
        .with_priority(5)
        .into_any_element()
    }

    /// Naming a preset, in a panel where the tab menu stood: the menu
    /// becomes the editor, as CHROTE's does.
    pub(super) fn render_preset_name(
        &mut self,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        deferred(
            self.scrim(surface::PRESET_NAME, cx).child(
                anchored()
                    .position(point(at.x, at.y))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        kit::panel(&t)
                            .w(px(260.))
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(kit::label(&t, "Preset name"))
                            .child(self.rename_field.clone())
                            .child(kit::note(
                                &t,
                                "Enter saves every tab's windows · Esc cancels",
                                Some(t.dim),
                            )),
                    ),
            ),
        )
        .with_priority(10)
        .into_any_element()
    }

    pub(super) fn render_overview(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let columns = self.overview_columns.clone();
        let measure = canvas(
            |_, _, _| (),
            move |bounds, _, _, _| {
                let width = f32::from(bounds.size.width);
                columns.set((((width - 24. + 10.) / (size::THUMB_W + 10.)) as usize).max(1));
            },
        )
        .absolute()
        .inset_0();
        let mut grid = div()
            .id("overview-grid")
            .debug_selector(|| "overview-grid".into())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.overview_scroll)
            .p(px(12.))
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .child(measure);
        let list = self.fleet.all.clone();
        self.overview_cursor = self.overview_cursor.min(list.len().saturating_sub(1));
        if list.is_empty() {
            grid = grid.child(kit::note(&t, "No sessions.", None));
        }
        for (i, s) in list.iter().enumerate() {
            let key = Key::of(s);
            let thumb = s
                .panes
                .first()
                .and_then(|p| self.fleet.thumb(&s.node, p.id));
            let text = thumb
                .map(|th| sessions::screen_tail(th, size::THUMB_LINES))
                .unwrap_or_default();
            let waiting = sessions::waiting(s);
            grid = grid.child(
                div()
                    .id(("ov", i))
                    .w(px(size::THUMB_W))
                    .h(px(size::THUMB_H))
                    .flex()
                    .flex_col()
                    .bg(t.surface)
                    .border_1()
                    .border_color(if i == self.overview_cursor || waiting.is_some() {
                        t.accent
                    } else {
                        t.divider
                    })
                    .cursor_pointer()
                    .hover(|d| d.border_color(t.accent))
                    .child(
                        div()
                            .h(px(26.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .bg(t.raised)
                            .children(
                                self.where_is(&s.node)
                                    .map(|p| super::place::passport(&t, &p.device, &p.environment)),
                            )
                            .when(waiting.is_some(), |d| d.child(kit::waiting_dot(&t)))
                            .when(i == self.overview_cursor, |d| {
                                d.child(div().text_color(t.accent).child("›"))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(s.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(t.dim)
                                    .child(sessions::activity(s, super::page::now_ms())),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .p(px(6.))
                            .bg(t.term_bg)
                            .text_size(px(size::THUMB_TEXT))
                            .line_height(px(size::THUMB_LINE))
                            .text_color(t.term_fg)
                            .flex()
                            .flex_col()
                            .justify_end()
                            .children(
                                text.lines()
                                    .map(|l| div().whitespace_nowrap().child(l.to_string()))
                                    .collect::<Vec<_>>(),
                            ),
                    )
                    .on_click(cx.listener(move |sh, _, window, cx| {
                        sh.close(surface::OVERVIEW, cx);
                        sh.place(key.clone(), window, cx);
                    })),
            );
        }
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::overview_key))
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(
                div()
                    .h(px(size::STRIP))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("OVERVIEW · {} SESSIONS", list.len())),
                    )
                    .child(
                        kit::note(&t, "Arrow keys choose · Enter opens", Some(t.dim)).mr(px(12.)),
                    )
                    .child(
                        kit::button(&t, "ov-close", "Close  Esc", Kind::Quiet)
                            .on_click(cx.listener(|s, _, _, cx| s.close(surface::OVERVIEW, cx))),
                    ),
            )
            .child(grid)
            .into_any_element()
    }

    /// The shell handles Escape and leader chords before they reach this layer.
    fn overview_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let len = self.fleet.all.len();
        if len == 0 {
            return;
        }
        let cursor = self.overview_cursor.min(len - 1);
        let columns = self.overview_columns.get();
        self.overview_cursor = match event.keystroke.key.as_str() {
            "left" => cursor.saturating_sub(1),
            "right" => (cursor + 1).min(len - 1),
            "up" => cursor.saturating_sub(columns),
            "down" => cursor.saturating_add(columns).min(len - 1),
            "enter" => {
                let key = Key::of(&self.fleet.all[cursor]);
                self.close(surface::OVERVIEW, cx);
                self.place(key, window, cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        // The first grid child is its measurement canvas.
        self.overview_scroll
            .scroll_to_item(self.overview_cursor + 1);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_find(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let mut results = div()
            .id("find-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        let found = self.fleet.found();
        let summary = match &found {
            None => "Type to search every pane's history.".to_string(),
            Some((matches, done)) => match *done {
                None => format!("Searching… {} found", matches.len()),
                Some((m, p, trunc)) => format!(
                    "{m} {} in {p} {}{}",
                    if m == 1 { "match" } else { "matches" },
                    if p == 1 { "pane" } else { "panes" },
                    if trunc { ", stopped early" } else { "" }
                ),
            },
        };
        if let Some((matches, _)) = &found {
            let cursor = self.find_cursor.min(matches.len().saturating_sub(1));
            for (i, m) in matches.iter().enumerate().take(500) {
                let name = self
                    .fleet
                    .session_of_pane(&m.node, m.pane)
                    .map(|x| x.name.clone())
                    .unwrap_or_else(|| format!("pane {}", m.pane));
                let device = self.where_is(&m.node).map(|p| p.device).unwrap_or_default();
                results = results.child(
                    div()
                        .id(("match", i))
                        .px(px(12.))
                        .py(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .cursor_pointer()
                        .border_b_1()
                        .border_color(t.divider)
                        .when(i == cursor, |d| d.bg(t.raised))
                        .hover(|d| d.bg(t.accent_light()))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(div().text_color(t.text).child(name))
                                .child(div().text_size(px(11.)).text_color(t.dim).child(device))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(t.dim)
                                        .child(format!("line {}", m.line + 1)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.dim)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(m.context[0].clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.text)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(m.context[1].clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.dim)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(m.context[2].clone()),
                        )
                        .on_click(cx.listener(move |s, _, window, cx| {
                            s.find_cursor = i;
                            s.open_match(i, window, cx);
                        })),
                );
            }
        }
        let drawer = div()
            .id("find")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(460.))
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.divider)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(size::STRIP))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("SEARCH SCROLLBACK"),
                    )
                    .child(
                        kit::button(&t, "find-close", "Close  Esc", Kind::Quiet)
                            .on_click(cx.listener(|s, _, _, cx| s.close(surface::FIND, cx))),
                    ),
            )
            .child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(self.find_query.clone())
                    .child(kit::note(&t, summary, Some(t.dim))),
            )
            .child(kit::rule(&t))
            .child(results);
        div()
            .absolute()
            .inset_0()
            .child(self.scrim(surface::FIND, cx).bg(alpha(t.bg, 0.3)))
            .child(drawer)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key {
        Key {
            node: "n".into(),
            user: String::new(),
            name: name.into(),
        }
    }

    fn row<'a>(items: &'a [MenuItem], label: &str) -> &'a MenuItem {
        items.iter().find(|m| m.label == label).expect(label)
    }

    #[test]
    fn the_tab_menu_reads_the_window_count_and_sets_one_from_its_submenu() {
        let mut ws = Workspace::default();
        ws.set_windows(0, 3);
        let menu = tab_menu(&ws, 0, false);
        let windows = row(&menu, "Windows");
        assert_eq!(windows.chord.as_deref(), Some("3 ›"));
        let checked: Vec<_> = windows.sub.iter().filter(|m| m.checked).collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].act, Some(Act::Windows(0, 3)));
        assert_eq!(windows.sub.len(), MAX_WINDOWS);
    }

    #[test]
    fn presets_are_saved_from_the_menu_and_restored_from_a_submenu() {
        let mut ws = Workspace::default();
        let menu = tab_menu(&ws, 0, false);
        assert_eq!(
            row(&menu, "Save layout as preset").act,
            Some(Act::SavePreset)
        );
        assert!(row(&menu, "Restore preset").sub[0].act.is_none());
        assert!(menu.iter().all(|m| m.label != "Delete preset"));
        ws.save_preset("Morning").unwrap();
        let menu = tab_menu(&ws, 0, false);
        let restore = &row(&menu, "Restore preset").sub;
        assert_eq!(restore[0].label, "Morning");
        assert_eq!(restore[0].act, Some(Act::RestorePreset(0)));
        assert!(row(&menu, "Delete preset").sub[0].danger);
    }

    #[test]
    fn reconnect_and_claim_all_say_why_they_are_refused() {
        let mut ws = Workspace::default();
        ws.add_tab();
        ws.show_tab(0);
        let menu = tab_menu(&ws, 0, false);
        assert!(row(&menu, "Reconnect frames").refused);
        let claim = row(&menu, "Claim all");
        assert!(claim.refused);
        assert_eq!(
            claim.hint.as_deref(),
            Some("No window here shows a live session.")
        );
        ws.bind(0, key("a"), None);
        let menu = tab_menu(&ws, 0, true);
        assert!(!row(&menu, "Reconnect frames").refused);
        let claim = row(&menu, "Claim all");
        assert!(!claim.refused);
        assert_eq!(claim.hint.as_deref(), Some(CLAIM_EXPLANATION));
        assert_eq!(claim.chord.as_deref(), Some("Alt+C"));
        // An inactive tab's menu acts on nothing on screen.
        let other = tab_menu(&ws, 1, true);
        let claim = row(&other, "Claim all");
        assert!(claim.refused);
        assert_eq!(
            claim.hint.as_deref(),
            Some("Open this tab to claim its visible frames.")
        );
        assert_eq!(row(&other, "Rename tab").act, Some(Act::RenameTab(1)));
    }

    #[test]
    fn the_last_tab_offers_no_close() {
        let mut ws = Workspace::default();
        assert!(
            tab_menu(&ws, 0, false)
                .iter()
                .all(|m| m.label != "Close tab")
        );
        ws.add_tab();
        assert_eq!(
            row(&tab_menu(&ws, 1, false), "Close tab").act,
            Some(Act::CloseTab(1))
        );
    }
}
