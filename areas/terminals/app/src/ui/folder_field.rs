//! The launcher's Folder field, as CHROTE's FolderField: one input that
//! takes a folder three ways. Untouched, the list under it offers the
//! launcher's folders; a fragment ranks them; a path lists the folders
//! under what is typed, on the launcher's node. Tab completes the lit row,
//! the arrows move, Enter takes the lit row or the typed path. The list
//! keeps one height whatever it holds, so nothing moves under the pointer.
//! Its rules live in `store::launch`.

use crate::http;
use crate::store::launch::{self, FolderList, Suggestion};
use crate::store::sessions;
use crate::theme::theme;
use gpui::{Context, Entity, EventEmitter, Window, div, prelude::*, px};
use parvi_app_kit::Host;
use parvi_app_kit::host::Request;
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit;
use parvi_core::NodeId;

/// Rows the list shows at once; it is this tall whatever it holds.
const ROWS: usize = 6;
const ROW_H: f32 = 20.;
/// Characters a row shows before it elides the middle of a long path.
const ROW_CHARS: usize = 52;

pub enum FolderEvent {
    /// The folder changed, typed or picked.
    Changed,
    /// Enter: launch with the folder as it now stands.
    Submit,
}

/// One folder listing, or the node's reason it could not be read.
struct Listing {
    node: String,
    dir: String,
    folders: Result<Vec<String>, String>,
}

pub struct FolderField {
    id: usize,
    pub field: Entity<TextField>,
    /// The node paths are listed on.
    node: String,
    host: Host,
    /// What the list offers before anything is typed.
    offered: Vec<String>,
    offered_user: String,
    /// Typing turns the list into suggestions; a folder picked, launched
    /// or left alone is not being typed.
    editing: bool,
    /// The lit row, for the value it was moved on.
    lit: Option<(String, Option<usize>)>,
    listing: Option<Listing>,
    /// The (node, folder) being read now.
    reading: Option<(String, String)>,
}

/// Keep existing rows in place while a launcher is open; new choices join
/// the end rather than moving underneath the pointer.
fn keep_recent_order(current: &mut Vec<String>, offered: Vec<String>) {
    for folder in offered {
        if !current.contains(&folder) {
            current.push(folder);
        }
    }
}

impl EventEmitter<FolderEvent> for FolderField {}

impl FolderField {
    pub fn new(id: usize, host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(cx, "~"));
        cx.subscribe_in(&field, window, |f: &mut Self, _, ev, window, cx| {
            f.on_field(ev, window, cx)
        })
        .detach();
        Self {
            id,
            field,
            node: String::new(),
            host,
            offered: vec![],
            offered_user: String::new(),
            editing: false,
            lit: None,
            listing: None,
            reading: None,
        }
    }

    pub fn text(&self, cx: &gpui::App) -> String {
        self.field.read(cx).text.clone()
    }

    /// Writes a folder without it counting as typed.
    pub fn set(&mut self, value: &str, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.set(value, false, cx));
        self.editing = false;
        cx.notify();
    }

    /// Where paths are listed and what is offered: the launcher's node now.
    pub fn serve(&mut self, node: &str, user: &str, offered: Vec<String>) {
        if self.node != node || self.offered_user != user {
            self.listing = None;
            self.offered.clear();
        }
        self.node = node.to_string();
        self.offered_user = user.to_string();
        keep_recent_order(&mut self.offered, offered);
    }

    pub fn reset_recent(&mut self) {
        self.offered.clear();
    }

    fn value(&self, cx: &gpui::App) -> String {
        self.field.read(cx).text.clone()
    }

    fn list(&self, cx: &gpui::App) -> FolderList {
        launch::folder_list(&self.value(cx), self.editing)
    }

    /// The rows for what is typed now.
    fn suggestions(&self, list: &FolderList, cx: &gpui::App) -> Vec<Suggestion> {
        match list {
            FolderList::Offered => self
                .offered
                .iter()
                .map(|p| Suggestion {
                    path: p.clone(),
                    complete: p.clone(),
                })
                .collect(),
            FolderList::Matching => launch::matching(&self.value(cx), &self.offered),
            FolderList::Inside { dir, segment } => match &self.listing {
                Some(l) if l.node == self.node && l.dir == *dir => {
                    launch::inside(dir, segment, l.folders.as_deref().unwrap_or_default())
                }
                _ => vec![],
            },
        }
    }

    fn lit(&self, list: &FolderList, rows: usize, cx: &gpui::App) -> Option<usize> {
        match &self.lit {
            Some((v, i)) if *v == self.value(cx) => i.map(|i| i.min(rows.saturating_sub(1))),
            _ => launch::first_lit(list, rows),
        }
    }

    /// The folder a launch takes now, exactly as the field shows it.
    pub fn chosen(&self, cx: &gpui::App) -> String {
        self.value(cx).trim().to_string()
    }

    fn on_field(&mut self, ev: &FieldEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            FieldEvent::Changed => {
                self.editing = true;
                self.read(cx);
                cx.emit(FolderEvent::Changed);
            }
            // Folders made since the last look show on the next.
            FieldEvent::Focused => self.listing = None,
            FieldEvent::Up | FieldEvent::Down => {
                let list = self.list(cx);
                let rows = self.suggestions(&list, cx).len();
                let lit = self.lit(&list, rows, cx);
                let next = launch::step(lit, rows, matches!(ev, FieldEvent::Down), &list);
                self.lit = Some((self.value(cx), next));
            }
            FieldEvent::Tab => {
                let list = self.list(cx);
                let rows = self.suggestions(&list, cx);
                let Some(row) = self
                    .lit(&list, rows.len(), cx)
                    .or(Some(0))
                    .and_then(|i| rows.get(i))
                else {
                    return;
                };
                let complete = row.complete.clone();
                self.field.update(cx, |f, cx| f.set(&complete, false, cx));
                self.editing = true;
                self.read(cx);
                cx.emit(FolderEvent::Changed);
            }
            FieldEvent::Submit => {
                let chosen = self.chosen_for_enter(cx);
                if chosen.is_empty() {
                    return;
                }
                self.set(&chosen, cx);
                cx.emit(FolderEvent::Submit);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Enter takes the lit row, or the typed path when none is lit.
    fn chosen_for_enter(&self, cx: &gpui::App) -> String {
        let list = self.list(cx);
        let rows = self.suggestions(&list, cx);
        match self.lit(&list, rows.len(), cx) {
            Some(i) => rows[i].path.clone(),
            None => self.value(cx).trim().to_string(),
        }
    }

    fn pick(&mut self, path: &str, cx: &mut Context<Self>) {
        self.set(path, cx);
        cx.emit(FolderEvent::Changed);
    }

    /// Reads the folder typed so far on the node, once per folder.
    fn read(&mut self, cx: &mut Context<Self>) {
        let FolderList::Inside { dir, .. } = self.list(cx) else {
            return;
        };
        let at = (self.node.clone(), dir.clone());
        let listed = self
            .listing
            .as_ref()
            .is_some_and(|l| l.node == at.0 && l.dir == at.1);
        if listed || self.reading.as_ref() == Some(&at) || self.node.is_empty() {
            return;
        }
        self.reading = Some(at.clone());
        let path = format!("/api/terminal/folders?path={}", http::enc(&at.1));
        let task = self
            .host
            .request(Request::get(NodeId(at.0.clone()), path), cx);
        cx.spawn(async move |this, cx| {
            let answer = http::json::<Vec<String>>(task.await);
            let _ = this.update(cx, |f, cx| {
                if f.reading.as_ref() != Some(&at) {
                    return;
                }
                f.reading = None;
                let folders = answer;
                f.listing = Some(Listing {
                    node: at.0,
                    dir: at.1,
                    folders,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// What the list says when it holds nothing.
    fn note(&self, list: &FolderList) -> String {
        match list {
            FolderList::Offered => "Nothing recent".into(),
            FolderList::Matching => "No folder matches".into(),
            FolderList::Inside { dir, .. } => match &self.listing {
                Some(l) if l.node == self.node && l.dir == *dir => match l.folders {
                    Err(ref why) => why.clone(),
                    Ok(_) => "No folders here".into(),
                },
                _ => "Reading…".into(),
            },
        }
    }
}

impl Render for FolderField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        // Leaving the field ends the typing, as CHROTE's blur does.
        if self.editing && !self.field.read(cx).is_focused(window) {
            self.editing = false;
        }
        let list = self.list(cx);
        let rows = self.suggestions(&list, cx);
        let lit = self.lit(&list, rows.len(), cx);
        let value = self.value(cx);
        let label = match list {
            FolderList::Offered => "Recent",
            FolderList::Matching => "Matching",
            FolderList::Inside { .. } => "Folders",
        };
        let mut body = div()
            .id(("folder-rows", self.id))
            .h(px(ROW_H * ROWS as f32))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if rows.is_empty() {
            body = body.child(kit::note(&t, self.note(&list), None));
        }
        for (n, row) in rows.iter().enumerate() {
            let path = row.path.clone();
            let shown = row.path == value;
            body = body.child(
                div()
                    .id(("folder-row", self.id * 1000 + n))
                    .debug_selector({
                        let id = self.id;
                        let path = path.clone();
                        move || format!("folder-row-{id}-{path}")
                    })
                    .flex_none()
                    .h(px(ROW_H))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .text_color(if shown || lit == Some(n) {
                        t.text
                    } else {
                        t.text2
                    })
                    .when(lit == Some(n), |d| d.bg(t.accent_soft()))
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.text))
                    // The end of a path says which folder it is.
                    .child(sessions::elide_middle(&row.path, ROW_CHARS))
                    .on_click(cx.listener(move |f, _, _, cx| f.pick(&path, cx))),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(self.field.clone())
            .child(kit::label(&t, label))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::keep_recent_order;

    #[test]
    fn recent_folders_do_not_move_when_session_activity_changes() {
        let mut rows = vec!["/first".into(), "/second".into()];
        keep_recent_order(
            &mut rows,
            vec!["/second".into(), "/first".into(), "/third".into()],
        );
        assert_eq!(rows, ["/first", "/second", "/third"]);
    }
}
