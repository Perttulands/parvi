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
    AnyElement, AsyncApp, ClickEvent, Context, ExternalPaths, FocusHandle, FontWeight, MouseButton,
    MouseMoveEvent, PathPromptOptions, WeakEntity, Window, div, prelude::*, px,
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
    /// What had the focus when the drawer opened.
    pub opener: Option<FocusHandle>,
    /// The inner edge is being dragged.
    pub resizing: bool,
}

/// A local file or a clipboard image kept in memory until sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    File(PathBuf),
    Image { name: String, bytes: Vec<u8> },
}

impl Terminals {
    /// Opens Send to Session aimed at `target`, with nothing in hand.
    pub fn open_send(&mut self, target: Option<Key>, window: &mut Window, cx: &mut Context<Self>) {
        self.handoff(None, target, window, cx);
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
        if self.send.sending.is_some() || self.send.uploading || self.send.waiting.is_some() {
            // The last one is still on its way: its draft stays until it lands.
            self.open(Surface::Send, window, cx);
            return;
        }
        let opener = window.focused(cx);
        self.send = Draft {
            reference,
            opener,
            ..Default::default()
        };
        self.send_target = target;
        self.send_cursor = 0;
        self.send_query.update(cx, |f, cx| f.set("", false, cx));
        self.send_note.update(cx, |f, cx| f.set("", false, cx));
        self.open(Surface::Send, window, cx);
    }

    /// The drawer closed: the focus goes back to what had it.
    pub(super) fn send_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.send.opener.take() {
            Some(f) if self.surfaces.is_empty() => {
                window.focus(&f, cx);
                true
            }
            _ => false,
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
            Ok(id) => self.send.sending = Some((id, k, launched, enter)),
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

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let answer = picked.await;
            let _ = this.update(cx, |s, cx| match answer {
                Ok(Ok(Some(paths))) => s.send_attach(&paths, cx),
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(e)) => s.report(&format!("The file picker failed: {e}"), true, None),
            });
        })
        .detach();
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
            .child(self.send_query.clone())
            .child(self.render_send_targets(cx));
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
                    .border_l_2()
                    .border_color(t.accent)
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .overflow_hidden()
                    .whitespace_nowrap()
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
        let blocked = busy || self.send.stuck || self.send.new_agent;
        let go = if self.send.uploading {
            "Attaching…"
        } else if busy {
            "Sending…"
        } else {
            "Send"
        };
        body = body.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    kit::button(&t, "send-attach", "Attach…", Kind::Quiet)
                        .on_click(cx.listener(|s, _: &ClickEvent, _, cx| s.pick_files(cx))),
                )
                .child(div().flex_1())
                .child(
                    kit::button(&t, "send-paste", "Paste only", Kind::Plain)
                        .when(blocked, |b| b.opacity(0.5))
                        .on_click(cx.listener(|s, _: &ClickEvent, window, cx| {
                            s.send_now(false, window, cx)
                        })),
                )
                .child(
                    kit::button(&t, "send-go", go, Kind::Primary)
                        .debug_selector(|| "send-go".into())
                        .when(blocked, |b| b.opacity(0.5))
                        .on_click(cx.listener(|s, _: &ClickEvent, window, cx| {
                            s.send_now(true, window, cx)
                        })),
                ),
        );
        let drawer = div()
            .id("send")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(width))
            .flex()
            .bg(t.surface)
            .border_l_1()
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
                        let w = room - e.position.x / px(1.);
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
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("SEND TO SESSION"),
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
                .h(px(size::ROW))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .when(chosen, |d| {
                    d.bg(t.accent_soft()).border_l_2().border_color(t.accent)
                })
                .hover(|d| d.bg(t.accent_light()))
        };
        let mut targets = div()
            .id("send-targets")
            .flex_none()
            .max_h(px(260.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                row(("target-new", 0), self.send.new_agent)
                    .debug_selector(|| "target-new".into())
                    .child(div().flex_1().text_color(t.text2).child("New agent…"))
                    .on_click(cx.listener(|s, _, window, cx| s.pick(None, window, cx))),
            );
        let list = sessions::targets(&self.fleet.all, &self.settings.recent_targets, &query);
        let cursor = self.send_cursor.min(list.len().saturating_sub(1));
        for (i, s) in list.iter().enumerate() {
            let key = Key::of(s);
            let k = key.clone();
            targets = targets.child(
                row(("target", i), target.as_ref() == Some(&key))
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
                                .child("focused tile"),
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
