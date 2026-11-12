//! A quick report over whichever page the operator is using.
use crate::{Work, tokens::*};
use gpui::{AppContext, Context, Entity, Focusable, Render, Window, div, prelude::*, px};
use parvi_app_kit::Host;
use parvi_app_kit::ask::Ask;
use parvi_app_kit::host::{Layer, Request, Status};
use parvi_app_kit::theme::{FAMILY, theme};
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_core::NodeId;
use parvi_work_contract::{ProjectView, Snapshot};
use serde_json::{Value, json};

const LAYER: &str = "report";

#[derive(Clone, PartialEq, Eq)]
struct Project {
    node: NodeId,
    name: String,
}

pub(crate) struct Report {
    host: Host,
    field: Entity<TextField>,
    sessions: Option<Entity<parvi_terminals_contract::SessionList>>,
    projects: Vec<Project>,
    selected: Option<Project>,
    preferred: Option<Project>,
    pending_projects: usize,
    context: String,
    error: Option<String>,
    filing: bool,
}

impl Report {
    fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(cx, "Title, then description").prose());
        cx.subscribe_in(
            &field,
            window,
            |report: &mut Self, _, event: &FieldEvent, _, cx| {
                if *event == FieldEvent::SubmitAlt {
                    report.file(cx);
                }
            },
        )
        .detach();
        let sessions = host.model::<parvi_terminals_contract::SessionList>(cx);
        Self {
            host,
            field,
            sessions,
            projects: Vec::new(),
            selected: None,
            preferred: None,
            pending_projects: 0,
            context: String::new(),
            error: None,
            filing: false,
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        self.context = self
            .sessions
            .as_ref()
            .and_then(|s| s.read(cx).focused.as_ref())
            .map(|s| format!("Context: {}", s.name))
            .unwrap_or_else(|| "Context: Parvi".into());
        self.error = None;
        self.projects.clear();
        let remembered: Value = self.host.setting(cx).unwrap_or(Value::Null);
        self.preferred = remembered.get("report_project").and_then(|p| {
            Some(Project {
                node: NodeId(p["node"].as_str()?.into()),
                name: p["name"].as_str()?.into(),
            })
        });
        self.selected = None;
        let nodes = self.host.nodes(cx);
        self.pending_projects = nodes.len();
        for node in nodes {
            let task = self.host.request(Request::new("POST", node.id.clone(), "/mcp").json(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"work_list", "arguments":{}}})), cx);
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |report, cx| {
                    let parsed = result
                        .ok()
                        .and_then(|answer| serde_json::from_slice::<Value>(&answer.body).ok())
                        .and_then(|v| {
                            v["result"]["content"][0]["text"]
                                .as_str()
                                .and_then(|s| serde_json::from_str::<Snapshot>(s).ok())
                        });
                    if let Some(snapshot) = parsed {
                        report
                            .projects
                            .extend(snapshot.projects.into_iter().map(|p| Project {
                                node: node.id.clone(),
                                name: match p {
                                    ProjectView::Beads(s) => s.folder,
                                    ProjectView::Github(r) => r.name,
                                },
                            }));
                        if report.selected.is_none()
                            && report
                                .preferred
                                .as_ref()
                                .is_some_and(|p| report.projects.contains(p))
                        {
                            report.selected = report.preferred.clone();
                        }
                    } else {
                        report.error = Some("Could not read projects from the node.".into());
                    }
                    report.pending_projects = report.pending_projects.saturating_sub(1);
                    if report.pending_projects == 0 && report.selected.is_none() {
                        report.selected = report.projects.first().cloned();
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn file(&mut self, cx: &mut Context<Self>) {
        if self.filing {
            return;
        }
        let Some(project) = self.selected.clone() else {
            self.error = Some("Choose a project.".into());
            cx.notify();
            return;
        };
        let text = self.field.read(cx).text.clone();
        let (title, body) = text.split_once('\n').unwrap_or((&text, ""));
        let title = title.trim();
        if title.is_empty() {
            self.error = Some("Enter a title.".into());
            cx.notify();
            return;
        }
        let body = body.trim();
        let description = if body.is_empty() {
            self.context.clone()
        } else {
            format!("{body}\n\n{}", self.context)
        };
        let task = self.host.request(
            Request::new("POST", project.node.clone(), "/api/work/items")
                .json(&json!({"project":project.name,"title":title,"description":description})),
            cx,
        );
        self.filing = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |report, cx| {
                report.filing = false;
                let reference = match result {
                    Ok(answer) if answer.ok() => serde_json::from_slice::<Value>(&answer.body)
                        .ok()
                        .and_then(|v| v["reference"].as_str().map(str::to_owned))
                        .ok_or_else(|| "The node returned no item reference.".to_string()),
                    Ok(answer) => Err(answer.error_text()),
                    Err(error) => Err(error.message),
                };
                match reference {
                    Ok(reference) => {
                        let id = reference
                            .rsplit('#')
                            .next()
                            .unwrap_or(&reference)
                            .to_owned();
                        let mut settings: Value =
                            report.host.setting(cx).unwrap_or_else(|_| json!({}));
                        if !settings.is_object() {
                            settings = json!({});
                        }
                        settings["report_project"] =
                            json!({"node": project.node.as_str(), "name": project.name});
                        let _ = report.host.set_setting(&settings, cx);
                        report
                            .field
                            .update(cx, |field, cx| field.set("", false, cx));
                        report.host.close_layer(LAYER, cx);
                        let host = report.host.clone();
                        report.host.report(
                            Status::new(format!("Filed {id}")).action("Show", move |_, cx| {
                                let _ = host.ask(
                                    Ask::Reveal {
                                        what: "item".into(),
                                        id: reference.clone(),
                                        node: Some(project.node.clone()),
                                        part: None,
                                        beside: false,
                                    },
                                    cx,
                                );
                            }),
                            cx,
                        );
                    }
                    Err(error) => report.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Report {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut projects = div().flex().flex_wrap().gap(px(SPACE_XS));
        for (i, project) in self.projects.iter().cloned().enumerate() {
            let label = if self.selected.as_ref() == Some(&project) {
                format!("✓ {}", project.name)
            } else {
                project.name.clone()
            };
            projects = projects.child(
                kit::button(&t, format!("report-project-{i}"), "", Kind::Quiet)
                    .max_w(px(CARD_W - 2. * SPACE_4XL))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .debug_selector(move || format!("report-project-{i}"))
                    .on_click(cx.listener(move |r, _, _, cx| {
                        r.selected = Some(project.clone());
                        cx.notify();
                    })),
            );
        }
        div()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(SPACE_4XL))
            .child(
                div()
                    .id("report-box")
                    .debug_selector(|| "report-box".into())
                    .w(px(CARD_W))
                    .p(px(SPACE_4XL))
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.divider)
                    .font_family(FAMILY)
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_XL))
                    .child(kit::label(&t, "Report"))
                    .child(
                        div()
                            .debug_selector(|| "report-field".into())
                            .child(self.field.clone()),
                    )
                    .child(projects)
                    .child(
                        div()
                            .debug_selector({
                                let context = self.context.clone();
                                move || format!("report-context-{context}")
                            })
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.context.clone()),
                    )
                    .when_some(self.error.clone(), |d, error| {
                        d.child(kit::note(&t, error, Some(t.error)))
                    })
                    .child(
                        kit::button(
                            &t,
                            "report-file",
                            if self.filing {
                                "Filing…"
                            } else {
                                "File report"
                            },
                            Kind::Primary,
                        )
                        .on_click(cx.listener(|report, _, _, cx| report.file(cx))),
                    ),
            )
    }
}

impl Work {
    pub(crate) fn open_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open(LAYER, cx) {
            return;
        }
        let report = self
            .report
            .get_or_insert_with(|| cx.new(|cx| Report::new(self.host.clone(), window, cx)))
            .clone();
        report.update(cx, |report, cx| report.open(cx));
        let focus = report.read(cx).field.read(cx).focus_handle(cx);
        let opener = window.focused(cx);
        self.host.layer(
            Layer::new(LAYER, report)
                .focus(focus.clone())
                .on_dismiss(move |window, cx| {
                    if let Some(opener) = &opener {
                        window.focus(opener, cx);
                    }
                }),
            cx,
        );
        window.focus(&focus, cx);
    }
}
