//! Brain's page entity and its requests; state and drawing stay separate.
use crate::state::{self, Brain, SETTLE_MS};
use gpui::{AppContext, Context, Entity, Window};
use parvi_app_kit::{
    Answers, Ask, Host, Page,
    host::Request,
    link::client::LinkState,
    widgets::field::{FieldEvent, TextField},
};
use parvi_core::NodeId;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    hidden: Vec<String>,
}

pub struct BrainPage {
    pub(crate) host: Host,
    pub(crate) state: Brain,
    pub(crate) field: Entity<TextField>,
    node: NodeId,
    shown: bool,
}

impl Page for BrainPage {
    const ID: &'static str = "brain";
    const TITLE: &'static str = "Brain";
    const AREA: Option<&'static str> = Some("brain");
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("brain")];

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
        if self.shown
            && state == Some(LinkState::Open)
            && (self.node.as_str().is_empty() || &self.node == node)
        {
            self.brain_read_rail(cx);
        }
    }
    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if let Ask::Reveal { id, node, .. } = ask {
            self.node = node.unwrap_or_else(|| NodeId(String::new()));
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
        let settings: Settings = host.setting(cx).unwrap_or_default();
        let mut state = Brain::default();
        state.hidden = settings.hidden.into_iter().collect();
        Self {
            host,
            state,
            field,
            node: NodeId(String::new()),
            shown: false,
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
        let request = Request::new(method, self.node.clone(), path);
        let task = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let (status, body) = match task.await {
                Ok(answer) => (answer.status, answer.body),
                Err(error) => (0, error.message.into_bytes()),
            };
            let _ = this.update(cx, |page, cx| {
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
        let reading = self.state.open();
        let path = format!("/api/brain/page?path={}", state::encode(path));
        self.node_request(
            "",
            "GET",
            &path,
            None,
            move |page, status, body, _| {
                page.state.read(reading, status, &body);
            },
            cx,
        );
        cx.notify();
    }
}
