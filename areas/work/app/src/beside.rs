//! An on-demand item card beside the page that asked for it.
use crate::{Work, tokens::*};
use gpui::{AppContext, Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use parvi_app_kit::host::Request;
use parvi_app_kit::theme::{FAMILY, theme};
use parvi_app_kit::widgets::kit;
use parvi_core::NodeId;
use parvi_work_contract::{Item, ProjectView, Snapshot, StoreView};

struct Card {
    work: Entity<Work>,
    error: Option<String>,
    loading: bool,
}

impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let card = self.work.update(cx, |work, cx| work.card(cx));
        div()
            .id("work-beside")
            .debug_selector(|| "work-beside".into())
            .w(px(CARD_W))
            .min_w(px(CARD_MIN))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.divider)
            .font_family(FAMILY)
            .when(self.loading, |d| {
                d.child(kit::note(&t, "Reading item…", None))
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("work-beside-error")
                        .debug_selector({
                            let error = error.clone();
                            move || format!("work-beside-error-{error}")
                        })
                        .p(px(SPACE_4XL))
                        .child(kit::note(&t, "Item not found", Some(t.error)))
                        .child(error),
                )
            })
            .children(card)
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl Work {
    pub(crate) fn open_beside(
        &mut self,
        from: &'static str,
        id: String,
        node: Option<NodeId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = node
            .or_else(|| self.host.nodes(cx).first().map(|n| n.id.clone()))
            .unwrap_or_else(|| NodeId(String::new()));
        let projects: Vec<ProjectView> = self
            .snapshots
            .get(node.as_str())
            .map(|s| s.projects.clone())
            .unwrap_or_default();
        let host = self.host.clone();
        let card_work = cx.new(|cx| Work::new(host.clone(), window, cx));
        card_work.update(cx, |w, _| w.beside_from = Some(from));
        let card = cx.new(|cx| {
            cx.observe(&card_work, |_, _, cx| cx.notify()).detach();
            Card {
                work: card_work.clone(),
                error: None,
                loading: true,
            }
        });
        self.beside_from = Some(from);
        host.show_beside(from, card.clone().into(), cx);
        let task = host.request(
            Request::get(node.clone(), format!("/api/work/item?ref={}", encode(&id))),
            cx,
        );
        // The Work page may not be watched while the asker is shown. Ask the
        // node for project metadata so a GitHub card can offer its checkout.
        let projects_task = host.request(Request::new("POST", node.clone(), "/mcp").json(&serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"work_list", "arguments":{}}})), cx);
        cx.spawn(async move |_, cx| {
            let answer = task.await;
            let result = match answer {
                Ok(answer) if answer.ok() => {
                    serde_json::from_slice::<serde_json::Value>(&answer.body)
                        .map_err(|e| e.to_string())
                        .and_then(|v| {
                            let line = v["line"].as_str().unwrap_or("").to_owned();
                            serde_json::from_value::<Item>(v["item"].clone())
                                .map(|item| (item, line))
                                .map_err(|e| e.to_string())
                        })
                }
                Ok(answer) => Err(answer.error_text()),
                Err(error) => Err(error.message),
            };
            let mut projects = projects;
            if let Ok(answer) = projects_task.await
                && answer.ok()
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&answer.body)
                && let Some(text) = value["result"]["content"][0]["text"].as_str()
                && let Ok(snapshot) = serde_json::from_str::<Snapshot>(text)
            {
                projects = snapshot.projects;
            }
            card.update(cx, |card, cx| {
                card.loading = false;
                match result {
                    Ok((item, line)) => card.work.update(cx, |w, cx| {
                        w.board = crate::store::work::Board::from_item(
                            item,
                            &projects,
                            parvi_app_kit::hub::now_ms(),
                        );
                        // One project, on the node that answered, for the existing card actions.
                        w.snapshots.insert(
                            node.to_string(),
                            Snapshot {
                                projects: vec![ProjectView::Beads(StoreView::default())],
                            },
                        );
                        w.reslice();
                        w.card_line = Some(line);
                        w.select(0, cx);
                    }),
                    Err(error) => card.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
