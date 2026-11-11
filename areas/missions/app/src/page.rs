use crate::store::{Store, canvas as board, flow, undo};
use gpui::{
    AppContext, Bounds, Context, Div, Entity, IntoElement, PathBuilder, Pixels, Render, Window,
    canvas, div, point, prelude::*, px,
};
use parvi_app_kit::{
    Host, Page,
    host::Request,
    theme::{board::LIST_W, size, theme},
    widgets::{
        boundary,
        field::TextField,
        kit::{self, Kind},
    },
};
use parvi_core::NodeId;
use parvi_missions_contract::{
    EditResponse, ErrorResponse, LayoutResponse, MissionResponse, WATCH, WATCH_RUNS,
};
use serde_json::json;

const PAD: f32 = 12.;
const EDGE: f32 = 2.;
const EDITOR_W: f32 = 360.;
const LINK_FIELD_W: f32 = 180.;
pub struct MissionsPage {
    host: Host,
    state: Store,
    generation: u64,
    editor: Option<Editor>,
    title: Entity<TextField>,
    goal: Entity<TextField>,
    extra: Entity<TextField>,
    repository: Entity<TextField>,
    work: Entity<TextField>,
    files: Entity<TextField>,
    links: Entity<TextField>,
    check: Entity<TextField>,
    check_version: Entity<TextField>,
    check_value: Entity<TextField>,
    source: Entity<TextField>,
    target: Entity<TextField>,
    history: undo::History,
    pending_history: Option<(WriteMode, undo::Entry)>,
    writing: bool,
    deleting_mission: bool,
    roles: Option<parvi_agents_contract::roles::Roles>,
    role: Entity<TextField>,
    harness: Entity<TextField>,
    model: Entity<TextField>,
    effort: Entity<TextField>,
    slot: Option<String>,
    note: Entity<TextField>,
    notes: Option<parvi_missions_contract::document::NotesDocument>,
    port_label: Entity<TextField>,
    port_kind: Entity<TextField>,
    port: Option<String>,
    limit: Option<String>,
    limit_title: Entity<TextField>,
    limit_rounds: Entity<TextField>,
    limit_seconds: Entity<TextField>,
    limit_warn: Entity<TextField>,
    limit_tokens: Entity<TextField>,
    limit_spent: Entity<TextField>,
    note_id: Option<String>,
}
#[derive(Clone, Copy)]
enum EditKind {
    Step,
    Input,
    Gate,
    End,
}
#[derive(Clone, Copy)]
enum WriteMode {
    Edit,
    Undo,
    Redo,
}
struct OutputDraft {
    name: Entity<TextField>,
    description: Entity<TextField>,
    kind: Entity<TextField>,
    required: bool,
    from: Entity<TextField>,
}
struct Editor {
    kind: EditKind,
    id: String,
    rev: i64,
    etag: String,
    busy: bool,
    error: Option<String>,
    brief: parvi_missions_contract::Brief,
    deleting: bool,
    outputs: Vec<OutputDraft>,
}
impl MissionsPage {
    pub fn new(host: Host, _: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            host,
            state: Store::default(),
            generation: 0,
            editor: None,
            title: cx.new(|cx| TextField::new(cx, "Step title")),
            goal: cx.new(|cx| TextField::new(cx, "Description").prose()),
            extra: cx.new(|cx| TextField::new(cx, "Device or outcome")),
            repository: cx.new(|cx| TextField::new(cx, "Repository")),
            work: cx.new(|cx| TextField::new(cx, "Work reference")),
            files: cx.new(|cx| TextField::new(cx, "One file per line").prose()),
            links: cx.new(|cx| TextField::new(cx, "One link per line").prose()),
            check: cx.new(|cx| TextField::new(cx, "Check profile")),
            check_version: cx.new(|cx| TextField::new(cx, "Profile version")),
            check_value: cx.new(|cx| TextField::new(cx, "Check value")),
            source: cx.new(|cx| TextField::new(cx, "From node:port")),
            target: cx.new(|cx| TextField::new(cx, "To node:port")),
            history: undo::History::default(),
            pending_history: None,
            writing: false,
            deleting_mission: false,
            roles: None,
            role: cx.new(|cx| TextField::new(cx, "Role id")),
            harness: cx.new(|cx| TextField::new(cx, "Harness")),
            model: cx.new(|cx| TextField::new(cx, "Model")),
            effort: cx.new(|cx| TextField::new(cx, "Effort")),
            slot: None,
            note: cx.new(|cx| TextField::new(cx, "Add a note").prose()),
            notes: None,
            port_label: cx.new(|cx| TextField::new(cx, "Port label")),
            port_kind: cx.new(|cx| TextField::new(cx, "text, file, or folder")),
            port: None,
            limit: None,
            limit_title: cx.new(|cx| TextField::new(cx, "Limit name")),
            limit_rounds: cx.new(|cx| TextField::new(cx, "Rounds")),
            limit_seconds: cx.new(|cx| TextField::new(cx, "Seconds")),
            limit_warn: cx.new(|cx| TextField::new(cx, "Warn after seconds")),
            limit_tokens: cx.new(|cx| TextField::new(cx, "Tokens")),
            limit_spent: cx.new(|cx| TextField::new(cx, "stop, best, or fail")),
            note_id: None,
        }
    }
    fn open(&mut self, node: String, slug: String, cx: &mut Context<Self>) {
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.state.selected = Some((node.clone(), slug.clone()));
        self.state.mission = None;
        self.state.mission_etag = None;
        self.state.layout_etag = None;
        self.editor = None;
        self.history = undo::History::default();
        self.deleting_mission = false;
        self.state.layout = None;
        self.notes = None;
        self.note_id = None;
        self.note.update(cx, |field, cx| field.set("", false, cx));
        self.state.error = None;
        // Encode a path segment, including non-ASCII bytes.
        let encoded: String = slug
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let note_request = self.host.request(
            Request::get(
                NodeId(node.clone()),
                format!("{}/notes", mission_path(&slug)),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = note_request.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation == generation {
                    page.notes = result
                        .ok()
                        .filter(|a| a.ok())
                        .and_then(|a| {
                            serde_json::from_slice::<parvi_missions_contract::NotesResponse>(
                                &a.body,
                            )
                            .ok()
                        })
                        .map(|n| n.notes);
                    cx.notify();
                }
            });
        })
        .detach();
        for layout in [false, true] {
            let path = format!(
                "/api/missions/{encoded}{}",
                if layout { "/layout" } else { "" }
            );
            let asked = self
                .host
                .request(Request::get(NodeId(node.clone()), path), cx);
            let key = (node.clone(), slug.clone());
            cx.spawn(async move |this, cx| {
                let answer = asked.await;
                let _ = this.update(cx, |page, cx| {
                    if page.generation != generation || page.state.selected.as_ref() != Some(&key) {
                        return;
                    }
                    match answer {
                        Ok(answer) if answer.ok() => {
                            if layout {
                                match serde_json::from_slice::<LayoutResponse>(&answer.body) {
                                    Ok(v) => {
                                        page.state.layout_etag = Some(v.layout.etag);
                                        page.state.layout = Some(v.layout.layout);
                                    }
                                    Err(e) => page.state.error = Some(e.to_string()),
                                }
                            } else {
                                match serde_json::from_slice::<MissionResponse>(&answer.body) {
                                    Ok(v) => {
                                        page.state.mission_etag = v.mission.etag;
                                        page.state.mission = Some(v.mission.mission);
                                    }
                                    Err(e) => page.state.error = Some(e.to_string()),
                                }
                            }
                        }
                        Ok(answer) => page.state.error = Some(answer.error_text()),
                        Err(e) => page.state.error = Some(e.message),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn create_mission(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.host.nodes(cx).first().map(|n| n.id.clone()) else {
            return;
        };
        let request = Request::new("POST", node.clone(), "/api/missions")
            .json(&json!({"updatedBy":"user:parvi-app"}));
        let asked = self.host.request(request, cx);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<MissionResponse>(&a.body) {
                        Ok(v) => page.open(node.to_string(), v.mission.mission.slug, cx),
                        Err(e) => page.state.error = Some(e.to_string()),
                    },
                    Ok(a) => {
                        page.state.error = Some(
                            serde_json::from_slice::<ErrorResponse>(&a.body)
                                .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                .unwrap_or_else(|_| a.error_text()),
                        )
                    }
                    Err(e) => page.state.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn delete_mission(&mut self, cx: &mut Context<Self>) {
        if self.writing || self.editor.is_some() {
            return;
        }
        if !self.deleting_mission {
            self.deleting_mission = true;
            cx.notify();
            return;
        }
        let (Some((node, slug)), Some(m), Some(etag)) = (
            self.state.selected.clone(),
            self.state.mission.as_ref(),
            self.state.mission_etag.clone(),
        ) else {
            return;
        };
        let request = Request::new("DELETE", NodeId(node), mission_path(&slug))
            .header("If-Match", etag)
            .json(&json!({"expectedRev":m.rev}));
        self.writing = true;
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }
                page.writing = false;
                page.deleting_mission = false;
                match result {
                    Ok(a) if a.ok() => {
                        page.state.selected = None;
                        page.state.mission = None;
                        page.state.layout = None;
                        page.notes = None;
                        page.history = undo::History::default();
                    }
                    Ok(a) => {
                        page.state.error = Some(
                            serde_json::from_slice::<ErrorResponse>(&a.body)
                                .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                .unwrap_or_else(|_| a.error_text()),
                        )
                    }
                    Err(e) => page.state.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn edit_step(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        let Some(m) = &self.state.mission else {
            return;
        };
        let Some(etag) = self.state.mission_etag.clone() else {
            return;
        };
        let (kind, title, goal, extra, brief) =
            if let Some(f) = m.formations.iter().find(|f| f.id == id) {
                let brief = f.brief.clone().unwrap_or_default();
                (
                    EditKind::Step,
                    f.title.clone(),
                    brief.goal.clone(),
                    f.device.clone(),
                    brief,
                )
            } else if let Some(c) = m.input_cards.iter().find(|c| c.id == id) {
                (
                    EditKind::Input,
                    c.title.clone(),
                    c.goal.clone(),
                    c.human_channel.clone(),
                    Default::default(),
                )
            } else if let Some(g) = m.gates.iter().find(|g| g.id == id) {
                (
                    EditKind::Gate,
                    g.title.clone(),
                    g.criterion.clone(),
                    g.kinds.join(","),
                    Default::default(),
                )
            } else if let Some(e) = m.ends.iter().find(|e| e.id == id) {
                (
                    EditKind::End,
                    e.title.clone(),
                    String::new(),
                    e.outcome.clone(),
                    Default::default(),
                )
            } else {
                return;
            };
        self.title
            .update(cx, |field, cx| field.set(&title, false, cx));
        self.goal
            .update(cx, |field, cx| field.set(&goal, false, cx));
        self.extra
            .update(cx, |field, cx| field.set(&extra, false, cx));
        for (field, value) in [
            (&self.repository, brief.repository.clone()),
            (&self.work, brief.work.clone()),
            (&self.files, brief.files.join("\n")),
            (&self.links, brief.links.join("\n")),
        ] {
            field.update(cx, |f, cx| f.set(&value, false, cx));
        }
        if let Some(gate) = m.gates.iter().find(|g| g.id == id) {
            for (field, value) in [
                (&self.check, gate.check.clone()),
                (&self.check_version, gate.check_version.clone()),
                (&self.check_value, gate.check_value.clone()),
                (&self.files, gate.files.join("\n")),
            ] {
                field.update(cx, |f, cx| f.set(&value, false, cx));
            }
        }
        let outputs = m
            .input_cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| {
                c.outputs
                    .iter()
                    .map(|o| {
                        let name = cx.new(|cx| {
                            let mut f = TextField::new(cx, "Output name");
                            f.set(&o.name, false, cx);
                            f
                        });
                        let description = cx.new(|cx| {
                            let mut f = TextField::new(cx, "Description");
                            f.set(&o.description, false, cx);
                            f
                        });
                        let kind = cx.new(|cx| {
                            let mut f = TextField::new(cx, "text, file, or folder");
                            f.set(&o.kind, false, cx);
                            f
                        });
                        let from = cx.new(|cx| {
                            let mut f = TextField::new(cx, "Source step:port");
                            f.set(&o.from, false, cx);
                            f
                        });
                        OutputDraft {
                            name,
                            description,
                            kind,
                            required: o.required,
                            from,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.editor = Some(Editor {
            kind,
            id: id.to_owned(),
            rev: m.rev,
            etag,
            brief,
            busy: false,
            error: None,
            deleting: false,
            outputs,
        });
        self.slot = m
            .formations
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.slots.first())
            .map(|s| s.id.clone());
        if let Some(slot) = m
            .formations
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.slots.first())
        {
            for (field, text) in [
                (&self.role, &slot.agent_id),
                (&self.harness, &slot.harness),
                (&self.model, &slot.model),
                (&self.effort, &slot.effort),
            ] {
                field.update(cx, |field, cx| field.set(text, false, cx));
            }
        }
        if matches!(kind, EditKind::Step)
            && let Some((node, _)) = self.state.selected.clone()
        {
            let asked = self.host.request(
                Request::get(NodeId(node), parvi_agents_contract::roles::PATH),
                cx,
            );
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                let answer = asked.await;
                let _ = this.update(cx, |page, cx| {
                    if page.generation == generation {
                        page.roles = answer
                            .ok()
                            .filter(|a| a.ok())
                            .and_then(|a| serde_json::from_slice(&a.body).ok());
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn write_board(&mut self, patch: serde_json::Value, mode: WriteMode, cx: &mut Context<Self>) {
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        let Some((node, slug)) = self.state.selected.clone() else {
            return;
        };
        let Some(mission) = self.state.mission.clone() else {
            return;
        };
        let Some(etag) = self.state.mission_etag.clone() else {
            return;
        };
        let before =
            match serde_json::to_value(parvi_missions_contract::document::MissionDocument {
                mission: mission.clone(),
                etag: None,
            }) {
                Ok(v) => v,
                Err(e) => {
                    self.state.error = Some(e.to_string());
                    return;
                }
            };
        let layout = self
            .state
            .layout
            .as_ref()
            .map(|l| l.nodes.as_slice())
            .unwrap_or(&[]);
        let reverse = match undo::inverse(&before, layout, &undo::Step::Board(patch.clone())) {
            Ok(v) => v,
            Err(_) if matches!(mode, WriteMode::Edit) => Vec::new(),
            Err(e) => {
                self.state.error = Some(e);
                self.restore_history();
                cx.notify();
                return;
            }
        };
        let deleted = patch
            .as_object()
            .is_some_and(|p| p.keys().any(|k| k.starts_with("delete")));
        let mut body = patch.clone();
        body["expectedRev"] = json!(mission.rev);
        body["updatedBy"] = json!("user:parvi-app");
        let request = Request::new("PATCH", NodeId(node), mission_path(&slug))
            .header("If-Match", etag)
            .json(&body);
        self.writing = true;
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                page.writing = false;
                let ok = result.as_ref().is_ok_and(|a| {
                    a.ok() && serde_json::from_slice::<EditResponse>(&a.body).is_ok()
                });
                if !ok {
                    page.restore_history();
                }
                match result {
                    Ok(answer) if answer.ok() => {
                        match serde_json::from_slice::<EditResponse>(&answer.body) {
                            Ok(v) => {
                                let after = serde_json::to_value(&v.mission).unwrap_or_default();
                                let steps = if reverse.is_empty() {
                                    undo::answered(&patch, &before, &after)
                                } else {
                                    reverse
                                };
                                if steps.is_empty() {
                                    page.history = undo::History::default();
                                }
                                let entry = undo::Entry {
                                    label: "edit".into(),
                                    steps,
                                };
                                page.history.wrote(undo::Version {
                                    rev: v.mission.mission.rev,
                                    layout: Some(v.layout.etag.clone()),
                                });
                                page.pending_history = None;
                                match mode {
                                    WriteMode::Edit => page.history.record(entry),
                                    WriteMode::Undo => page.history.undone(entry),
                                    WriteMode::Redo => page.history.redone(entry),
                                }
                                if deleted {
                                    page.editor = None;
                                } else if let Some(ed) = &mut page.editor {
                                    ed.rev = v.mission.mission.rev;
                                    if let Some(etag) = &v.mission.etag {
                                        ed.etag = etag.clone();
                                    }
                                }
                                page.state.mission_etag = v.mission.etag;
                                page.state.mission = Some(v.mission.mission);
                                page.state.layout_etag = Some(v.layout.etag);
                                page.state.layout = Some(v.layout.layout);
                                page.state.error = None;
                            }
                            Err(e) => page.state.error = Some(e.to_string()),
                        }
                    }
                    Ok(a) => {
                        page.state.error = Some(
                            serde_json::from_slice::<ErrorResponse>(&a.body)
                                .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                .unwrap_or_else(|_| a.error_text()),
                        )
                    }
                    Err(e) => page.state.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn restore_history(&mut self) {
        if let Some((mode, entry)) = self.pending_history.take() {
            match mode {
                WriteMode::Undo => self.history.redone(entry),
                WriteMode::Redo => self.history.undone(entry),
                WriteMode::Edit => {}
            }
        }
    }
    fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        let Some(m) = &self.state.mission else {
            return;
        };
        let version = undo::Version {
            rev: m.rev,
            layout: self.state.layout_etag.clone(),
        };
        match self.history.take(redo, &version) {
            Ok(Some(entry)) => {
                let mode = if redo {
                    WriteMode::Redo
                } else {
                    WriteMode::Undo
                };
                let step = (entry.steps.len() == 1).then(|| entry.steps[0].clone());
                self.pending_history = Some((mode, entry));
                if let Some(undo::Step::Board(patch)) = step {
                    self.write_board(patch, mode, cx);
                } else {
                    self.restore_history();
                    self.state.error = Some(
                        "This edit has several steps or changes layout and cannot be undone here."
                            .into(),
                    );
                    cx.notify();
                }
            }
            Ok(None) => {}
            Err(_) => {
                self.state.error =
                    Some("The mission changed elsewhere; undo history was cleared.".into());
                cx.notify();
            }
        }
    }
    fn add_note(&mut self, delete: bool, cx: &mut Context<Self>) {
        let (Some((node, slug)), Some(notes)) = (self.state.selected.clone(), self.notes.as_ref())
        else {
            return;
        };
        let text = self.note.read(cx).text.clone();
        if text.trim().is_empty() && !delete {
            return;
        }
        let target = self
            .editor
            .as_ref()
            .map(|e| e.id.as_str())
            .unwrap_or("mission");
        let request = Request::new(
            "PATCH",
            NodeId(node),
            format!("{}/notes", mission_path(&slug)),
        )
        .header("If-Match", notes.etag.clone())
        .json(&json!({"target":target, "text":text,"author":"human:ui", "action":if delete {"delete"} else if self.note_id.is_some() {"edit"} else {"append"}, "entryId":self.note_id}));
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<
                        parvi_missions_contract::NotesResponse,
                    >(&a.body)
                    {
                        Ok(n) => {
                            page.notes = Some(n.notes);
                            page.note_id = None;
                            page.note.update(cx, |f, cx| f.set("", false, cx));
                        }
                        Err(e) => page.state.error = Some(e.to_string()),
                    },
                    Ok(a) => {
                        page.state.error = Some(
                            serde_json::from_slice::<ErrorResponse>(&a.body)
                                .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                .unwrap_or_else(|_| a.error_text()),
                        )
                    }
                    Err(e) => page.state.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn select_slot(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else {
            return;
        };
        let Some(slot) = self
            .state
            .mission
            .as_ref()
            .and_then(|m| m.formations.iter().find(|f| f.id == ed.id))
            .and_then(|f| f.slots.iter().find(|s| s.id == id))
        else {
            return;
        };
        for (field, text) in [
            (&self.role, &slot.agent_id),
            (&self.harness, &slot.harness),
            (&self.model, &slot.model),
            (&self.effort, &slot.effort),
        ] {
            field.update(cx, |field, cx| field.set(text, false, cx));
        }
        self.slot = Some(id.to_owned());
        cx.notify();
    }
    fn port_edit(&mut self, op: &str, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else {
            return;
        };
        let id = ed.id.clone();
        let label = self.port_label.read(cx).text.clone();
        let kind = self.port_kind.read(cx).text.clone();
        let patch = match op {
            "input" | "output" => {
                json!({"addPort":{"formationId":id,"direction":op,"label":label,"kind":kind}})
            }
            "update" => {
                json!({"updatePort":{"formationId":id,"portId":self.port,"label":label,"kind":kind}})
            }
            "remove" => json!({"removePort":{"formationId":id,"portId":self.port}}),
            _ => return,
        };
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn select_limit(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(limit) = self
            .state
            .mission
            .as_ref()
            .and_then(|m| m.limits.iter().find(|l| l.id == id))
        else {
            return;
        };
        let value = |n: Option<i64>| n.map(|n| n.to_string()).unwrap_or_default();
        for (field, text) in [
            (&self.limit_title, limit.title.clone()),
            (&self.limit_rounds, value(limit.rounds)),
            (&self.limit_seconds, value(limit.seconds)),
            (&self.limit_warn, value(limit.warn_seconds)),
            (&self.limit_tokens, value(limit.tokens)),
            (&self.limit_spent, limit.when_spent.clone()),
        ] {
            field.update(cx, |f, cx| f.set(&text, false, cx));
        }
        self.limit = Some(id.to_owned());
        cx.notify();
    }
    fn limit_action(&mut self, action: &str, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else {
            return;
        };
        if action == "create" {
            self.write_board(json!({"createLimit":{"title":"Limit","target":ed.id,"rounds":3,"whenSpent":"stop","x":0,"y":0}}), WriteMode::Edit, cx);
            return;
        }
        let Some(id) = self.limit.clone() else {
            return;
        };
        if action == "delete" {
            self.write_board(json!({"deleteLimit":{"id":id}}), WriteMode::Edit, cx);
            self.limit = None;
            return;
        }
        let mut value = json!({"id":id, "title":self.limit_title.read(cx).text, "whenSpent":self.limit_spent.read(cx).text});
        for (key, field) in [
            ("rounds", &self.limit_rounds),
            ("seconds", &self.limit_seconds),
            ("warnSeconds", &self.limit_warn),
            ("tokens", &self.limit_tokens),
        ] {
            let text = &field.read(cx).text;
            if text.trim().is_empty() {
                value[key] = json!(0);
            } else if let Ok(n) = text.trim().parse::<i64>() {
                value[key] = json!(n);
            } else {
                self.state.error = Some(format!("{key} must be a whole number"));
                cx.notify();
                return;
            }
        }
        self.write_board(json!({"updateLimit":value}), WriteMode::Edit, cx);
    }
    fn add_output(&mut self, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextField::new(cx, "Output name"));
        let description = cx.new(|cx| TextField::new(cx, "Description"));
        let kind = cx.new(|cx| {
            let mut f = TextField::new(cx, "text, file, or folder");
            f.set("text", false, cx);
            f
        });
        let from = cx.new(|cx| TextField::new(cx, "Source step:port"));
        if let Some(ed) = &mut self.editor {
            ed.outputs.push(OutputDraft {
                name,
                description,
                kind,
                required: false,
                from,
            });
        }
        cx.notify();
    }
    fn choose_harness(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(catalog) = &self.roles else {
            return;
        };
        let mut staff = [
            self.role.read(cx).text.clone(),
            self.harness.read(cx).text.clone(),
            self.model.read(cx).text.clone(),
            self.effort.read(cx).text.clone(),
        ];
        crate::store::staffing::pick_harness(catalog, &mut staff, id);
        for (field, value) in [
            (&self.harness, &staff[1]),
            (&self.model, &staff[2]),
            (&self.effort, &staff[3]),
        ] {
            field.update(cx, |f, cx| f.set(value, false, cx));
        }
    }
    fn set_type(&mut self, kind: &str, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else {
            return;
        };
        let patch = json!({"setFormationType": {"id":ed.id, "type":kind, "keepSlotId":self.slot.clone().unwrap_or_default()}});
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn set_device(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else {
            return;
        };
        let patch = json!({"updateFormation":{"id":ed.id,"device":self.extra.read(cx).text}});
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn assign_slot(&mut self, cx: &mut Context<Self>) {
        let (Some(ed), Some(slot)) = (&self.editor, &self.slot) else {
            return;
        };
        let patch = json!({"assignSlot": {"formationId": ed.id, "slotId": slot,
            "agentId":self.role.read(cx).text, "harness":self.harness.read(cx).text,
            "model":self.model.read(cx).text, "effort":self.effort.read(cx).text}});
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn arrange(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let (Some((node, slug)), Some(etag)) =
            (self.state.selected.clone(), self.state.layout_etag.clone())
        else {
            return;
        };
        let path = format!("{}/layout", mission_path(&slug));
        let request = Request::new("PATCH", NodeId(node), path)
            .header("If-Match", etag)
            .json(&json!({"arrange":true}));
        self.writing = true;
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }
                page.writing = false;
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<LayoutResponse>(&a.body) {
                        Ok(v) => {
                            page.state.layout_etag = Some(v.layout.etag);
                            page.state.layout = Some(v.layout.layout);
                        }
                        Err(e) => page.state.error = Some(e.to_string()),
                    },
                    Ok(a) => {
                        page.state.error = Some(
                            serde_json::from_slice::<ErrorResponse>(&a.body)
                                .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                .unwrap_or_else(|_| a.error_text()),
                        )
                    }
                    Err(e) => page.state.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn link(&mut self, cx: &mut Context<Self>) {
        let from = self.source.read(cx).text.clone();
        let to = self.target.read(cx).text.clone();
        self.write_board(
            json!({"wireConnection":{"from":from, "to":to}}),
            WriteMode::Edit,
            cx,
        );
    }
    fn delete_card(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &mut self.editor else {
            return;
        };
        if !ed.deleting {
            ed.deleting = true;
            cx.notify();
            return;
        }
        let key = match ed.kind {
            EditKind::Step => "deleteFormation",
            EditKind::Input => "deleteInputCard",
            EditKind::Gate => "deleteGate",
            EditKind::End => "deleteEnd",
        };
        let patch = json!({key: {"id": ed.id}});
        self.write_board(patch, WriteMode::Edit, cx);
    }
    fn save_step(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &mut self.editor else {
            return;
        };
        if ed.busy {
            return;
        }
        let Some((node, slug)) = self.state.selected.clone() else {
            return;
        };
        let title = self.title.read(cx).text.clone();
        let goal = self.goal.read(cx).text.clone();
        let extra = self.extra.read(cx).text.clone();
        let operation = match ed.kind {
            EditKind::Step => {
                let mut brief = ed.brief.clone();
                brief.goal = goal;
                brief.repository = self.repository.read(cx).text.clone();
                brief.work = self.work.read(cx).text.clone();
                brief.files = self
                    .files
                    .read(cx)
                    .text
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                brief.links = self
                    .links
                    .read(cx)
                    .text
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                json!({"editFormation": {"id": ed.id, "title": title, "brief": brief}})
            }
            EditKind::Input => {
                let outputs: Vec<_> = ed
                    .outputs
                    .iter()
                    .map(|o| parvi_missions_contract::MissionOutput {
                        name: o.name.read(cx).text.clone(),
                        description: o.description.read(cx).text.clone(),
                        kind: o.kind.read(cx).text.clone(),
                        required: o.required,
                        from: o.from.read(cx).text.clone(),
                    })
                    .collect();
                if outputs.iter().any(|o| o.name.trim().is_empty()) {
                    ed.error = Some("Every output needs a name.".into());
                    cx.notify();
                    return;
                }
                json!({"updateInputCard": {"id": ed.id, "title": title, "goal": goal, "humanChannel":extra, "outputs":outputs}})
            }
            EditKind::Gate => {
                json!({"updateGate": {"id":ed.id, "title":title, "criterion":goal, "kinds":extra.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>(), "check":self.check.read(cx).text, "checkVersion":self.check_version.read(cx).text, "checkValue":self.check_value.read(cx).text, "files":self.files.read(cx).text.lines().map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>()}})
            }
            EditKind::End => json!({"updateEnd": {"id": ed.id, "title":title, "outcome":extra}}),
        };
        let before = self.state.mission.as_ref().and_then(|m| {
            serde_json::to_value(parvi_missions_contract::document::MissionDocument {
                mission: m.clone(),
                etag: None,
            })
            .ok()
        });
        let reverse = before
            .as_ref()
            .and_then(|before| {
                undo::inverse(
                    before,
                    self.state
                        .layout
                        .as_ref()
                        .map(|l| l.nodes.as_slice())
                        .unwrap_or(&[]),
                    &undo::Step::Board(operation.clone()),
                )
                .ok()
            })
            .unwrap_or_default();
        let mut body = operation;
        body["expectedRev"] = json!(ed.rev);
        body["updatedBy"] = json!("user:parvi-app");
        let path = mission_path(&slug);
        let request = Request::new("PATCH", NodeId(node.clone()), path)
            .header("If-Match", ed.etag.clone())
            .json(&body);
        ed.busy = true;
        ed.error = None;
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }
                match answer {
                    Ok(answer) if answer.ok() => {
                        match serde_json::from_slice::<EditResponse>(&answer.body) {
                            Ok(v) => {
                                page.history.wrote(undo::Version {
                                    rev: v.mission.mission.rev,
                                    layout: Some(v.layout.etag.clone()),
                                });
                                page.history.record(undo::Entry {
                                    label: "edit card".into(),
                                    steps: reverse,
                                });
                                page.state.mission_etag = v.mission.etag;
                                page.state.mission = Some(v.mission.mission);
                                page.state.layout_etag = Some(v.layout.etag);
                                page.state.layout = Some(v.layout.layout);
                                page.editor = None;
                            }
                            Err(e) => {
                                if let Some(ed) = &mut page.editor {
                                    ed.busy = false;
                                    ed.error = Some(e.to_string());
                                }
                            }
                        }
                    }
                    answer => {
                        if let Some(ed) = &mut page.editor {
                            ed.busy = false;
                            ed.error = Some(match answer {
                                Ok(a) => serde_json::from_slice::<ErrorResponse>(&a.body)
                                    .map(|e| format!("{}: {}", e.error.code, e.error.message))
                                    .unwrap_or_else(|_| a.error_text()),
                                Err(e) => format!("{}: {}", e.code, e.message),
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
fn mission_path(slug: &str) -> String {
    let encoded: String = slug
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("/api/missions/{encoded}")
}
impl Page for MissionsPage {
    const ID: &'static str = "missions";
    const TITLE: &'static str = "Missions";
    const AREA: Option<&'static str> = Some("missions");
    const KINDS: parvi_core::frame::Kinds = parvi_missions_contract::KINDS;
    const CHANNELS: &'static [u8] = &[WATCH, WATCH_RUNS];
    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.frame(node.as_str(), kind, body);
        if self.editor.is_none()
            && kind == parvi_missions_contract::SNAPSHOT
            && let Some((selected_node, slug)) = self.state.selected.clone()
            && selected_node == node.as_str()
            && let Some(mission) = &self.state.mission
            && let Some(listing) = self
                .state
                .nodes
                .get(&selected_node)
                .and_then(|n| n.missions.as_ref())
                .and_then(|list| list.iter().find(|m| m.slug == slug))
            && listing.rev.is_some_and(|rev| rev != mission.rev)
        {
            self.open(selected_node, slug, cx);
        }
        cx.notify();
    }
}
impl Render for MissionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut list = div().flex().flex_col().gap(px(PAD));
        for (node, feed) in &self.state.nodes {
            if let Some(missions) = &feed.missions {
                for mission in missions {
                    let (node, slug) = (node.clone(), mission.slug.clone());
                    let label = if mission.title.is_empty() {
                        slug.clone()
                    } else {
                        mission.title.clone()
                    };
                    let selector = format!("mission-{node}-{slug}");
                    list = list.child(
                        div()
                            .id(gpui::SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .p(px(PAD))
                            .cursor_pointer()
                            .border_1()
                            .border_color(t.divider)
                            .child(label)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.open(node.clone(), slug.clone(), cx)
                            })),
                    );
                    let mut runs: Vec<_> = feed
                        .runs
                        .values()
                        .filter(|r| r.mission_slug == mission.slug)
                        .collect();
                    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                    for run in runs {
                        list = list.child(
                            div()
                                .px(px(PAD))
                                .text_color(t.text2)
                                .child(format!("{} · {}", run.run_id, run.status)),
                        );
                    }
                }
            }
        }
        if self.state.selected.is_some()
            && self.editor.is_none()
            && let Some(notes) = &self.notes
        {
            list = list.child("Mission notes");
            for note in &notes.mission {
                let id = note.id.clone();
                let selected = id.clone();
                let text = note.text.clone();
                if note.author == "human:ui" {
                    list = list.child(
                        kit::button(
                            &t,
                            gpui::SharedString::from(format!("mission-note-{id}")),
                            note.text.clone(),
                            Kind::Quiet,
                        )
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.note_id = Some(selected.clone());
                            p.note.update(cx, |f, cx| f.set(&text, false, cx));
                            cx.notify();
                        })),
                    );
                } else {
                    list = list.child(div().child(note.text.clone()));
                }
            }
            list = list
                .child(div().child(self.note.clone()))
                .child(
                    kit::button(
                        &t,
                        "mission-add-note",
                        if self.note_id.is_some() {
                            "Save note"
                        } else {
                            "Add note"
                        },
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(|p, _, _, cx| p.add_note(false, cx))),
                )
                .when(self.note_id.is_some(), |d| {
                    d.child(
                        kit::button(&t, "mission-delete-note", "Delete note", Kind::Quiet)
                            .on_click(cx.listener(|p, _, _, cx| p.add_note(true, cx))),
                    )
                });
        }
        let content: Div = if let Some(m) = &self.state.mission {
            let layout = self
                .state
                .layout
                .as_ref()
                .map(|l| l.nodes.as_slice())
                .unwrap_or(&[]);
            let board = board::board(m, &flow::build(m), layout, false);
            let (x, y, width, height) = board::extent(&board);
            let dx = PAD - x.min(0.);
            let dy = PAD - (y - 24.).min(0.);
            let wires = board.wires.clone();
            let ink = t.text2;
            let lines = canvas(
                |_, _, _| (),
                boundary::paint(move |b: Bounds<Pixels>, _: (), window, _| {
                    for wire in &wires {
                        let at = |p: (f32, f32)| {
                            point(b.origin.x + px(p.0 + dx), b.origin.y + px(p.1 + dy))
                        };
                        let mut path = PathBuilder::stroke(px(EDGE));
                        path.move_to(at(wire.from));
                        if let Some(y) = wire.lane {
                            path.line_to(at((wire.from.0, y)));
                            path.line_to(at((wire.to.0, y)));
                        }
                        path.line_to(at(wire.to));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, ink);
                        }
                    }
                }),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut drawing = div()
                .relative()
                .flex_none()
                .w(px(width + x + dx + PAD))
                .h(px(height + y + dy + PAD))
                .child(lines);
            for card in board.cards {
                let selector = format!("mission-card-{}", card.id);
                let card_id = card.id.clone();
                let details = m
                    .formations
                    .iter()
                    .find(|f| f.id == card.id)
                    .map(|f| {
                        let brief = f.brief.as_ref().map(|b| b.goal.as_str()).unwrap_or("");
                        let slots = f
                            .slots
                            .iter()
                            .map(|s| s.name())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("{}\n{}\n{}", f.kind, brief, slots)
                    })
                    .or_else(|| {
                        m.input_cards
                            .iter()
                            .find(|c| c.id == card.id)
                            .map(|c| c.goal.clone())
                    })
                    .unwrap_or_default();
                drawing = drawing.child(
                    div()
                        .id(gpui::SharedString::from(format!("card-{}", card.id)))
                        .absolute()
                        .left(px(card.x + dx))
                        .top(px(card.y + dy))
                        .w(px(card.w))
                        .h(px(card.h))
                        .p(px(PAD))
                        .bg(t.bg)
                        .border_1()
                        .border_color(t.divider)
                        .rounded(px(size::RADIUS))
                        .debug_selector(move || selector.clone())
                        .child(m.node_name(&card.id).to_owned())
                        .child(div().text_color(t.text2).child(details))
                        .on_click(cx.listener(move |page, _, _, cx| page.edit_step(&card_id, cx))),
                );
            }
            drawing
        } else {
            div().p(px(PAD)).child(
                self.state
                    .error
                    .clone()
                    .unwrap_or_else(|| "Select a mission".into()),
            )
        };
        div()
            .size_full()
            .flex()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                div()
                    .id("missions-list")
                    .w(px(LIST_W))
                    .flex_none()
                    .overflow_y_scroll()
                    .child(kit::button(&t, "new-mission", "+ Mission", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.create_mission(cx))))
                    .child(list)
                    .when(self.state.selected.is_some(), |d| d.child(kit::button(&t, "delete-mission", if self.deleting_mission { "Confirm delete mission" } else { "Delete mission" }, Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.delete_mission(cx))))),
            )
            .child(
                div()
                    .id("missions-canvas")
                    .flex_1()
                    .min_w_0()
                    .flex().flex_col()
                    .child(div().flex().gap(px(PAD))
                        .child(kit::button(&t, "new-input", "+ Input", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.write_board(json!({"createInputCard":{"title":"Input","goal":"","x":200,"y":200}}), WriteMode::Edit, cx))))
                        .child(kit::button(&t, "new-step", "+ Step", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.write_board(json!({"createFormation":{"type":"solo","title":"New step","x":200,"y":200}}), WriteMode::Edit, cx))))
                        .child(kit::button(&t, "new-gate", "+ Gate", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.write_board(json!({"createGate":{"title":"New gate","kinds":["human"],"x":200,"y":200}}), WriteMode::Edit, cx))))
                        .child(kit::button(&t, "new-end", "+ End", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.write_board(json!({"createEnd":{"title":"End","outcome":"done","x":200,"y":200}}), WriteMode::Edit, cx))))
                        .child(kit::button(&t, "arrange", "Arrange", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.arrange(cx))))
                        .child(kit::button(&t, "undo", "Undo", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.undo(false, cx))))
                        .child(kit::button(&t, "redo", "Redo", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.undo(true, cx)))))
                    .child(div().flex().gap(px(PAD)).child(div().w(px(LINK_FIELD_W)).child(self.source.clone())).child(div().w(px(LINK_FIELD_W)).child(self.target.clone())).child(kit::button(&t, "link", "Connect", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.link(cx)))))
                    .when_some(self.state.error.clone(), |d, e| d.child(div().text_color(t.error).child(e)))
                    .child(div().id("missions-board-scroll").flex_1().overflow_scroll().child(content)),
            )
            .when_some(self.editor.as_ref(), |root, ed| {
                root.child(
                    div()
                        .id("step-editor")
                        .overflow_y_scroll()
                        .w(px(EDITOR_W))
                        .flex_none()
                        .p(px(PAD))
                        .flex()
                        .flex_col()
                        .gap(px(PAD))
                        .child(match ed.kind {
                            EditKind::Step => "Edit step",
                            EditKind::Input => "Edit input",
                            EditKind::Gate => "Edit gate",
                            EditKind::End => "Edit end",
                        })
                        .child(
                            kit::button(&t, "step-save", "Save", Kind::Primary)
                                .debug_selector(|| "step-save".into())
                                .on_click(cx.listener(|p, _, _, cx| p.save_step(cx))),
                        )
                        .child(kit::button(&t, "step-delete", if ed.deleting { "Confirm delete" } else { "Delete" }, Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.delete_card(cx))))
                        .child("Title")
                        .child(
                            div()
                                .debug_selector(|| "step-title".into())
                                .child(self.title.clone()),
                        )
                        .child(match ed.kind {
                            EditKind::Step => "Brief",
                            EditKind::Input => "Goal",
                            EditKind::Gate => "Criterion",
                            EditKind::End => "Description",
                        })
                        .child(
                            div()
                                .debug_selector(|| "step-goal".into())
                                .child(self.goal.clone()),
                        )
                        .when(matches!(ed.kind, EditKind::Step), |d| d.child("Formation type")
                            .child(kit::button(&t, "type-solo", "Solo", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.set_type("solo", cx))))
                            .child(kit::button(&t, "type-peer", "Peer", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.set_type("peer", cx))))
                            .child(kit::button(&t, "type-orchestrated", "Orchestrated", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.set_type("orchestrated", cx))))
                            .child("Device").child(div().child(self.extra.clone()))
                            .child(kit::button(&t, "set-device", "Set device", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.set_device(cx)))))
                        .when(matches!(ed.kind, EditKind::Step), |d| d.child("Repository").child(div().child(self.repository.clone()))
                            .child("Work").child(div().child(self.work.clone()))
                            .child("Files").child(div().child(self.files.clone()))
                            .child("Links").child(div().child(self.links.clone())))
                        .when(!matches!(ed.kind, EditKind::Step), |d| {
                            d.child(match ed.kind {
                                EditKind::Input => "Human channel",
                                EditKind::Gate => "Kinds (comma separated)",
                                EditKind::End => "Outcome",
                                _ => "",
                            })
                            .child(div().child(self.extra.clone()))
                        })
                        .when(matches!(ed.kind, EditKind::Step), |mut d| {
                            if let Some(f) = self.state.mission.as_ref().and_then(|m| m.formations.iter().find(|f| f.id == ed.id)) {
                                d = d.child("Ports");
                                for port in f.inputs.iter().chain(&f.outputs) {
                                    let id = port.id.clone(); let selected = id.clone(); let label = port.label.clone(); let kind = port.kind.clone();
                                    d = d.child(kit::button(&t, gpui::SharedString::from(format!("port-{id}")), format!("{} ({})", port.label, id), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { p.port = Some(selected.clone()); p.port_label.update(cx, |f, cx| f.set(&label, false, cx)); p.port_kind.update(cx, |f, cx| f.set(&kind, false, cx)); cx.notify(); })));
                                }
                                d = d.child(div().child(self.port_label.clone())).child(div().child(self.port_kind.clone()))
                                    .child(kit::button(&t, "port-add-in", "+ Input port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("input", cx))))
                                    .child(kit::button(&t, "port-add-out", "+ Output port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("output", cx))))
                                    .when(self.port.is_some(), |d| d.child(kit::button(&t, "port-update", "Rename port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("update", cx)))).child(kit::button(&t, "port-remove", "Remove port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("remove", cx)))))
                                    .child("Slots");
                                for slot in &f.slots {
                                    let id = slot.id.clone(); let selected = id.clone();
                                    d = d.child(kit::button(&t, gpui::SharedString::from(format!("slot-{id}")), slot.name(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| p.select_slot(&selected, cx))));
                                }
                            }
                            d
                        })
                        .when(matches!(ed.kind, EditKind::Step) && self.slot.is_some(), |mut d| {
                            d = d.child("Staff selected slot");
                            if let Some(catalog) = &self.roles {
                                for role in catalog.roles.iter().filter(|r| r.status != "retired") {
                                    let id = role.id.clone(); let selected = id.clone();
                                    d = d.child(kit::button(&t, gpui::SharedString::from(format!("role-{id}")), role.display_name.clone(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { p.role.update(cx, |f, cx| f.set(&selected, false, cx)); })));
                                }
                                for harness in &catalog.harnesses {
                                    let id = harness.id.clone(); let selected = id.clone();
                                    d = d.child(kit::button(&t, gpui::SharedString::from(format!("harness-{id}")), id, Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { p.choose_harness(&selected, cx); })));
                                }
                            }
                            d.child(div().child(self.role.clone())).child(div().child(self.harness.clone()))
                                .child(div().child(self.model.clone())).child(div().child(self.effort.clone()))
                                .child(kit::button(&t, "assign-slot", "Assign", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.assign_slot(cx))))
                        })
                        .when(matches!(ed.kind, EditKind::Step | EditKind::Input), |mut d| {
                            d = d.child("Limits");
                            if let Some(mission) = &self.state.mission { for limit in mission.limits.iter().filter(|l| l.target == ed.id) {
                                let id = limit.id.clone(); let selected = id.clone();
                                d = d.child(kit::button(&t, gpui::SharedString::from(format!("limit-{id}")), limit.title.clone(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| p.select_limit(&selected, cx))));
                            } }
                            d = d.child(kit::button(&t, "create-limit", "Add limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("create", cx))));
                            d.when(self.limit.is_some(), |d| d.child(div().child(self.limit_title.clone())).child(div().child(self.limit_rounds.clone())).child(div().child(self.limit_seconds.clone())).child(div().child(self.limit_warn.clone())).child(div().child(self.limit_tokens.clone())).child(div().child(self.limit_spent.clone())).child(kit::button(&t, "save-limit", "Save limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("save", cx)))).child(kit::button(&t, "delete-limit", "Delete limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("delete", cx)))) )
                        })
                        .when(matches!(ed.kind, EditKind::Input), |mut d| {
                            d = d.child("Outputs");
                            for (i, output) in ed.outputs.iter().enumerate() {
                                d = d.child(div().child(output.name.clone())).child(div().child(output.description.clone()))
                                    .child(kit::button(&t, ("required", i), if output.required { "Required" } else { "Optional" }, Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { if let Some(o) = p.editor.as_mut().and_then(|e| e.outputs.get_mut(i)) { o.required = !o.required; } cx.notify(); })))
                                    .child(kit::button(&t, ("remove-output", i), "Remove output", Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { if let Some(e) = &mut p.editor && i < e.outputs.len() { e.outputs.remove(i); } cx.notify(); })))
                                     .child(div().child(output.kind.clone())).child(div().child(output.from.clone()));
                            }
                            d.child(kit::button(&t, "add-output", "Add output", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_output(cx))))
                        })
                        .when(matches!(ed.kind, EditKind::Gate), |d| d.child("Check profile").child(div().child(self.check.clone())).child("Profile version").child(div().child(self.check_version.clone())).child("Check value").child(div().child(self.check_value.clone())).child("Files").child(div().child(self.files.clone())))
                        .when_some(self.notes.as_ref(), |mut d, notes| {
                            d = d.child("Notes");
                            if let Some(thread) = notes.elements.iter().find(|n| n.node_id == ed.id) {
                                for note in &thread.entries {
                                    let id = note.id.clone(); let selected = id.clone(); let text = note.text.clone();
                                    if note.author == "human:ui" {
                                        d = d.child(kit::button(&t, gpui::SharedString::from(format!("note-{id}")), note.text.clone(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { p.note_id = Some(selected.clone()); p.note.update(cx, |f, cx| f.set(&text, false, cx)); cx.notify(); })));
                                    } else { d = d.child(div().child(note.text.clone())); }
                                }
                            }
                            d.child(div().child(self.note.clone()))
                                .child(kit::button(&t, "add-note", if self.note_id.is_some() { "Save note" } else { "Add note" }, Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_note(false, cx))))
                                .when(self.note_id.is_some(), |d| d.child(kit::button(&t, "delete-note", "Delete note", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_note(true, cx)))))
                        })
                        .when_some(ed.error.clone(), |d, error| {
                            d.child(
                                div()
                                    .debug_selector({ let error = error.clone(); move || format!("step-error-{error}") })
                                    .text_color(t.error)
                                    .child(error),
                            )
                        })
                        .child(
                            kit::button(&t, "step-cancel", "Cancel", Kind::Quiet).on_click(
                                cx.listener(|p, _, _, cx| {
                                    if !p.editor.as_ref().is_some_and(|e| e.busy) { p.editor = None; }
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
    }
}
