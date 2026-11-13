//! An item beside the page that named it (a terminal's Bead id): from the
//! board the page holds when it holds the item, at once; else read from
//! its node with its family's titles.
use crate::Work;
use crate::store::work::Board;
use crate::view::Place;
use gpui::{AppContext, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use parvi_app_kit::host::Request;
use parvi_app_kit::theme::{FAMILY, theme};
use parvi_app_kit::widgets::pane;
use parvi_core::NodeId;
use parvi_work_contract::{Item, ProjectView, Snapshot, StoreView};
use std::rc::Rc;
use std::time::Duration;

struct Card {
    work: Entity<Work>,
    error: Option<(String, bool)>,
    loading: bool,
}

impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let detail = self.work.update(cx, |work, cx| {
            work.shown_row()
                .map(|row| work.detail(&t, row, Place::Beside, cx))
        });
        let words = match &self.error {
            Some((_, true)) => Some(("work-beside-not-found", "Item not found")),
            Some((_, false)) => Some(("work-beside-failed", "Could not read item")),
            None if self.loading => Some(("work-beside-loading", "Reading item…")),
            None => None,
        };
        div()
            .id("work-beside")
            .debug_selector(|| "work-beside".into())
            // The page that shows it beside decides the width.
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.divider)
            .font_family(FAMILY)
            .text_color(t.text)
            .when_some(
                words.filter(|_| detail.is_none()),
                |d, (selector, words)| {
                    let error = self.error.clone().map(|(e, _)| e);
                    let detail_error = error.clone();
                    d.child(
                        pane::head(&t)
                            .child(pane::title(&t, words).debug_selector(move || selector.into())),
                    )
                    .child(
                        pane::empty(&t, error.unwrap_or_default(), None)
                            .when_some(detail_error, |d, e| {
                                d.debug_selector(move || format!("work-beside-error-{e}"))
                            }),
                    )
                },
            )
            .children(detail)
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
        let host = self.host.clone();
        let card_work = cx.new(|cx| Work::new(host.clone(), window, cx));
        self.beside_from = Some(from);
        // The page's board holds the item: it shows now, family and all.
        if let Some(row) = self.find_on(&id, Some(node.as_str())) {
            let (board, snapshots) = (self.board.clone(), self.snapshots.clone());
            card_work.update(cx, |w, cx| {
                w.beside_from = Some(from);
                w.board = board;
                w.snapshots = snapshots;
                w.select(row, cx);
            });
            let card = cx.new(|cx| {
                cx.observe(&card_work, |_, _, cx| cx.notify()).detach();
                Card {
                    work: card_work.clone(),
                    error: None,
                    loading: false,
                }
            });
            host.show_beside(from, card.into(), cx);
            return;
        }
        card_work.update(cx, |w, _| w.beside_from = Some(from));
        let card = cx.new(|cx| {
            cx.observe(&card_work, |_, _, cx| cx.notify()).detach();
            Card {
                work: card_work.clone(),
                error: None,
                loading: true,
            }
        });
        host.show_beside(from, card.clone().into(), cx);
        let task = host.request(
            Request::get(node.clone(), format!("/api/work/item?ref={}", encode(&id)))
                .within(Duration::from_secs(95)),
            cx,
        );
        // Whatever the page knows of the node's projects names a GitHub
        // item's checkout.
        let mut projects: Vec<ProjectView> = self
            .snapshots
            .get(node.as_str())
            .map(|s| s.projects.clone())
            .unwrap_or_default();
        cx.spawn(async move |_, cx| {
            let result = match task.await {
                Ok(answer) if answer.ok() => {
                    serde_json::from_slice::<serde_json::Value>(&answer.body)
                        .map_err(|e| (e.to_string(), false))
                        .and_then(|v| {
                            let line = v["line"].as_str().unwrap_or("").to_owned();
                            let family: Vec<Item> =
                                serde_json::from_value(v["family"].clone()).unwrap_or_default();
                            serde_json::from_value::<Item>(v["item"].clone())
                                .map(|item| (item, line, family))
                                .map_err(|e| (e.to_string(), false))
                        })
                }
                Ok(answer) => {
                    let not_found = serde_json::from_slice::<serde_json::Value>(&answer.body)
                        .ok()
                        .is_some_and(|v| v["error"]["code"] == "not_found");
                    Err((answer.error_text(), not_found))
                }
                Err(error) => Err((error.message, false)),
            };
            card.update(cx, |card, cx| {
                card.loading = false;
                match result {
                    Err(error) => card.error = Some(error),
                    Ok((item, line, family)) => {
                        let unread: Vec<ProjectView> = family_stores(family)
                            .into_iter()
                            .filter(|f| {
                                !projects.iter().any(|p| match (p, f) {
                                    (ProjectView::Beads(p), ProjectView::Beads(f)) => {
                                        p.folder == f.folder && p.items.is_some()
                                    }
                                    _ => false,
                                })
                            })
                            .collect();
                        projects.extend(unread);
                        card.work.update(cx, |w, cx| {
                            w.board = Rc::new(Board::from_item(item, &projects));
                            // All rows in this board came from the answering node.
                            w.snapshots.insert(
                                node.to_string(),
                                std::sync::Arc::new(Snapshot {
                                    refreshed: 0,
                                    projects: vec![
                                        ProjectView::Beads(StoreView::default());
                                        w.board.projects.len()
                                    ],
                                }),
                            );
                            w.card_line = Some((w.board.rows[0].reference.clone(), line));
                            w.select(0, cx);
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The family's items as the stores that hold them, by the folder their
/// references name.
fn family_stores(family: Vec<Item>) -> Vec<ProjectView> {
    let mut stores: Vec<StoreView> = Vec::new();
    for item in family {
        let Some(folder) = item
            .reference
            .strip_prefix("bead:")
            .and_then(|r| r.split_once(':'))
            .and_then(|(_, r)| r.rsplit_once('#'))
            .map(|(f, _)| f.to_owned())
        else {
            continue;
        };
        match stores.iter_mut().find(|s| s.folder == folder) {
            Some(s) => s.items.get_or_insert_with(Vec::new).push(item),
            None => stores.push(StoreView {
                folder,
                items: Some(vec![item]),
                ..Default::default()
            }),
        }
    }
    stores.into_iter().map(ProjectView::Beads).collect()
}
