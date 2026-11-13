//! Send to Session, the one hand-off (CHROTE's `SendDrawer`): a drawer at
//! the right edge. Every surface opens it through [`Terminals::handoff`] with a
//! reference to what it was showing; the reference is shown, not edited, and
//! the note under it is the operator's. Enter sends (paste and submit),
//! Shift+Enter breaks the line, Ctrl+Enter or Paste only pastes. Files
//! dropped on the drawer or attached land on the target's node and their
//! paths follow the note. "New agent…" launches one and pastes the message
//! into it without submitting, once the node finds it ready for a paste.
//!
//! A send that arrived closes the drawer; one that failed keeps it with the
//! words of why; one whose outcome is unknown keeps it and refuses to send
//! again until the operator has looked. Closing gives the focus back to
//! what had it, and the width dragged at the inner edge is remembered on
//! this device. The rules are `store::send`'s.

use super::launcher::SEND;
use super::page::Terminals;
use super::surface::{self, Surface};
use crate::http;
use crate::store::send;
use crate::store::sessions::{self, Key};
use crate::theme::{alpha, size, theme};
use gpui::{
    AnyElement, AsyncApp, ClickEvent, Context, ExternalPaths, FontWeight, MouseButton,
    MouseMoveEvent, WeakEntity, Window, div, prelude::*, px,
};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_core::NodeId;
use parvi_terminals_contract::Outcome;
use std::path::PathBuf;

/// The drawer's state besides its fields: what it hands over and how the
/// last send went.
#[derive(Default)]
pub struct Draft {
    /// The one line naming the object handed over.
    pub reference: Option<String>,
    /// The originating session, excluded from this handoff's targets.
    pub source: Option<Key>,
    /// Files from this computer, written on the target's node on send.
    pub files: Vec<Attachment>,
    /// Why the last send did not simply arrive, shown above the actions.
    pub said: Option<String>,
    /// The last send's outcome is unknown: no sending again from here.
    pub stuck: bool,
    /// The picker's "New agent…" row is chosen.
    pub new_agent: bool,
    /// Explicitly chosen pane; absent means the target tile's pane.
    pub pane: Option<u32>,
    /// The drawer's launch, by node and request, awaiting its session.
    pub launch: Option<(String, u32)>,
    /// The drawer's launched session (node, id), awaiting its first pane.
    pub waiting: Option<(String, String)>,
    /// A send in flight, by request, whether it was a launch's paste, and
    /// whether it presses Enter.
    pub sending: Option<(u32, Key, bool, bool)>,
    /// Files being written on the target's node.
    pub uploading: bool,
    /// The focused tile grid before the drawer made room for itself.
    pub launch_grid: Option<super::page::Grid>,
    /// The inner edge is being dragged.
    pub resizing: bool,
    /// The drawer opened on the left, clear of its target's window. Chosen
    /// once as it opens, so it never jumps under the pointer.
    pub left: bool,
}

/// A local file or a clipboard image kept in memory until sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    File(PathBuf),
    Image { name: String, bytes: Vec<u8> },
}

impl Terminals {
    /// Opens Send with the focused terminal as the source, aimed at `target`.
    pub fn open_send(&mut self, target: Option<Key>, window: &mut Window, cx: &mut Context<Self>) {
        let reference = self
            .focused_key()
            .map(|key| format!("session {}", key.text()));
        self.handoff(reference, target, window, cx);
    }

    /// Sends the named window's session as the source to a different target.
    pub fn send_from(&mut self, source: Key, window: &mut Window, cx: &mut Context<Self>) {
        let reference = Some(format!("session {}", source.text()));
        self.handoff_from(reference, None, Some(source), window, cx);
    }

    pub(super) fn send_targets(&self, query: &str) -> Vec<&parvi_terminals_contract::Session> {
        sessions::targets(&self.fleet.all, &self.settings.recent_targets, query)
            .into_iter()
            .filter(|s| {
                self.send
                    .source
                    .as_ref()
                    .is_none_or(|source| !sessions::is(s, source))
            })
            .collect()
    }

    /// Opens Send to Session with `reference`, the line naming what the
    /// operator was looking at, aimed at `target`. A fresh opening is a
    /// fresh message: the last note belonged to something else.
    pub fn handoff(
        &mut self,
        reference: Option<String>,
        target: Option<Key>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handoff_from(reference, target, None, window, cx);
    }

    fn handoff_from(
        &mut self,
        reference: Option<String>,
        target: Option<Key>,
        source: Option<Key>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.send.sending.is_some() || self.send.uploading || self.send.waiting.is_some() {
            // The last one is still on its way: its draft stays until it lands.
            self.open(Surface::Send, window, cx);
            return;
        }
        let launch_grid = self.launch_grids.borrow()[self.settings.workspace.focus];
        self.send = Draft {
            reference,
            source,
            launch_grid,
            ..Default::default()
        };
        self.send_target = target;
        self.send_cursor = 0;
        self.send_query.update(cx, |f, cx| f.set("", false, cx));
        self.send_note.update(cx, |f, cx| f.set("", false, cx));
        self.open(Surface::Send, window, cx);
        // Where the windows move aside for the drawer it covers none of
        // them; otherwise it opens on the side away from the target.
        let room = window.viewport_size().width / px(1.);
        self.send.left = self.drawer_room(window).is_none()
            && send::drawer_left(
                self.aimed_span(),
                send::width(self.settings.send_width, room),
                room,
            );
    }

    /// The drawer closed: the keyboard goes back to the terminal, whichever
    /// view Send came from.
    pub(super) fn send_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.surfaces.is_empty() {
            window.focus(&self.term_focus, cx);
        }
    }

    /// Picks a target from the list; the picker's own row is a new agent.
    fn pick(&mut self, target: Option<Key>, window: &mut Window, cx: &mut Context<Self>) {
        self.send.new_agent = target.is_none();
        if self.send_target != target {
            self.send.pane = None;
        }
        if self.send.new_agent {
            let l = self.launchers[SEND].clone();
            l.update(cx, |l, cx| l.focus_first(window, cx));
        } else {
            self.send_target = target;
            parvi_app_kit::widgets::field::focus(&self.send_note, window, cx);
        }
        self.send.said = None;
        self.send.stuck = false;
        cx.notify();
    }

    /// Send (Enter) or Paste only (Ctrl+Enter) to the chosen target.
    pub fn send_now(&mut self, enter: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.send.sending.is_some()
            || self.send.uploading
            || self.send.waiting.is_some()
            || self.send.stuck
        {
            return;
        }
        if self.send.new_agent {
            self.send.said =
                Some("Launch the new agent below; the message is pasted into it.".into());
            cx.notify();
            return;
        }
        if self.draft_text(cx).trim().is_empty() && self.send.files.is_empty() {
            return;
        }
        let Some(k) = self.send_target.clone() else {
            parvi_app_kit::widgets::field::focus(&self.send_query, window, cx);
            return;
        };
        self.deliver(k, enter, false, cx);
    }

    fn draft_text(&self, cx: &Context<Self>) -> String {
        let note = self.send_note.read(cx).text.clone();
        send::message(self.send.reference.as_deref(), &note, &[])
    }

    /// Writes the attachments on `k`'s node, then pastes the message.
    fn deliver(&mut self, k: Key, enter: bool, launched: bool, cx: &mut Context<Self>) {
        self.send.said = None;
        let Some(pane) = self.send_pane(&k) else {
            self.send.said = Some(format!("{} has no pane to paste into.", k.name));
            cx.notify();
            return;
        };
        let files = self.send.files.clone();
        if files.is_empty() {
            self.paste(k, pane, enter, launched, vec![], cx);
            return;
        }
        self.send.uploading = true;
        let node = NodeId(k.node.clone());
        let user = k.user.clone();
        cx.spawn(async move |this, cx| {
            let written = write_drops(&this, &node, &user, &files, cx).await;
            let _ = this.update(cx, |s, cx| {
                s.send.uploading = false;
                match written {
                    Ok(paths) => s.paste(k, pane, enter, launched, paths, cx),
                    Err(why) => s.send.said = Some(why),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn paste(
        &mut self,
        k: Key,
        pane: u32,
        enter: bool,
        launched: bool,
        files: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let note = self.send_note.read(cx).text.clone();
        let text = send::message(self.send.reference.as_deref(), &note, &files);
        let Some(s) = self.fleet.session(&k) else {
            self.send.said = Some(format!("{} is not running.", k.name));
            return;
        };
        if !s.panes.iter().any(|p| p.id == pane) {
            self.send.said = Some(format!(
                "The selected pane of {} is gone; choose another.",
                k.name
            ));
            cx.notify();
            return;
        }
        // A launched agent may still be starting: the node waits until it
        // is ready for the paste (#242).
        match self.fleet.paste_to(&k.node, pane, &text, enter, launched) {
            Ok(id) => {
                self.answered(&k, cx);
                self.send.sending = Some((id, k, launched, enter));
            }
            Err(e) => self.send.said = Some(e),
        }
        cx.notify();
    }

    /// The chosen pane, otherwise the pane shown by the target's tile,
    /// otherwise the target's first pane.
    fn send_pane(&self, key: &Key) -> Option<u32> {
        let session = self.fleet.session(key)?;
        self.send.pane.or_else(|| {
            let ws = &self.settings.workspace;
            ws.shown(ws.focus)
                .filter(|binding| &binding.key == key)
                .or_else(|| {
                    ws.windows()
                        .iter()
                        .filter_map(|w| w.shown())
                        .find(|binding| &binding.key == key)
                })
                .and_then(|binding| binding.pane)
                .filter(|id| session.panes.iter().any(|pane| pane.id == *id))
                .or_else(|| session.panes.first().map(|pane| pane.id))
        })
    }

    /// The node said what became of a send.
    pub(super) fn send_delivered(&mut self, id: u32, outcome: Outcome, cx: &mut Context<Self>) {
        let Some((_, k, launched, enter)) = self.send.sending.take_if(|(i, ..)| *i == id) else {
            return;
        };
        match send::said(outcome, &k.name) {
            None => {
                sessions::remember(&mut self.settings.recent_targets, &k.text(), 5);
                self.send.files.clear();
                self.send_note.update(cx, |f, cx| f.set("", false, cx));
                self.close(surface::SEND, cx);
                self.report(&send::arrived(&k.name, launched, enter), false, None);
                self.save(cx);
            }
            Some(why) => {
                self.send.stuck = outcome == Outcome::Unknown;
                self.report(&why, true, None);
                self.send.said = Some(why);
            }
        }
        cx.notify();
    }

    /// The drawer's launcher asked `node` for a session (request `id`).
    pub fn send_launching(&mut self, node: &str, id: u32) {
        self.send.launch = Some((node.to_string(), id));
    }

    /// A launch answered: the drawer's own pastes its message, unsubmitted.
    pub(super) fn send_launched(
        &mut self,
        node: &str,
        id: u32,
        ok: bool,
        session: &str,
        cx: &mut Context<Self>,
    ) {
        match send::landed(self.send.launch.as_ref(), node, id, ok, session) {
            send::Landed::NotOurs => return,
            send::Landed::Failed => {}
            send::Landed::Wait(sid) => {
                self.send.new_agent = false;
                self.send.waiting = Some((node.to_string(), sid));
                self.send_waited(cx);
            }
        }
        self.send.launch = None;
        self.launchers[SEND].update(cx, |l, cx| l.done(cx));
        cx.notify();
    }

    /// The fleet changed: a launched session awaiting its message gets it
    /// once it is listed with a pane.
    pub(super) fn send_waited(&mut self, cx: &mut Context<Self>) {
        let Some((node, sid)) = &self.send.waiting else {
            return;
        };
        let Some((key, enter)) = send::launch_paste(&self.fleet.all, node, sid) else {
            return;
        };
        self.send.waiting = None;
        self.send_target = Some(key.clone());
        self.send.pane = None;
        self.deliver(key, enter, true, cx);
    }

    /// Files from this computer, dropped on the drawer or picked.
    fn send_attach(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for p in paths {
            if p.is_dir() {
                self.send.said = Some(format!("{} is a folder; attach files.", p.display()));
            } else if !self.send.files.contains(&Attachment::File(p.clone())) {
                self.send.files.push(Attachment::File(p.clone()));
            }
        }
        cx.notify();
    }

    pub(super) fn send_clipboard(&mut self, item: gpui::ClipboardItem, cx: &mut Context<Self>) {
        // File managers may offer an image preview as well as the original file.
        let has_files = item.entries().iter().any(|e| matches!(e, gpui::ClipboardEntry::ExternalPaths(paths) if !paths.paths().is_empty()));
        for entry in item.into_entries() {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => self.send_attach(paths.paths(), cx),
                gpui::ClipboardEntry::Image(image) if !has_files => {
                    self.send.files.push(Attachment::Image {
                        name: format!("clipboard.{}", image.format.extension()),
                        bytes: image.bytes,
                    });
                }
                gpui::ClipboardEntry::Image(_) | gpui::ClipboardEntry::String(_) => {}
            }
        }
        cx.notify();
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        parvi_app_kit::widgets::path_picker::pick(
            self.host.clone(),
            "Attach",
            window,
            cx,
            |page, paths, cx| page.send_attach(&paths, cx),
        );
    }

    pub(super) fn render_send(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let room = window.viewport_size().width / px(1.);
        let width = send::width(self.settings.send_width, room);
        let busy =
            self.send.sending.is_some() || self.send.uploading || self.send.waiting.is_some();
        let mut body = div()
            .id("send-body")
            .debug_selector(|| "send-body".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(kit::label(&t, "Target"))
            .child(self.render_send_aimed(&t))
            .child(self.send_query.clone())
            .child(self.render_send_targets(cx));
        // The list shows whole rows; say what lies past its end.
        let query = self.send_query.read(cx).text.clone();
        let past =
            (self.send_targets(&query).len() + 1).saturating_sub(size::SEND_TARGET_ROWS as usize);
        if past > 0 {
            body = body.child(kit::note(
                &t,
                format!("{past} more below; scroll, or type to find one"),
                Some(t.dim),
            ));
        }
        if !self.send.new_agent
            && let Some(key) = self.send_target.as_ref()
            && let Some(session) = self.fleet.session(key)
            && (session.panes.len() > 1
                || self
                    .send
                    .pane
                    .is_some_and(|id| !session.panes.iter().any(|p| p.id == id)))
        {
            let selected = self.send_pane(key);
            let mut choices = div().flex().flex_wrap().gap(px(6.));
            for (index, pane) in session.panes.iter().enumerate() {
                let id = pane.id;
                let label = format!("Pane {} · %{}", index + 1, id);
                choices = choices.child(
                    kit::button(
                        &t,
                        ("send-pane", index),
                        label,
                        if selected == Some(id) {
                            Kind::Primary
                        } else {
                            Kind::Quiet
                        },
                    )
                    .debug_selector(move || {
                        if selected == Some(id) {
                            format!("send-pane-selected-{index}")
                        } else {
                            format!("send-pane-{index}")
                        }
                    })
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.send.pane = Some(id);
                        s.send.said = None;
                        cx.notify();
                    })),
                );
            }
            body = body.child(kit::label(&t, "Pane")).child(choices);
        }
        if self.send.new_agent {
            body = body
                .child(kit::note(
                    &t,
                    "The new agent gets the message pasted, not sent: press Enter there once it is ready.",
                    Some(t.dim),
                ))
                .child(self.launchers[SEND].clone());
        }
        body = body.child(div().mt(px(8.)).child(kit::label(&t, "Message")));
        if let Some(r) = &self.send.reference {
            body = body.child(
                div()
                    .id("send-reference")
                    .flex_none()
                    .px(px(8.))
                    .py(px(4.))
                    .bg(t.raised)
                    .rounded(px(size::RADIUS))
                    .border_1()
                    .border_color(t.divider)
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(r.clone()),
            );
        }
        body = body
            .child(self.send_note.clone())
            .child(self.render_send_files(&t, cx))
            .child(kit::note(
                &t,
                "Enter sends · Shift+Enter new line · Ctrl+Enter pastes · drop or paste files to attach",
                Some(t.dim),
            ));
        if let Some(why) = &self.send.said {
            body = body.child(kit::note(&t, why.clone(), Some(t.error)));
        }
        // Nothing goes until a target is chosen, and the button says which.
        let aimed = (!self.send.new_agent)
            .then(|| self.send_target.clone())
            .flatten();
        let blocked = busy || self.send.stuck || aimed.is_none();
        let go = if self.send.uploading {
            "Attaching…".to_string()
        } else if busy {
            "Sending…".to_string()
        } else {
            match &aimed {
                Some(k) => format!("Send to {}", k.name),
                None => "Pick a target".to_string(),
            }
        };
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.))
                .child(
                    kit::button(&t, "send-attach", "Attach…", Kind::Quiet)
                        .flex_none()
                        .on_click(
                            cx.listener(|s, _: &ClickEvent, window, cx| s.pick_files(window, cx)),
                        ),
                )
                .child(div().flex_1())
                .child(
                    kit::button(&t, "send-paste", "Paste only", Kind::Plain)
                        .flex_none()
                        .when(blocked, |b| b.opacity(0.5))
                        .on_click(cx.listener(|s, _: &ClickEvent, window, cx| {
                            s.send_now(false, window, cx)
                        })),
                )
                .child(
                    kit::button(&t, "send-go", go, Kind::Primary)
                        .flex_none()
                        .debug_selector(|| "send-go".into())
                        .when(blocked, |b| b.opacity(0.5))
                        .on_click(cx.listener(|s, _: &ClickEvent, window, cx| {
                            s.send_now(true, window, cx)
                        })),
                ),
        );
        let left = self.send.left;
        let drawer = div()
            .id("send")
            .debug_selector(move || {
                if left {
                    "send-drawer-left"
                } else {
                    "send-drawer"
                }
                .into()
            })
            .absolute()
            .top_0()
            .bottom_0()
            .when(left, |d| d.left_0().border_r_1().flex_row_reverse())
            .when(!left, |d| d.right_0().border_l_1())
            .w(px(width))
            .flex()
            .bg(t.surface)
            .border_color(t.divider)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.bg(alpha(t.accent, 0.08)))
            .on_drop(
                cx.listener(|s, paths: &ExternalPaths, _, cx| s.send_attach(paths.paths(), cx)),
            )
            .child(
                div()
                    .id("send-grip")
                    .flex_none()
                    .w(px(size::DRAWER_GRIP))
                    .h_full()
                    .cursor_col_resize()
                    .hover(|d| d.bg(t.accent_soft()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|s, _, _, cx| {
                            s.send.resizing = true;
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.render_send_head(&t, cx))
                    .child(body),
            );
        let resizing = self.send.resizing;
        div()
            .absolute()
            .inset_0()
            .child(self.scrim(surface::SEND, cx).bg(alpha(t.bg, 0.3)))
            .child(drawer)
            .when(resizing, |d| {
                d.cursor_col_resize()
                    .on_mouse_move(cx.listener(move |s, e: &MouseMoveEvent, _, cx| {
                        let x = e.position.x / px(1.);
                        let w = if left { x } else { room - x };
                        s.settings.send_width = Some(send::width(Some(w), room));
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|s, _, _, cx| {
                            s.send.resizing = false;
                            s.save(cx);
                            cx.notify();
                        }),
                    )
            })
            .into_any_element()
    }

    /// The left and right edge of the window showing the session Send aims
    /// at, as last painted.
    fn aimed_span(&self) -> Option<(f32, f32)> {
        let k = self.send_target.as_ref().filter(|_| !self.send.new_agent)?;
        let geo = self.geo.borrow();
        let ws = &self.settings.workspace;
        (0..ws.windows().len())
            .find(|&i| ws.shown(i).is_some_and(|b| &b.key == k))
            .and_then(|i| geo.get(i).and_then(|g| g.body))
            .map(|b| {
                let l = f32::from(b.origin.x);
                (l, l + f32::from(b.size.width))
            })
    }

    /// The target, named where the drawer starts: what Enter sends to.
    fn render_send_aimed(&self, t: &crate::theme::Theme) -> gpui::Div {
        let aimed = (!self.send.new_agent)
            .then(|| self.send_target.clone())
            .flatten();
        let line = div()
            .flex_none()
            .h(px(size::ROW))
            .flex()
            .items_center()
            .gap(px(6.));
        match aimed {
            Some(k) => {
                let name = k.name.clone();
                let focused = self.focused_key().as_ref() == Some(&k);
                line.debug_selector(move || format!("send-aimed-{name}"))
                    .child(div().text_color(t.text2).child("To"))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(k.name),
                    )
                    .when(focused, |d| {
                        d.child(
                            div()
                                .text_size(px(size::SMALL))
                                .text_color(t.dim)
                                .child("the focused window"),
                        )
                    })
            }
            None if self.send.new_agent => {
                line.child(div().text_color(t.text2).child("To a new agent"))
            }
            None => line.debug_selector(|| "send-aimed-none".into()).child(
                div()
                    .text_color(t.text2)
                    .child("No target yet: pick one below"),
            ),
        }
    }

    fn render_send_head(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> gpui::Div {
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
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Send to session"),
            )
            .child(
                kit::button(t, "send-close", "Close  Esc", Kind::Quiet)
                    .on_click(cx.listener(|s, _, _, cx| s.close(surface::SEND, cx))),
            )
    }

    /// The picker: a new agent first, then the sessions, recent ones first.
    fn render_send_targets(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = theme(cx).clone();
        let query = self.send_query.read(cx).text.clone();
        let focused = self.focused_key();
        let target = (!self.send.new_agent)
            .then(|| self.send_target.clone())
            .flatten();
        let row = |id: (&'static str, usize), chosen: bool| {
            div()
                .id(id)
                // Rows keep their height; the list scrolls instead of
                // squeezing them.
                .flex_none()
                .h(px(size::ROW))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .when(chosen, |d| {
                    d.bg(t.accent_light())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                })
                .hover(|d| d.bg(t.accent_soft()))
        };
        let mut targets = div()
            .id("send-targets")
            .flex_none()
            // Whole rows, so the last one shown is never cut through.
            .max_h(px(size::SEND_TARGET_ROWS * size::ROW))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                row(("target-new", 0), self.send.new_agent)
                    .debug_selector(|| "target-new".into())
                    .child(div().flex_1().text_color(t.text2).child("New agent…"))
                    .on_click(cx.listener(|s, _, window, cx| s.pick(None, window, cx))),
            );
        let list = self.send_targets(&query);
        let cursor = self.send_cursor.min(list.len().saturating_sub(1));
        for (i, s) in list.iter().enumerate() {
            let key = Key::of(s);
            let k = key.clone();
            let selector = format!("send-target-{}", key.text());
            targets = targets.child(
                row(("target", i), target.as_ref() == Some(&key))
                    .debug_selector(move || selector.clone())
                    .when(i == cursor && !query.is_empty(), |d| d.bg(t.raised))
                    .when_some(s.user.clone(), |d, u| d.child(kit::user_square(&t, &u)))
                    .when(sessions::waiting(s).is_some(), |d| {
                        d.child(kit::waiting_dot(&t))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(s.name.clone()),
                    )
                    .when(focused.as_ref() == Some(&key), |d| {
                        d.child(
                            div()
                                .text_size(px(10.))
                                .text_color(t.dim)
                                .child("focused window"),
                        )
                    })
                    .on_click(
                        cx.listener(move |sh, _, window, cx| sh.pick(Some(k.clone()), window, cx)),
                    ),
            );
        }
        targets
    }

    /// The attached files, each with a way to take it off again.
    fn render_send_files(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap(px(2.));
        for (i, p) in self.send.files.iter().enumerate() {
            let (name, size) = match p {
                Attachment::File(path) => (
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    std::fs::metadata(path)
                        .map(|m| send::bytes(m.len()))
                        .unwrap_or_default(),
                ),
                Attachment::Image { name, bytes } => {
                    (name.clone(), send::bytes(bytes.len() as u64))
                }
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(size::SMALL))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(name))
                    .child(div().text_color(t.dim).child(size))
                    .child(
                        kit::button(t, ("send-unattach", i), "Remove", Kind::Quiet).on_click(
                            cx.listener(move |s, _: &ClickEvent, _, cx| {
                                if i < s.send.files.len() {
                                    s.send.files.remove(i);
                                }
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        list
    }
}

/// Writes each file on the target's node, in order: their paths there, or
/// why one could not be.
async fn write_drops(
    page: &WeakEntity<Terminals>,
    node: &NodeId,
    user: &str,
    files: &[Attachment],
    cx: &mut AsyncApp,
) -> Result<Vec<String>, String> {
    let mut paths = vec![];
    for f in files {
        let (name, bytes) = match f {
            Attachment::File(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let path = path.clone();
                let bytes = cx
                    .background_spawn(async move { std::fs::read(path) })
                    .await
                    .map_err(|e| format!("{name} could not be read: {e}"))?;
                (name, bytes)
            }
            Attachment::Image { name, bytes } => (name.clone(), bytes.clone()),
        };
        if bytes.len() > http::MAX_DROP {
            return Err(format!("{name} is over 256 MiB; copy it in Files instead."));
        }
        let task = page
            .update(cx, |s, cx| {
                s.host
                    .request(http::drop_file(node, user, &name, bytes), cx)
            })
            .map_err(|_| "the page closed".to_string())?;
        let path =
            http::dropped(task.await).map_err(|e| format!("{name} could not be attached: {e}"))?;
        paths.push(path);
    }
    Ok(paths)
}
