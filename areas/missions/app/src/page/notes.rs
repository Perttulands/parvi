//! Canvas note pins and sticky threads, carried from the original Missions page.
use super::canvas::View;
use super::*;
use gpui::{AnyElement, FontWeight, Hsla, MouseButton, SharedString};
use parvi_app_kit::theme::{Theme, rgb};
use parvi_missions_contract::{
    MISSION_THREAD, Mission,
    document::{Note, NotesDocument},
};

const STICKY_W: f32 = 240.;
const OPEN_W: f32 = 320.;
const GAP: f32 = 6.;
const STICKY_ROOM: f32 = 130.;
const PIN: f32 = 22.;
const PIN_IN: f32 = 9.;
const PIN_MIN: f32 = 16.;
const AUTHOR: &str = "human:ui";

#[derive(Default)]
pub(super) struct NotesUi {
    pub(super) open: Option<String>,
    loading: bool,
    error: Option<String>,
}
fn paper(t: &Theme, agent: bool) -> (Hsla, Hsla) {
    if agent {
        (rgb(t.ansi[12]), rgb(t.ansi[4]))
    } else {
        (rgb(t.ansi[11]), rgb(t.ansi[3]))
    }
}
fn author(value: &str) -> (bool, String) {
    let (prefix, name) = value.split_once(':').unwrap_or((value, ""));
    if prefix == "agent" {
        (true, if name.is_empty() { "agent" } else { name }.into())
    } else {
        (
            false,
            if name.is_empty() || name == "ui" {
                "operator"
            } else {
                name
            }
            .into(),
        )
    }
}
fn when(note: &Note) -> String {
    let timestamp = note
        .created_at
        .chars()
        .take(16)
        .collect::<String>()
        .replace('T', " ");
    if note.edited_at.is_some() {
        format!("{timestamp} · edited")
    } else {
        timestamp
    }
}
fn thread<'a>(notes: &'a NotesDocument, target: &str) -> &'a [Note] {
    if target == MISSION_THREAD {
        &notes.mission
    } else {
        notes
            .elements
            .iter()
            .find(|thread| thread.node_id == target)
            .map(|thread| thread.entries.as_slice())
            .unwrap_or(&[])
    }
}
fn home(card: &board::Card, below: f32) -> (f32, f32) {
    (
        card.x.max(card.x + card.w - STICKY_W),
        card.y + card.h + GAP + below,
    )
}
impl MissionsPage {
    /// Keep a failed notes read visible and let the operator retry it.
    pub(super) fn load_canvas_notes(&mut self, node: String, slug: String, cx: &mut Context<Self>) {
        if self.note_paper.loading {
            return;
        }
        self.note_paper.loading = true;
        self.note_paper.error = None;
        let generation = self.generation;
        let asked = self.host.request(
            Request::get(
                NodeId(node.clone()),
                format!("{}/notes", mission_path(&slug)),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation
                    || page.state.selected.as_ref() != Some(&(node, slug))
                {
                    return;
                }
                page.note_paper.loading = false;
                match result {
                    Ok(answer) if answer.ok() => match serde_json::from_slice::<
                        parvi_missions_contract::NotesResponse,
                    >(&answer.body)
                    {
                        Ok(notes) => {
                            page.notes = Some(notes.notes);
                            page.note_paper.error = None;
                        }
                        Err(error) => page.note_paper.error = Some(error.to_string()),
                    },
                    Ok(answer) => page.note_paper.error = Some(answer.error_text()),
                    Err(error) => {
                        page.note_paper.error = Some(format!("{}: {}", error.code, error.message))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn open_canvas_note(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.note_saving
            || self.note.read(cx).text != self.note_base
            || self.editor.is_some() && self.unsaved(cx).is_some()
        {
            self.state.error =
                Some("Save or discard the current draft before opening another note.".into());
            cx.notify();
            return;
        }
        self.editor = None;
        self.note_id = None;
        self.note_base.clear();
        self.note.update(cx, |field, cx| field.set("", false, cx));
        self.note_paper.open = Some(target);
        self.note_etag = self.notes.as_ref().map(|notes| notes.etag.clone());
        parvi_app_kit::widgets::field::focus(&self.note, window, cx);
        self.state.error = None;
        cx.notify();
    }
    pub(super) fn close_canvas_note(&mut self, cx: &mut Context<Self>) {
        if self.note_saving || self.note.read(cx).text != self.note_base {
            self.state.error =
                Some("Save or discard the current note draft before closing it.".into());
        } else {
            self.note_paper.open = None;
            self.note_etag = None;
            self.note_id = None;
            self.note_base.clear();
            self.note.update(cx, |field, cx| field.set("", false, cx));
            self.state.error = None;
        }
        cx.notify();
    }
    pub(super) fn canvas_notes(
        &self,
        t: &Theme,
        mission: &Mission,
        board: &board::Board,
        view: &View,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(notes) = self.notes.as_ref() else {
            let notice = if let Some(error) = &self.note_paper.error {
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "canvas-notes-error".into())
                            .text_color(t.error)
                            .child(format!("Reading notes failed: {error}")),
                    )
                    .child(
                        kit::button(t, "canvas-notes-retry", "Try again", Kind::Quiet)
                            .debug_selector(|| "canvas-notes-retry".into())
                            .on_click(cx.listener(|page, _, _, cx| {
                                if let Some((node, slug)) = page.state.selected.clone() {
                                    page.load_canvas_notes(node, slug, cx);
                                }
                            })),
                    )
            } else {
                div().text_color(t.dim).child("Reading notes…")
            };
            return vec![
                div()
                    .absolute()
                    .left(px(PAD))
                    .bottom(px(PAD))
                    .child(notice)
                    .into_any_element(),
            ];
        };
        let mut out = Vec::new();
        let mut input_sticky = false;
        for card in &board.cards {
            let entries = thread(notes, &card.id);
            let open = self.note_paper.open.as_deref() == Some(card.id.as_str());
            out.push(self.canvas_note_pin(t, card, !entries.is_empty(), view, cx));
            if entries.is_empty() && !open {
                continue;
            }
            if card.kind == board::Kind::Input {
                input_sticky = true;
            }
            out.push(self.canvas_sticky(
                t,
                &card.id,
                mission.node_name(&card.id),
                entries,
                home(card, 0.),
                view,
                cx,
            ));
        }
        let at = match board
            .cards
            .iter()
            .find(|card| card.kind == board::Kind::Input)
        {
            Some(card) => (
                card.x,
                home(card, if input_sticky { STICKY_ROOM } else { 0. }).1,
            ),
            None => board
                .cards
                .iter()
                .min_by(|a, b| (a.x + a.y).total_cmp(&(b.x + b.y)))
                .map_or((40., 40.), |card| (card.x, card.y + card.h + GAP)),
        };
        // Keep the mission paper out of the way of cards below its anchor. Its
        // height changes when the thread opens, so place it below all cards in
        // its horizontal span rather than estimating the paper height.
        let width = if self.note_paper.open.as_deref() == Some(MISSION_THREAD) {
            OPEN_W
        } else {
            STICKY_W
        };
        let at = (
            at.0,
            board
                .cards
                .iter()
                .filter(|card| {
                    card.x < at.0 + width && card.x + card.w > at.0 && card.y + card.h > at.1
                })
                .fold(at.1, |y, card| y.max(card.y + card.h + GAP)),
        );
        out.push(self.canvas_sticky(
            t,
            MISSION_THREAD,
            "Mission notes",
            thread(notes, MISSION_THREAD),
            at,
            view,
            cx,
        ));
        out
    }
    fn canvas_note_pin(
        &self,
        t: &Theme,
        card: &board::Card,
        has: bool,
        view: &View,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let z = view.zoom();
        let size = (PIN * z).max(PIN_MIN);
        let inset = (PIN_IN * z).max(EDGE);
        let (right, top) = view.position(card.x + card.w, card.y);
        let play_space = if card.kind == board::Kind::Input {
            (PAD + size::CONTROL + GAP) * z
        } else if card.kind == board::Kind::End {
            board::END_H * z / 3.
        } else {
            0.
        };
        let x = right - inset - size - play_space;
        let y = if card.kind == board::Kind::End {
            top + (card.h * z - size) / 2.
        } else {
            top + inset
        };
        let (fill, edge) = paper(t, false);
        let target = card.id.clone();
        let selector = format!("canvas-note-pin-{target}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(px(x))
            .top(px(y))
            .size(px(size))
            .border_1()
            .border_color(edge)
            .bg(if has { fill } else { edge })
            .shadow_sm()
            .text_size(px((size::TEXT * z).max(size::SMALL)))
            .font_weight(FontWeight::BOLD)
            .text_color(t.bg)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .opacity(if has { 1. } else { 0.8 })
            .hover(|d| d.opacity(1.))
            .tooltip(kit::tip(if has {
                "Open the note thread"
            } else {
                "Add a note"
            }))
            .child(if has { "▰" } else { "+" })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |page, _, window, cx| {
                page.open_canvas_note(target.clone(), window, cx)
            }))
            .into_any_element()
    }
    #[allow(clippy::too_many_arguments)]
    fn canvas_sticky(
        &self,
        t: &Theme,
        target: &str,
        title: &str,
        entries: &[Note],
        at: (f32, f32),
        view: &View,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let z = view.zoom();
        let (x, y) = view.position(at.0, at.1);
        let open = self.note_paper.open.as_deref() == Some(target);
        let latest = entries.last();
        let (fill, edge) = paper(t, latest.is_some_and(|note| author(&note.author).0));
        let selector = format!("canvas-note-{target}");
        let base = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px((if open { OPEN_W } else { STICKY_W }) * z))
            .p(px(GAP * z))
            .flex()
            .flex_col()
            .gap(px(GAP * z))
            .border_1()
            .border_l(px(size::RADIUS * z))
            .border_color(edge)
            .bg(fill)
            .text_color(t.bg)
            .shadow_lg()
            .text_size(px(size::TEXT * z))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if !open {
            let target = target.to_owned();
            let summary = latest
                .map(|note| note.text.clone())
                .unwrap_or_else(|| "Write what this mission is for.".into());
            let byline = latest
                .map(|note| format!("{} · {}", author(&note.author).1, when(note)))
                .unwrap_or_default();
            return base
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::BOLD)
                                .child(title.to_uppercase()),
                        )
                        .child("OPEN"),
                )
                .when(!byline.is_empty(), |d| d.child(byline))
                .child(div().line_clamp(4).child(summary))
                .when(entries.len() > 1, |d| {
                    d.child(format!("+{} earlier", entries.len() - 1))
                })
                .on_click(cx.listener(move |page, _, window, cx| {
                    page.open_canvas_note(target.clone(), window, cx)
                }))
                .into_any_element();
        }
        let mut list = div()
            .id(SharedString::from(format!("canvas-note-thread-{target}")))
            .flex()
            .flex_col()
            .gap(px(GAP * z))
            .max_h(px(size::NOTICE * z))
            .overflow_y_scroll();
        for note in entries {
            let (_, name) = author(&note.author);
            let edit_id = note.id.clone();
            let delete_id = note.id.clone();
            let edit_text = note.text.clone();
            let delete_text = note.text.clone();
            let entry_selector = format!("canvas-note-entry-{}-{}", note.id, name);
            let (_, entry_edge) = paper(t, author(&note.author).0);
            let mut entry = div()
                .debug_selector(move || entry_selector.clone())
                .flex()
                .flex_col()
                .p(px(GAP * z))
                .border_1()
                .border_l(px(size::RADIUS * z))
                .border_color(entry_edge)
                .child(format!("{} · {}", name, when(note)))
                .child(note.text.clone());
            if note.author == AUTHOR {
                let edit_selector = format!("canvas-note-edit-{}", note.id);
                let delete_selector = format!("canvas-note-delete-{}", note.id);
                entry = entry.child(div().flex()
                    .child(kit::button(t, SharedString::from(edit_selector.clone()), "Edit", Kind::Quiet).text_color(t.bg).debug_selector(move || edit_selector.clone()).on_click(cx.listener(move |page, _, window, cx| { page.select_note(&edit_id, &edit_text, cx); if page.note_id.as_deref() == Some(edit_id.as_str()) { parvi_app_kit::widgets::field::focus(&page.note, window, cx); } })))
                    .child(kit::button(t, SharedString::from(delete_selector.clone()), "Delete", Kind::Quiet).text_color(t.bg).debug_selector(move || delete_selector.clone()).on_click(cx.listener(move |page, _, _, cx| {
                        if page.note_saving || page.note.read(cx).text != page.note_base { page.state.error = Some("Save or discard the current note draft before deleting an entry.".into()); cx.notify(); return; }
                        page.select_note(&delete_id, &delete_text, cx);
                        page.add_note(true, cx);
                    }))));
            }
            list = list.child(entry);
        }
        let close_selector = format!("canvas-note-close-{target}");
        let save_selector = format!("canvas-note-save-{target}");
        let placeholder = if self.note_id.is_some() {
            "Save edit"
        } else if entries.is_empty() {
            "Add note"
        } else {
            "Reply"
        };
        base.child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::BOLD)
                        .child(title.to_uppercase()),
                )
                .child(
                    kit::button(
                        t,
                        SharedString::from(close_selector.clone()),
                        "×",
                        Kind::Quiet,
                    )
                    .text_color(t.bg)
                    .debug_selector(move || close_selector.clone())
                    .on_click(cx.listener(|page, _, _, cx| page.close_canvas_note(cx))),
                ),
        )
        .child(list)
        .child(
            div()
                .debug_selector(|| "canvas-note-field".into())
                .child(self.note.clone()),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(GAP * z))
                .when(
                    self.note_id.is_some() || self.note.read(cx).text != self.note_base,
                    |d| {
                        d.child(
                            kit::button(t, "canvas-note-discard", "Discard", Kind::Quiet)
                                .text_color(t.bg)
                                .on_click(cx.listener(|page, _, _, cx| page.discard_note(cx))),
                        )
                    },
                )
                .child(
                    kit::button(
                        t,
                        SharedString::from(save_selector.clone()),
                        if self.note_saving {
                            "Saving…"
                        } else {
                            placeholder
                        },
                        Kind::Primary,
                    )
                    .debug_selector(move || save_selector.clone())
                    .when(!self.note_saving, |d| {
                        d.on_click(cx.listener(|page, _, _, cx| page.add_note(false, cx)))
                    }),
                ),
        )
        .when_some(self.state.error.clone(), |d, error| {
            d.child(
                div()
                    .debug_selector(|| "canvas-note-error".into())
                    .text_color(t.error)
                    .child(error),
            )
        })
        .into_any_element()
    }
}
