use crate::{AgentsPage, role_state as role_library, tokens};
use gpui::{AnyElement, AppContext, Context, Entity, SharedString, Window, div, prelude::*, px};
use parvi_agents_contract::roles::{Role, Roles, Usage};
use parvi_app_kit::link::http::ANSWER;
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::theme::{size, theme};
use parvi_app_kit::widgets::{
    field::TextField,
    kit::{self, Kind},
    pane,
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
    /// Deletes the chosen role; the menu that offers it has asked twice.
    fn role_delete(&mut self, cx: &mut Context<Self>) {
        let l = &self.library;
        let Some(role) = &l.selected else {
            return;
        };
        let (node, id, etag) = (l.node.clone(), role.id.clone(), role.etag.clone());
        self.role_write(
            node,
            id.clone(),
            "DELETE",
            role_path(&id),
            None,
            Some(etag),
            cx,
        );
    }
    /// What the role's "…" offers: Retire or Restore, and Delete when no
    /// mission slot uses it.
    fn role_menu(&self, cx: &mut Context<Self>) -> Vec<MenuRow> {
        let l = &self.library;
        let Some(role) = &l.selected else {
            return vec![];
        };
        let page = cx.entity().downgrade();
        let retired = role.status == "retired";
        let mut retire = MenuRow::new(if retired { "Restore" } else { "Retire" }, {
            let page = page.clone();
            move |_, cx| {
                let _ = page.update(cx, |s, cx| s.role_status(!retired, cx));
            }
        });
        if !retired {
            retire = retire.hint("Out of staffing; its slots need restaffing");
        }
        let mut delete = MenuRow::new("Delete", move |_, cx| {
            let _ = page.update(cx, |s, cx| s.role_delete(cx));
        })
        .danger();
        if !role_library::may_delete(l.usage.as_ref()) {
            delete = delete.refused(role_library::usage_words(l.usage.as_ref()));
        }
        vec![retire, delete]
    }
    pub(crate) fn render_role_library(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let l = &self.library;
        let list_head = pane::head(&t)
            .child(
                pane::act(&t, "library-back", "← Agents").on_click(cx.listener(|s, _, _, cx| {
                    s.library.open = false;
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                pane::act(&t, "role-new", "New")
                    .on_click(cx.listener(|s, _, _, cx| s.role_form(true, cx))),
            );
        let mut rows = div().flex().flex_col();
        let nodes: Vec<String> = self
            .host
            .nodes(cx)
            .into_iter()
            .map(|n| n.id.to_string())
            .collect();
        // Each device keeps its own roles; choose which when there are several.
        if nodes.len() > 1 {
            rows = rows.child(pane::label(&t, "Device"));
            for node in nodes {
                let name = self.place_name(&node);
                let chosen = node == l.node;
                rows = rows.child(
                    pane::row(&t, SharedString::from(format!("role-node-{node}")), chosen)
                        .child(pane::cell(name))
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
            rows = rows.child(pane::label(&t, "Roles"));
        }
        for role in &l.roles {
            let id = role.id.clone();
            let name = if role.display_name.is_empty() {
                role.id.clone()
            } else {
                role.display_name.clone()
            };
            let selector = format!("role-{id}");
            rows = rows.child(
                pane::row(&t, SharedString::from(selector.clone()), l.wanted == id)
                    .debug_selector(move || selector)
                    .child(pane::cell(name))
                    .when(role.status == "retired", |d| {
                        d.child(pane::meta(&t, "retired", None))
                    })
                    .on_click(cx.listener(move |s, _, _, cx| s.inspect_role(id.clone(), cx))),
            );
        }
        let list = pane::list(&t, "roles-list", list_head, rows);

        let quiet = l.error.is_none() && l.unreadable.is_empty();
        let mut notes = div().flex().flex_col().gap(px(tokens::P4));
        if let Some(e) = &l.error {
            notes = notes.child(div().text_color(t.error).child(e.clone()));
        }
        for item in &l.unreadable {
            notes = notes.child(
                div()
                    .text_color(t.error)
                    .child(format!("{}: {}", item.name, item.reason)),
            );
        }
        let (head, body) = if let Some(form) = &l.form {
            let head = pane::head(&t)
                .child(pane::title(
                    &t,
                    if form.creating {
                        "New role"
                    } else {
                        "Edit role"
                    },
                ))
                .child(pane::act(&t, "role-cancel", "Cancel").on_click(cx.listener(
                    |s, _, _, cx| {
                        s.library.form = None;
                        cx.notify();
                    },
                )))
                .child(
                    kit::button(&t, "role-save", "Save", Kind::Primary)
                        .debug_selector(|| "role-save".into())
                        .when(l.busy, |b| b.opacity(tokens::DIMMED))
                        .on_click(cx.listener(|s, _, _, cx| s.save_role(cx))),
                );
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(size::SECTION_GAP))
                .when(!quiet, |d| d.child(notes));
            for (label, field) in [
                ("ID", &form.id),
                ("Name", &form.name),
                ("Kind", &form.kind),
                ("Summary", &form.summary),
                ("Capabilities", &form.capabilities),
            ] {
                if label != "ID" || form.creating {
                    let selector = format!("role-field-{label}");
                    body = body.child(
                        pane::section(&t, label).child(
                            div()
                                .id(selector.clone())
                                .debug_selector(move || selector.clone())
                                .child(field.clone()),
                        ),
                    );
                }
            }
            (head, body)
        } else if let Some(role) = &l.selected {
            let head = pane::head(&t)
                .child(pane::title(&t, role.display_name.clone()))
                .child(
                    pane::act(&t, "role-edit", "Edit")
                        .debug_selector(|| "role-edit".into())
                        .on_click(cx.listener(|s, _, _, cx| s.role_form(false, cx))),
                )
                .child(pane::act(&t, "role-more", "…").on_click(cx.listener(
                    |s, e: &gpui::ClickEvent, _, cx| {
                        let rows = s.role_menu(cx);
                        s.host.menu(rows, e.position(), cx);
                    },
                )));
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(size::SECTION_GAP))
                .when(!quiet, |d| d.child(notes))
                .child(div().whitespace_normal().child(role.summary.clone()));
            let mut lines = div().flex().flex_col().child(pane::line(
                &t,
                "Kind",
                div().child(role.kind.clone()),
            ));
            if role.status != "active" {
                lines = lines.child(pane::line(&t, "Status", div().child(role.status.clone())));
            }
            if !role.tags.is_empty() {
                lines = lines.child(pane::line(
                    &t,
                    "Capabilities",
                    div().child(role.tags.join(", ")),
                ));
            }
            lines = lines.child(pane::line(
                &t,
                "Used by",
                div().child(role_library::usage_words(l.usage.as_ref())),
            ));
            body = body.child(lines);
            if let Some(usage) = &l.usage
                && (!usage.usage.is_empty() || !usage.unreadable.is_empty())
            {
                let mut uses = pane::section(&t, "Mission slots");
                for (index, u) in usage.usage.iter().enumerate() {
                    let slug = u.mission_slug.clone();
                    let node = l.node.clone();
                    uses = uses.child(
                        pane::row(&t, ("role-use", index), false)
                            .mx_0()
                            .child(pane::cell(format!(
                                "{} / {} / {}",
                                u.mission_title, u.formation_title, u.slot_label
                            )))
                            .tooltip(kit::tip("Show it in Missions"))
                            .on_click(cx.listener(move |s, _, _, cx| {
                                let _ = s.host.ask(
                                    parvi_app_kit::Ask::Reveal {
                                        what: "mission".into(),
                                        id: slug.clone(),
                                        node: Some(parvi_core::NodeId(node.clone())),
                                        part: None,
                                        beside: false,
                                    },
                                    cx,
                                );
                            })),
                    );
                }
                for u in &usage.unreadable {
                    uses = uses.child(
                        div()
                            .text_color(t.error)
                            .child(format!("{}: {}", u.name, u.reason)),
                    );
                }
                body = body.child(uses);
            }
            let mut said = pane::section(&t, "Notes");
            for note in &role.notes {
                said = said.child(
                    div()
                        .pb(px(tokens::P8))
                        .child(
                            div()
                                .text_size(px(size::SMALL))
                                .text_color(t.dim)
                                .child(format!("{} · {}", note.actor, note.ts)),
                        )
                        .child(div().whitespace_normal().child(note.text.clone())),
                );
            }
            if let Some(note) = &l.note {
                said = said.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(size::HEAD_GAP))
                        .child(div().flex_1().min_w_0().child(note.clone()))
                        .child(
                            kit::button(&t, "role-add-note", "Add", Kind::Plain)
                                .on_click(cx.listener(|s, _, _, cx| s.add_role_note(cx))),
                        ),
                );
            }
            (head, body.child(said))
        } else {
            let words = if l.busy {
                "Reading the roles…"
            } else if l.roles.is_empty() {
                "No roles yet."
            } else {
                "Choose a role."
            };
            (
                pane::head(&t),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(size::SECTION_GAP))
                    .when(!quiet, |d| d.child(notes))
                    .child(pane::empty(&t, words, None).p_0()),
            )
        };
        pane::page(&t, list, pane::detail("roles-detail", head, body))
            .text_color(t.text)
            .into_any_element()
    }
}
