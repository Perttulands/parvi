//! Brain's page entity and its requests; state and drawing stay separate.
use crate::state::{self, Brain, SETTLE_MS};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, ScrollHandle, WeakEntity, Window,
};
use parvi_app_kit::widgets::select::Selectable;
use parvi_app_kit::{
    Answers, Ask, Host, Page,
    host::Request,
    link::client::LinkState,
    widgets::{
        code::{Code, CodeEvent},
        field::{FieldEvent, TextField},
    },
};
use parvi_core::NodeId;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc, time::Duration};

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    hidden: Vec<String>,
}

pub struct BrainPage {
    pub(crate) host: Host,
    me: WeakEntity<BrainPage>,
    pub(crate) state: Brain,
    pub(crate) field: Entity<TextField>,
    pub(crate) editor: Entity<Code>,
    pub(crate) summary: Entity<TextField>,
    pub(crate) select: Rc<RefCell<Selectable>>,
    pub(crate) anchor: Rc<RefCell<Option<String>>>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) reader_focus: FocusHandle,
    node: NodeId,
    shown: bool,
}

impl Page for BrainPage {
    const ID: &'static str = "brain";
    const TITLE: &'static str = "Brain";
    const AREA: Option<&'static str> = Some("brain");
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("brain")];

    fn chords(&self, _: &App) -> Vec<parvi_app_kit::chords::Chord> {
        let me = self.me.clone();
        vec![
            parvi_app_kit::chords::Chord::new("l", "Brain", move |_, cx| {
                let _ = me.update(cx, |page, cx| page.toggle_dock(cx));
            })
            .alt("l", Some(false))
            .everywhere(),
        ]
    }

    fn focus(&self, cx: &App) -> Option<FocusHandle> {
        Some(self.field.read(cx).focus_handle(cx))
    }
    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            self.brain_read_rail(cx);
        }
    }
    fn link(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (self.shown || self.host.docked(cx))
            && state == Some(LinkState::Open)
            && (self.node.as_str().is_empty() || &self.node == node)
        {
            self.brain_read_rail(cx);
        }
    }
    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if let Ask::Reveal { id, node, .. } = ask {
            let next = node.unwrap_or_else(|| NodeId(String::new()));
            if self.node != next {
                self.state.close();
                self.state.typed("");
                self.field.update(cx, |f, cx| f.set("", false, cx));
                self.state.collections = None;
                self.state.arrivals.clear();
                self.state.open_collection = None;
                self.node = next;
                self.brain_read_rail(cx);
            }
            self.brain_open(&id, cx);
        }
    }
}

impl BrainPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(cx, "Search the brain…"));
        cx.subscribe_in(&field, window, |page, field, event, _, cx| match event {
            FieldEvent::Changed => {
                let text = field.read(cx).text.clone();
                page.brain_typed(&text, cx);
            }
            FieldEvent::Up => page.brain_step(-1, cx),
            FieldEvent::Down => page.brain_step(1, cx),
            FieldEvent::Submit => page.brain_step(0, cx),
            _ => {}
        })
        .detach();
        let editor = cx.new(Code::new);
        cx.subscribe_in(&editor, window, |page, _, event, window, cx| match event {
            CodeEvent::Save => page.brain_save(cx),
            CodeEvent::Escape => page.brain_discard(window, cx),
            CodeEvent::Changed => {}
        })
        .detach();
        let summary = cx.new(|cx| TextField::new(cx, "What this edit changes"));
        cx.subscribe_in(&summary, window, |page, _, event, _, cx| {
            if let FieldEvent::Submit = event {
                page.brain_save(cx);
            }
        })
        .detach();
        let settings: Settings = host.setting(cx).unwrap_or_default();
        let mut state = Brain::default();
        state.hidden = settings.hidden.into_iter().collect();
        Self {
            host,
            me: cx.weak_entity(),
            state,
            field,
            editor,
            summary,
            select: Rc::default(),
            anchor: Rc::default(),
            scroll: ScrollHandle::new(),
            reader_focus: cx.focus_handle(),
            node: NodeId(String::new()),
            shown: false,
        }
    }
    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        if self.host.docked(cx) {
            self.host.close_dock(cx);
        } else if let Some(page) = self.me.upgrade() {
            self.brain_read_rail(cx);
            let dock = cx.new(|cx| crate::view::Dock::new(page, cx));
            self.host.show_dock(dock.into(), cx);
        }
    }
    pub(crate) fn save_hidden(&self, cx: &mut Context<Self>) {
        let settings = Settings {
            hidden: self.state.hidden.iter().cloned().collect(),
        };
        if let Err(error) = self.host.set_setting(&settings, cx) {
            self.host.toast(&error.message, cx);
        }
    }
    pub(crate) fn node_request(
        &mut self,
        _: &str,
        method: &'static str,
        path: &str,
        body: Option<Vec<u8>>,
        done: impl FnOnce(&mut Self, u16, Vec<u8>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        debug_assert!(body.is_none());
        let requested_node = self.node.clone();
        let request = Request::new(method, requested_node.clone(), path);
        let task = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let (status, body) = match task.await {
                Ok(answer) => (answer.status, answer.body),
                Err(error) => (0, error.message.into_bytes()),
            };
            let _ = this.update(cx, |page, cx| {
                if page.node != requested_node {
                    return;
                }
                done(page, status, body, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn brain_typed(&mut self, query: &str, cx: &mut Context<Self>) {
        let Some(asked) = self.state.typed(query) else {
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SETTLE_MS))
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.state.asked != asked {
                    return;
                }
                let path = format!("/api/brain/search?q={}", state::encode(&page.state.query));
                page.node_request(
                    "",
                    "GET",
                    &path,
                    None,
                    move |page, status, body, _| {
                        page.state.found(asked, status, &body);
                    },
                    cx,
                );
            });
        })
        .detach();
        cx.notify();
    }
    fn brain_step(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(path) = self.state.step(by) {
            self.brain_open(&path, cx);
        }
    }
    pub(crate) fn brain_open(&mut self, path: &str, cx: &mut Context<Self>) {
        self.brain_open_at(path, None, cx);
    }
    pub(crate) fn brain_open_at(
        &mut self,
        path: &str,
        anchor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let reading = self.state.open();
        let path = format!("/api/brain/page?path={}", state::encode(path));
        self.node_request(
            "",
            "GET",
            &path,
            None,
            move |page, status, body, _| {
                if page.state.read(reading, status, &body) && status == 200 {
                    page.scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                    *page.anchor.borrow_mut() = anchor;
                    page.select.borrow_mut().selection.clear();
                }
            },
            cx,
        );
        cx.notify();
    }
    pub(crate) fn brain_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.start_edit() {
            return;
        }
        let Some(page) = &self.state.page else {
            return;
        };
        let text = page.text();
        let summary = format!("Edit {}", page.title);
        self.summary
            .update(cx, |field, cx| field.set(&summary, false, cx));
        self.editor.update(cx, |editor, cx| {
            editor.set(&text, cx);
            editor.edit(true, window, cx);
        });
        Code::focus(&self.editor, window, cx);
        cx.notify();
    }
    pub(crate) fn brain_discard(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.discard() {
            let id = self.state.edit.as_ref().map(|edit| edit.id);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = this.update(cx, |page, cx| {
                    if let Some(edit) = &mut page.state.edit
                        && Some(edit.id) == id
                    {
                        edit.discarding = false;
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }
    pub(crate) fn brain_save(&mut self, cx: &mut Context<Self>) {
        let (Some(page), Some(edit)) = (&self.state.page, &mut self.state.edit) else {
            return;
        };
        if edit.saving {
            return;
        }
        edit.saving = true;
        let id = edit.id;
        let path = page.path.clone();
        let text = self.editor.read(cx).text().to_string();
        let summary = self.summary.read(cx).text.trim().to_string();
        let body = serde_json::json!({"path": path, "content": text, "summary": summary});
        let node = self.node.clone();
        let request = Request::new("PUT", node.clone(), "/api/brain/page")
            .json(&body)
            .within(Duration::from_secs(150));
        let task = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let (status, body) = match task.await {
                Ok(answer) => (answer.status, answer.body),
                Err(error) => (0, error.message.into_bytes()),
            };
            let _ = this.update(cx, |page, cx| {
                if page.node != node {
                    return;
                }
                let keep = page.editor.read(cx).text() != text;
                match page.state.saved(id, &path, &text, keep, status, &body) {
                    Ok(commit) => {
                        page.host
                            .toast(&format!("Shelved {path} · {}", commit.message), cx);
                        page.brain_read_rail(cx);
                    }
                    Err(why) if status == 0 => page.host.toast(
                        &format!("{path}: save unconfirmed; check before retrying: {why}"),
                        cx,
                    ),
                    Err(why) => page.host.toast(&format!("{path} was not saved: {why}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
