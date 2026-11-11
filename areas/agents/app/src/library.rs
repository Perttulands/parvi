use crate::{AgentsPage, role_state as role_library, tokens};
use gpui::{AnyElement, AppContext, Context, Entity, Window, div, prelude::*, px};
use parvi_agents_contract::roles::{Role, Roles, Usage};
use parvi_app_kit::link::http::ANSWER;
use parvi_app_kit::theme::{board, theme};
use parvi_app_kit::widgets::{
    field::TextField,
    kit::{self, Kind},
};
use serde_json::json;

#[derive(Default)]
pub(crate) struct Library {
    pub open: bool,
    node: String,
    roles: Vec<Role>,
    unreadable: Vec<role_library::Unreadable>,
    selected: Option<Role>,
    wanted: String,
    usage: Option<Usage>,
    form: Option<Form>,
    error: Option<String>,
    busy: bool,
    confirming_delete: bool,
    note: Option<Entity<TextField>>,
}
pub(crate) struct Form {
    creating: bool,
    id: Entity<TextField>,
    name: Entity<TextField>,
    kind: Entity<TextField>,
    summary: Entity<TextField>,
    capabilities: Entity<TextField>,
}
fn role_path(id: &str) -> String {
    format!("/api/roles/{}", id)
}
fn response<T: serde::de::DeserializeOwned>(
    answer: Result<(u16, Vec<u8>), parvi_core::Error>,
) -> Result<T, String> {
    match answer {
        Ok((status, body)) if (200..300).contains(&status) => {
            serde_json::from_slice(&body).map_err(|e| e.to_string())
        }
        Ok((status, body)) => Err(parvi_app_kit::host::Answer { status, body }.error_text()),
        Err(e) => Err(e.to_string()),
    }
}
struct Call {
    method: &'static str,
    path: String,
    if_match: Option<String>,
    body: Option<serde_json::Value>,
    wait: std::time::Duration,
}
impl AgentsPage {
    fn mission_call(
        &mut self,
        node: &str,
        method: &'static str,
        path: &str,
        body: Option<serde_json::Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<(u16, Vec<u8>), parvi_core::Error>, &mut Context<Self>)
        + 'static,
    ) {
        self.mission_request(
            node,
            Call {
                method,
                path: path.into(),
                if_match: None,
                body,
                wait: ANSWER,
            },
            cx,
            done,
        );
    }
    fn mission_request(
        &mut self,
        node: &str,
        call: Call,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<(u16, Vec<u8>), parvi_core::Error>, &mut Context<Self>)
        + 'static,
    ) {
        let mut req = parvi_app_kit::host::Request::new(
            call.method,
            parvi_core::NodeId(node.into()),
            call.path,
        )
        .within(call.wait);
        if let Some(tag) = call.if_match {
            req = req.header("if-match", tag);
        }
        if let Some(body) = call.body {
            req = req.json(&body);
        }
        let task = self.host.request(req, cx);
        cx.spawn(async move |page, cx| {
            let answer = task.await.map(|a| (a.status, a.body));
            let _ = page.update(cx, |page, cx| done(page, answer, cx));
        })
        .detach();
    }
    pub(crate) fn load_role_library(&mut self, cx: &mut Context<Self>) {
        if self.library.node.is_empty() {
            self.library.node = self
                .host
                .nodes(cx)
                .first()
                .map(|n| n.id.to_string())
                .unwrap_or_default();
        }
        let node = self.library.node.clone();
        if node.is_empty() {
            self.library.error = Some("No node is connected.".into());
            return;
        }
        self.library.busy = true;
        let requested_node = node.clone();
        self.mission_call(
            &node,
            "GET",
            "/api/roles",
            None,
            cx,
            move |s, answer, cx| {
                if s.library.node != requested_node {
                    return;
                }
                let l = &mut s.library;
                l.busy = false;
                match response::<Roles>(answer) {
                    Ok(r) => {
                        l.roles = r.roles;
                        l.unreadable = r.unreadable;
                        l.error = None;
                    }
                    Err(e) => l.error = Some(e),
                }
                cx.notify();
            },
        );
    }
    fn inspect_role(&mut self, id: String, cx: &mut Context<Self>) {
        if self.library.busy {
            return;
        }
        let node = self.library.node.clone();
        self.library.selected = None;
        self.library.form = None;
        self.library.wanted = id.clone();
        self.library.confirming_delete = false;
        self.library.note = Some(cx.new(|cx| TextField::new(cx, "Add a note").prose()));
        self.library.usage = None;
        let path = role_path(&id);
        let n = node.clone();
        let i = id.clone();
        self.mission_call(&node, "GET", &path, None, cx, move |s, answer, cx| {
            if s.library.node != n || s.library.wanted != i {
                return;
            }
            match response::<Role>(answer) {
                Ok(role) if role.id == i => {
                    s.library.selected = Some(role);
                    s.library.error = None;
                }
                Ok(_) => {}
                Err(e) => s.library.error = Some(e),
            }
            cx.notify();
        });
        let n = node.clone();
        let wanted = id;
        self.mission_call(
            &node,
            "GET",
            &format!("{path}/usage"),
            None,
            cx,
            move |s, answer, cx| {
                if s.library.node != n || s.library.wanted != wanted {
                    return;
                }
                match response::<Usage>(answer) {
                    Ok(usage) => s.library.usage = Some(usage),
                    Err(e) => s.library.error = Some(e),
                }
                cx.notify();
            },
        );
    }
    fn role_form(&mut self, creating: bool, cx: &mut Context<Self>) {
        if self.library.busy {
            return;
        }
        let role = self.library.selected.clone().unwrap_or_default();
        let field = |cx: &mut Context<Self>, hint: &str, value: &str| {
            cx.new(|cx| {
                let mut f = TextField::new(cx, hint);
                f.set(value, false, cx);
                f
            })
        };
        let (id, name, kind, summary, capabilities) = if creating {
            ("", "", "specialist", "", String::new())
        } else {
            (
                role.id.as_str(),
                role.display_name.as_str(),
                role.kind.as_str(),
                role.summary.as_str(),
                role.tags
                    .iter()
                    .filter(|t| !t.contains(':'))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        self.library.form = Some(Form {
            creating,
            id: field(cx, "role-id", id),
            name: field(cx, "Display name", name),
            kind: field(cx, "Kind", kind),
            summary: field(cx, "Summary", summary),
            capabilities: field(cx, "Capabilities, comma separated", &capabilities),
        });
        cx.notify();
    }
    fn save_role(&mut self, cx: &mut Context<Self>) {
        let l = &self.library;
        if l.busy {
            return;
        }
        let Some(form) = &l.form else {
            return;
        };
        let id = form.id.read(cx).text.trim().to_string();
        let mut body = role_library::role_change(
            &form.name.read(cx).text,
            &form.kind.read(cx).text,
            &form.summary.read(cx).text,
            &form.capabilities.read(cx).text,
        );
        let creating = form.creating;
        let etag = l.selected.as_ref().map(|r| r.etag.clone());
        let node = l.node.clone();
        let (method, path, etag) = if creating {
            body["id"] = json!(id);
            ("POST", "/api/roles".to_string(), None)
        } else {
            ("PATCH", role_path(&id), etag)
        };
        self.role_write(node, id, method, path, Some(body), etag, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn role_write(
        &mut self,
        node: String,
        id: String,
        method: &'static str,
        path: String,
        body: Option<serde_json::Value>,
        etag: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if method != "POST" && etag.as_deref().is_none_or(str::is_empty) {
            self.library.error =
                Some("The role version is missing; select it again before changing it.".into());
            cx.notify();
            return;
        }
        self.library.busy = true;
        let n = node.clone();
        self.mission_request(
            &node,
            Call {
                method,
                path,
                if_match: etag,
                body,
                wait: ANSWER,
            },
            cx,
            move |s, answer, cx| {
                if s.library.node != n {
                    return;
                }
                s.library.busy = false;
                match response::<serde_json::Value>(answer) {
                    Ok(_) => {
                        s.library.form = None;
                        s.library.error = None;
                        if method == "DELETE" {
                            s.library.selected = None;
                            s.library.usage = None;
                        } else {
                            s.inspect_role(id, cx);
                        }
                        s.load_role_library(cx);
                    }
                    Err(e) => s.library.error = Some(e),
                }
                cx.notify();
            },
        );
    }
    fn add_role_note(&mut self, cx: &mut Context<Self>) {
        let l = &self.library;
        if l.busy {
            return;
        }
        let (Some(role), Some(field)) = (&l.selected, &l.note) else {
            return;
        };
        let note = field.read(cx).text.trim().to_string();
        if note.is_empty() {
            return;
        }
        let (node, id, etag) = (l.node.clone(), role.id.clone(), role.etag.clone());
        self.role_write(
            node,
            id.clone(),
            "PATCH",
            role_path(&id),
            Some(json!({"note":note,"updatedBy":"human:ui"})),
            Some(etag),
            cx,
        );
    }
    fn role_status(&mut self, retired: bool, cx: &mut Context<Self>) {
        let l = &self.library;
        if l.busy || l.usage.is_none() {
            return;
        }
        let Some(role) = &l.selected else {
            return;
        };
        let (node, id, etag) = (l.node.clone(), role.id.clone(), role.etag.clone());
        self.role_write(
            node,
            id.clone(),
            "PATCH",
            role_path(&id),
            Some(json!({"retire":retired})),
            Some(etag),
            cx,
        );
    }
    pub(crate) fn render_role_library(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let l = &self.library;
        let mut rail = div()
            .id("roles-rail")
            .w(px(tokens::RAIL_W))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(tokens::P4))
            .p(px(tokens::P8))
            .bg(t.surface)
            .child(
                kit::button(&t, "library-back", "← Agents", Kind::Quiet).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.library.open = false;
                        cx.notify();
                    },
                )),
            )
            .child(kit::label(&t, "Roles by place"));
        for node in self
            .host
            .nodes(cx)
            .into_iter()
            .map(|n| n.id.to_string())
            .collect::<Vec<_>>()
        {
            let name = self.place_name(&node);
            let chosen = node == l.node;
            rail = rail.child(
                kit::button(
                    &t,
                    format!("role-node-{node}"),
                    name,
                    if chosen { Kind::Primary } else { Kind::Quiet },
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    let l = &mut s.library;
                    if l.busy {
                        return;
                    }
                    l.node = node.clone();
                    l.roles.clear();
                    l.unreadable.clear();
                    l.selected = None;
                    l.wanted.clear();
                    l.usage = None;
                    l.form = None;
                    s.load_role_library(cx);
                })),
            );
        }
        rail = rail.child(
            kit::button(&t, "role-new", "+ New role", Kind::Plain)
                .on_click(cx.listener(|s, _, _, cx| s.role_form(true, cx))),
        );
        for role in &l.roles {
            let id = role.id.clone();
            let label = format!(
                "{}{}",
                if role.display_name.is_empty() {
                    &role.id
                } else {
                    &role.display_name
                },
                if role.status == "retired" {
                    " · retired"
                } else {
                    ""
                }
            );
            rail = rail.child(
                kit::button(&t, format!("role-{id}"), label, Kind::Quiet)
                    .debug_selector({
                        let selector = format!("role-{id}");
                        move || selector.clone()
                    })
                    .on_click(cx.listener(move |s, _, _, cx| s.inspect_role(id.clone(), cx))),
            );
        }
        let mut detail = div()
            .id("roles-detail")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p(px(board::PAD))
            .flex()
            .flex_col()
            .gap(px(tokens::P8));
        if let Some(e) = &l.error {
            detail = detail.child(kit::note(&t, e.clone(), Some(t.error)));
        }
        if l.busy {
            detail = detail.child(kit::note(&t, "Working…", Some(t.dim)));
        }
        for item in &l.unreadable {
            detail = detail.child(kit::note(
                &t,
                format!("{}: {}", item.name, item.reason),
                Some(t.error),
            ));
        }
        if let Some(form) = &l.form {
            detail = detail.child(kit::label(
                &t,
                if form.creating {
                    "New role"
                } else {
                    "Edit role"
                },
            ));
            for (label, field) in [
                ("ID", &form.id),
                ("Name", &form.name),
                ("Kind", &form.kind),
                ("Summary", &form.summary),
                ("Capabilities", &form.capabilities),
            ] {
                if label != "ID" || form.creating {
                    let selector = format!("role-field-{label}");
                    detail = detail.child(kit::label(&t, label)).child(
                        div()
                            .id(selector.clone())
                            .debug_selector(move || selector.clone())
                            .child(field.clone()),
                    );
                }
            }
            detail = detail.child(
                div()
                    .flex()
                    .gap(px(tokens::P8))
                    .child(
                        kit::button(&t, "role-save", "Save", Kind::Primary)
                            .debug_selector(|| "role-save".into())
                            .on_click(cx.listener(|s, _, _, cx| s.save_role(cx))),
                    )
                    .child(
                        kit::button(&t, "role-cancel", "Cancel", Kind::Plain).on_click(
                            cx.listener(|s, _, _, cx| {
                                s.library.form = None;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        } else if let Some(role) = &l.selected {
            detail = detail
                .child(
                    div()
                        .text_size(px(tokens::TITLE))
                        .child(role.display_name.clone()),
                )
                .child(kit::note(
                    &t,
                    format!("{} · {} · {}", role.id, role.kind, role.status),
                    Some(t.dim),
                ))
                .child(role.summary.clone())
                .child(kit::note(
                    &t,
                    format!("Capabilities: {}", role.tags.join(", ")),
                    Some(t.dim),
                ))
                .child(kit::label(&t, "Mission usage"))
                .child(role_library::usage_words(l.usage.as_ref()));
            detail = detail.child(kit::label(&t, "Notes"));
            for note in &role.notes {
                detail = detail
                    .child(kit::note(
                        &t,
                        format!("{} · {}", note.actor, note.ts),
                        Some(t.dim),
                    ))
                    .child(note.text.clone());
            }
            if let Some(note) = &l.note {
                detail = detail.child(note.clone()).child(
                    kit::button(&t, "role-add-note", "Add note", Kind::Plain)
                        .on_click(cx.listener(|s, _, _, cx| s.add_role_note(cx))),
                );
            }
            detail = detail.child(
                kit::button(&t, "role-edit", "Edit", Kind::Plain)
                    .debug_selector(|| "role-edit".into())
                    .on_click(cx.listener(|s, _, _, cx| s.role_form(false, cx))),
            );
            if let Some(usage) = &l.usage {
                for (index, u) in usage.usage.iter().enumerate() {
                    let slug = u.mission_slug.clone();
                    let node = l.node.clone();
                    detail = detail.child(
                        kit::button(
                            &t,
                            format!("role-use-{index}"),
                            format!(
                                "{} / {} / {}",
                                u.mission_title, u.formation_title, u.slot_label
                            ),
                            Kind::Quiet,
                        )
                        .on_click(cx.listener(move |s, _, window, cx| {
                            let _ = window;
                            let _ = s.host.ask(
                                parvi_app_kit::Ask::Reveal {
                                    what: "mission".into(),
                                    id: slug.clone(),
                                    node: Some(parvi_core::NodeId(node.clone())),
                                    part: None,
                                },
                                cx,
                            );
                        })),
                    );
                }
                for u in &usage.unreadable {
                    detail = detail.child(kit::note(
                        &t,
                        format!("{}: {}", u.name, u.reason),
                        Some(t.error),
                    ));
                }
                detail = detail.child(kit::note(&t, "Retiring removes this role from staffing; existing slots need restaffing or restoration. Deletion is allowed only when no slot uses it.", Some(t.dim)));
                let retired = role.status == "retired";
                detail = detail.child(
                    div().flex().gap(px(tokens::P8)).child(
                        kit::button(
                            &t,
                            "role-retire",
                            if retired { "Restore" } else { "Retire" },
                            Kind::Plain,
                        )
                        .on_click(cx.listener(move |s, _, _, cx| s.role_status(!retired, cx))),
                    ),
                );
                if role_library::may_delete(Some(usage)) {
                    if l.confirming_delete {
                        let (node, id, etag) = (l.node.clone(), role.id.clone(), role.etag.clone());
                        detail = detail
                            .child(kit::note(
                                &t,
                                "Delete this role card? This cannot be undone.",
                                Some(t.error),
                            ))
                            .child(
                                kit::button(&t, "role-delete-confirm", "Delete role", Kind::Plain)
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.role_write(
                                            node.clone(),
                                            id.clone(),
                                            "DELETE",
                                            role_path(&id),
                                            None,
                                            Some(etag.clone()),
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                kit::button(&t, "role-delete-cancel", "Cancel", Kind::Quiet)
                                    .on_click(cx.listener(|s, _, _, cx| {
                                        s.library.confirming_delete = false;
                                        cx.notify();
                                    })),
                            );
                    } else {
                        detail = detail.child(
                            kit::button(&t, "role-delete", "Delete…", Kind::Plain).on_click(
                                cx.listener(|s, _, _, cx| {
                                    s.library.confirming_delete = true;
                                    cx.notify();
                                }),
                            ),
                        );
                    }
                }
            }
        } else {
            detail = detail.child(kit::note(&t, "Choose a role or create one.", Some(t.dim)));
        }
        div()
            .size_full()
            .flex()
            .bg(t.bg)
            .child(rail)
            .child(detail)
            .into_any_element()
    }
}
