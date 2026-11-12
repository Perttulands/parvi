mod canvas;
mod editor_diff;
mod edits;
mod flow_view;
mod manage;
mod navigator;
mod notes;
mod run;
mod staffing;
mod validation;
use crate::store::{Store, canvas as board, flow, undo};
use gpui::{
    AppContext, Context, Div, Entity, FocusHandle, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, Render, SystemNotification, Window, div, prelude::*, px,
};
use parvi_app_kit::{
    Host, Page,
    host::Request,
    theme::{size, theme},
    widgets::{
        field::{FieldEvent, TextField},
        kit::{self, Kind},
    },
};
use parvi_core::NodeId;
use parvi_missions_contract::{
    EditResponse, ErrorResponse, LayoutResponse, MissionResponse, WATCH, WATCH_RUNS,
};
use serde_json::json;

const PAD: f32 = 12.;
const START_FORM_MAX: f32 = 680.;
const TAB_GAP: f32 = 6.;
const EDGE: f32 = 2.;
const EDITOR_W: f32 = 360.;
pub struct MissionsPage {
    host: Host,
    focus: FocusHandle,
    state: Store,
    validation: crate::store::validation::Validation,
    navigator: navigator::Navigator,
    manage: manage::Manage,
    canvas: canvas::View,
    mission_canvas_mode: bool,
    note_paper: notes::NotesUi,
    run: run::RunUi,
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
    history: undo::History,
    writing: bool,
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
    note_base: String,
    note_etag: Option<String>,
    note_saving: bool,
    wanted: Option<Wanted>,
    /// A run output on its way into the start form of the mission opening.
    carry: Option<String>,
    reveal_card: Option<String>,
    /// A step being added: once it exists, wire it from this port (if any)
    /// and open it in the editor.
    new_step: Option<Option<String>>,
    seen_run_snapshots: std::collections::BTreeSet<String>,
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
}
struct OutputDraft {
    name: Entity<TextField>,
    description: Entity<TextField>,
    kind: Entity<TextField>,
    required: bool,
    from: Entity<TextField>,
}
#[derive(Clone, Debug)]
struct Wanted {
    what: String,
    id: String,
    node: Option<NodeId>,
    part: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorSnapshot {
    fields: Vec<String>,
    outputs: Vec<(String, String, String, bool, String)>,
}
impl EditorSnapshot {
    fn accept_fields(&mut self, submitted: &Self, range: std::ops::Range<usize>) {
        self.fields[range.clone()].clone_from_slice(&submitted.fields[range]);
    }
}
struct Editor {
    kind: EditKind,
    id: String,
    rev: i64,
    etag: String,
    busy: bool,
    error: Option<String>,
    deleting: bool,
    baseline: Option<EditorSnapshot>,
    outputs: Vec<OutputDraft>,
}
impl MissionsPage {
    pub fn new(host: Host, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let page = Self {
            host,
            focus: cx.focus_handle(),
            state: Store::default(),
            validation: Default::default(),
            navigator: navigator::Navigator::new(cx),
            manage: manage::Manage::default(),
            canvas: canvas::View::default(),
            mission_canvas_mode: false,
            note_paper: notes::NotesUi::default(),
            run: run::RunUi::new(cx),
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
            history: undo::History::default(),
            writing: false,
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
            note_base: String::new(),
            note_etag: None,
            note_saving: false,
            wanted: None,
            carry: None,
            reveal_card: None,
            new_step: None,
            seen_run_snapshots: Default::default(),
        };
        for field in [
            &page.title,
            &page.goal,
            &page.extra,
            &page.repository,
            &page.work,
            &page.files,
            &page.links,
            &page.check,
            &page.check_version,
            &page.check_value,
            &page.role,
            &page.harness,
            &page.model,
            &page.effort,
            &page.port_label,
            &page.port_kind,
            &page.limit_title,
            &page.limit_rounds,
            &page.limit_seconds,
            &page.limit_warn,
            &page.limit_tokens,
            &page.limit_spent,
            &page.note,
        ] {
            Self::watch_editor_field(field, cx);
        }
        cx.subscribe(&page.note, |page, field, event: &FieldEvent, cx| {
            if matches!(event, FieldEvent::Changed)
                && page.note_etag.is_none()
                && field.read(cx).text != page.note_base
            {
                page.note_etag = page.notes.as_ref().map(|notes| notes.etag.clone());
            }
        })
        .detach();
        page.host.channel(WATCH_RUNS, true, cx);
        host_guard(&page.host, cx);
        page
    }
    fn report_new_asks(
        &mut self,
        node: &NodeId,
        body: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(runs) = serde_json::from_slice::<Vec<parvi_missions_contract::RunProjection>>(body)
        else {
            return;
        };
        let initial = self.seen_run_snapshots.insert(node.to_string());
        if initial {
            return;
        }
        for run in runs {
            let previous = self
                .state
                .nodes
                .get(node.as_str())
                .and_then(|feed| feed.runs.get(&run.run_id));
            if previous.is_some_and(|old| old.event_count >= run.event_count) || run.r#final {
                continue;
            }
            for gate in &run.waiting_gates {
                if previous.is_some_and(|old| {
                    old.waiting_gates
                        .iter()
                        .any(|g| g.gate_id == gate.gate_id && g.requested_seq == gate.requested_seq)
                }) {
                    continue;
                }
                if !window.is_window_active() {
                    let page = cx.entity().downgrade();
                    let run_id = run.run_id.clone();
                    let run_node = node.clone();
                    self.host.notify(
                        SystemNotification {
                            tag: format!("mission-{}-{}-{}", node, run.run_id, gate.requested_seq)
                                .into(),
                            title: format!("{} needs you", run.mission_slug).into(),
                            body: format!("Gate {} is waiting for your answer", gate.gate_id)
                                .into(),
                            actions: vec![],
                        },
                        move |_, cx| {
                            let _ = page.update(cx, |page, cx| {
                                page.host.show(cx);
                                page.wanted = Some(Wanted {
                                    what: "run".into(),
                                    id: run_id.clone(),
                                    node: Some(run_node.clone()),
                                    part: None,
                                });
                                page.resolve_wanted(cx);
                            });
                        },
                        cx,
                    );
                }
            }
            let blocked_seq = |run: &parvi_missions_contract::RunProjection| {
                run.events
                    .iter()
                    .rev()
                    .find(|event| event.kind == "run_blocked")
                    .map(|event| event.seq)
            };
            if run.status == "blocked"
                && previous.is_none_or(|old| {
                    old.status != "blocked" || blocked_seq(old) != blocked_seq(&run)
                })
                && !window.is_window_active()
            {
                let page = cx.entity().downgrade();
                let run_id = run.run_id.clone();
                let run_node = node.clone();
                self.host.notify(
                    SystemNotification {
                        tag: format!(
                            "mission-{}-{}-blocked-{}",
                            node, run.run_id, run.event_count
                        )
                        .into(),
                        title: format!("{} needs you", run.mission_slug).into(),
                        body: "Run is blocked".into(),
                        actions: vec![],
                    },
                    move |_, cx| {
                        let _ = page.update(cx, |page, cx| {
                            page.host.show(cx);
                            page.wanted = Some(Wanted {
                                what: "run".into(),
                                id: run_id.clone(),
                                node: Some(run_node.clone()),
                                part: None,
                            });
                            page.resolve_wanted(cx);
                        });
                    },
                    cx,
                );
            }
        }
    }
    fn watch_editor_field(field: &Entity<TextField>, cx: &mut Context<Self>) {
        cx.subscribe(field, |page, _, event: &FieldEvent, cx| {
            if matches!(event, FieldEvent::Changed) && page.wanted.is_some() {
                page.resolve_wanted(cx);
            }
        })
        .detach();
    }
    fn unsaved(&self, cx: &gpui::App) -> Option<String> {
        if let Some(reason) = self.manage_unsaved(cx) {
            return Some(reason);
        }
        if let Some(editor) = &self.editor
            && editor
                .baseline
                .as_ref()
                .is_some_and(|base| *base != self.editor_snapshot(editor, cx))
        {
            return Some(format!("Mission card {}", editor.id));
        }
        if self.note_saving {
            return Some("Saving mission note".into());
        }
        if self.note.read(cx).text != self.note_base {
            return Some("Mission note".into());
        }
        self.run_unsaved(cx)
    }
    fn editor_snapshot(&self, editor: &Editor, cx: &gpui::App) -> EditorSnapshot {
        let fields = [
            &self.title,
            &self.goal,
            &self.extra,
            &self.repository,
            &self.work,
            &self.files,
            &self.links,
            &self.check,
            &self.check_version,
            &self.check_value,
            &self.role,
            &self.harness,
            &self.model,
            &self.effort,
            &self.port_label,
            &self.port_kind,
            &self.limit_title,
            &self.limit_rounds,
            &self.limit_seconds,
            &self.limit_warn,
            &self.limit_tokens,
            &self.limit_spent,
        ]
        .iter()
        .map(|field| field.read(cx).text.clone())
        .collect();
        let outputs = editor
            .outputs
            .iter()
            .map(|o| {
                (
                    o.name.read(cx).text.clone(),
                    o.description.read(cx).text.clone(),
                    o.kind.read(cx).text.clone(),
                    o.required,
                    o.from.read(cx).text.clone(),
                )
            })
            .collect();
        EditorSnapshot { fields, outputs }
    }
    fn resolve_wanted(&mut self, cx: &mut Context<Self>) {
        let Some(wanted) = self.wanted.clone() else {
            return;
        };
        if self.writing
            || self.editor.as_ref().is_some_and(|e| e.busy)
            || self.unsaved(cx).is_some()
        {
            self.state.error =
                Some("Save or discard the current draft before opening the requested item.".into());
            cx.notify();
            return;
        }
        let live: Vec<String> = self
            .host
            .nodes(cx)
            .into_iter()
            .map(|n| n.id.to_string())
            .collect();
        let Some((node, slug)) = self.state.find_reveal(
            &wanted.what,
            &wanted.id,
            wanted.node.as_ref().map(NodeId::as_str),
            &live,
        ) else {
            return;
        };
        self.wanted = None;
        self.open(node.clone(), slug.clone(), cx);
        if self.state.selected.as_ref() != Some(&(node, slug)) {
            self.wanted = Some(wanted);
            return;
        }
        if wanted.what == "run" {
            self.reveal_run(wanted.id, wanted.part, cx);
        } else {
            self.reveal_card = wanted.part;
            cx.notify();
        }
    }
    fn open(&mut self, node: String, slug: String, cx: &mut Context<Self>) {
        if self.unsaved(cx).is_some() && !self.start_uncertain() {
            self.state.error =
                Some("Save or discard the current draft before opening another mission.".into());
            cx.notify();
            return;
        }
        if self.start_uncertain() {
            self.run.error = Some("The start outcome is unknown. Retry it before leaving.".into());
            cx.notify();
            return;
        }
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        self.run = run::RunUi::new(cx);
        if self.state.selected.as_ref() != Some(&(node.clone(), slug.clone())) {
            self.canvas = canvas::View::default();
        }
        self.reveal_card = None;
        self.generation += 1;
        let generation = self.generation;
        self.state.selected = Some((node.clone(), slug.clone()));
        self.state.mission = None;
        self.state.mission_etag = None;
        self.validation.clear();
        self.state.layout_etag = None;
        self.editor = None;
        self.history = undo::History::default();
        self.state.layout = None;
        self.notes = None;
        self.note_id = None;
        self.note_base.clear();
        self.note_etag = None;
        self.note_paper = notes::NotesUi::default();
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
        self.load_canvas_notes(node.clone(), slug.clone(), cx);
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
                                        if let Some(value) = page.carry.take() {
                                            page.start_with(&value, cx);
                                        }
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
    fn refresh_selected_if_newer(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_none()
            && !self.writing
            && !self.is_showing_run()
            && let Some((node, slug)) = self.state.selected.clone()
            && let Some(mission) = &self.state.mission
            && let Some(listing) = self
                .state
                .nodes
                .get(&node)
                .and_then(|feed| feed.missions.as_ref())
                .and_then(|list| list.iter().find(|item| item.slug == slug))
            && ((!listing.id.is_empty() && listing.id != mission.id)
                || listing.rev.is_some_and(|rev| rev > mission.rev))
        {
            self.open(node, slug, cx);
        }
    }
    fn edit_step(&mut self, id: &str, cx: &mut Context<Self>) {
        // The card being edited stays in view beside the editor.
        self.canvas.follow(Some(id.to_owned()));
        if self.writing || self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        if self.unsaved(cx).is_some() {
            self.state.error = Some("Save or discard the current draft first.".into());
            cx.notify();
            return;
        }
        if self.note_paper.open.is_some() {
            self.note_paper = notes::NotesUi::default();
            self.note_id = None;
            self.note_base.clear();
            self.note_etag = None;
            self.note.update(cx, |field, cx| field.set("", false, cx));
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
                        for field in [&name, &description, &kind, &from] {
                            Self::watch_editor_field(field, cx);
                        }
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
            busy: false,
            error: None,
            deleting: false,
            baseline: None,
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
        let snapshot = self
            .editor
            .as_ref()
            .map(|editor| self.editor_snapshot(editor, cx));
        if let Some(editor) = &mut self.editor {
            editor.baseline = snapshot;
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
        let reverse =
            undo::inverse(&before, layout, &undo::Step::Board(patch.clone())).unwrap_or_default();
        let submitted = self.editor.as_ref().map(|ed| self.editor_snapshot(ed, cx));
        let saved_fields = if patch.get("assignSlot").is_some() {
            Some(10..14)
        } else if patch
            .get("updateFormation")
            .is_some_and(|v| v.get("device").is_some())
        {
            Some(2..3)
        } else if ["addPort", "updatePort", "removePort"]
            .iter()
            .any(|op| patch.get(*op).is_some())
        {
            Some(14..16)
        } else if ["updateLimit", "deleteLimit"]
            .iter()
            .any(|op| patch.get(*op).is_some())
        {
            Some(16..22)
        } else {
            None
        };
        let deleted_card = [
            "deleteFormation",
            "deleteInputCard",
            "deleteGate",
            "deleteEnd",
        ]
        .iter()
        .find_map(|key| {
            patch
                .get(*key)
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
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
                                match mode {
                                    WriteMode::Edit => page.history.record(entry),
                                }
                                if page.editor.as_ref().is_some_and(|ed| {
                                    deleted_card.as_deref() == Some(ed.id.as_str())
                                }) {
                                    page.editor = None;
                                } else if let Some(ed) = &mut page.editor {
                                    if let (Some(base), Some(sent), Some(range)) =
                                        (&mut ed.baseline, &submitted, &saved_fields)
                                    {
                                        base.accept_fields(sent, range.clone());
                                    }
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
                                page.created_step(&mission, cx);
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
                page.refresh_selected_if_newer(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.step_history(redo, cx);
    }
    fn select_note(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        if self.note_saving || self.note.read(cx).text != self.note_base {
            self.state.error = Some("Save or discard the current note draft first.".into());
            cx.notify();
            return;
        }
        self.note_id = Some(id.to_owned());
        self.note_base = text.to_owned();
        self.note_etag = self.notes.as_ref().map(|notes| notes.etag.clone());
        self.note.update(cx, |f, cx| f.set(text, false, cx));
        cx.notify();
    }
    fn discard_note(&mut self, cx: &mut Context<Self>) {
        if self.note_saving {
            return;
        }
        self.note_id = None;
        self.note_base.clear();
        self.note_etag = None;
        self.note.update(cx, |f, cx| f.set("", false, cx));
        self.resolve_wanted(cx);
        cx.notify();
    }
    fn add_note(&mut self, delete: bool, cx: &mut Context<Self>) {
        if self.note_saving {
            return;
        }
        let (Some((node, slug)), Some(notes)) = (self.state.selected.clone(), self.notes.as_ref())
        else {
            return;
        };
        let text = self.note.read(cx).text.clone();
        if text.trim().is_empty() && !delete {
            return;
        }
        let target = self
            .note_paper
            .open
            .as_deref()
            .or_else(|| self.editor.as_ref().map(|e| e.id.as_str()))
            .unwrap_or("mission");
        let sent_text = text.clone();
        let sent_id = self.note_id.clone();
        let request = Request::new(
            "PATCH",
            NodeId(node),
            format!("{}/notes", mission_path(&slug)),
        )
        .header("If-Match", self.note_etag.clone().unwrap_or_else(|| notes.etag.clone()))
        .json(&json!({"target":target, "text":text,"author":"human:ui", "action":if delete {"delete"} else if self.note_id.is_some() {"edit"} else {"append"}, "entryId":self.note_id}));
        self.note_saving = true;
        let asked = self.host.request(request, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                page.note_saving = false;
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<
                        parvi_missions_contract::NotesResponse,
                    >(&a.body)
                    {
                        Ok(n) => {
                            let next_etag = n.notes.etag.clone();
                            page.notes = Some(n.notes);
                            if page.note.read(cx).text == sent_text {
                                page.note_id = None;
                                page.note_base.clear();
                                page.note_etag = None;
                                page.note.update(cx, |f, cx| f.set("", false, cx));
                            } else if sent_id.is_some() && !delete {
                                page.note_id = sent_id;
                                page.note_base = sent_text;
                                page.note_etag = Some(next_etag.clone());
                            } else {
                                page.note_id = None;
                                page.note_base.clear();
                                page.note_etag = Some(next_etag);
                            }
                            page.resolve_wanted(cx);
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
    fn group_is_dirty(&self, range: std::ops::Range<usize>, cx: &gpui::App) -> bool {
        let Some(ed) = &self.editor else {
            return false;
        };
        let Some(base) = &ed.baseline else {
            return false;
        };
        let now = self.editor_snapshot(ed, cx);
        range.into_iter().any(|i| base.fields[i] != now.fields[i])
    }
    fn accept_group(&mut self, range: std::ops::Range<usize>, cx: &gpui::App) {
        let current = self.editor.as_ref().map(|ed| self.editor_snapshot(ed, cx));
        if let (Some(ed), Some(current)) = (&mut self.editor, current)
            && let Some(base) = &mut ed.baseline
        {
            base.accept_fields(&current, range);
        }
    }
    fn select_port(&mut self, id: &str, label: &str, kind: &str, cx: &mut Context<Self>) {
        if self.group_is_dirty(14..16, cx) {
            if let Some(ed) = &mut self.editor {
                ed.error = Some("Save the port draft before selecting another port.".into());
            }
            cx.notify();
            return;
        }
        self.port = Some(id.to_owned());
        self.port_label.update(cx, |f, cx| f.set(label, false, cx));
        self.port_kind.update(cx, |f, cx| f.set(kind, false, cx));
        self.accept_group(14..16, cx);
        cx.notify();
    }
    fn select_slot(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.group_is_dirty(10..14, cx) {
            if let Some(ed) = &mut self.editor {
                ed.error = Some("Save the staffing draft before selecting another slot.".into());
            }
            cx.notify();
            return;
        }
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
        self.accept_group(10..14, cx);
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
        if self.group_is_dirty(16..22, cx) {
            if let Some(ed) = &mut self.editor {
                ed.error = Some("Save the limit draft before selecting another limit.".into());
            }
            cx.notify();
            return;
        }
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
        self.accept_group(16..22, cx);
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
        for field in [&name, &description, &kind, &from] {
            Self::watch_editor_field(field, cx);
        }
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
        self.save_editor_merged(cx);
    }
}
fn host_guard(host: &Host, cx: &mut Context<MissionsPage>) {
    host.guard_close(&cx.entity(), |page: &MissionsPage, cx| page.unsaved(cx), cx);
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
    const KEY: Option<char> = Some('M');
    const AREA: Option<&'static str> = Some("missions");
    const KINDS: parvi_core::frame::Kinds = parvi_missions_contract::KINDS;
    const CHANNELS: &'static [u8] = &[WATCH];
    const ANSWERS: &'static [parvi_app_kit::ask::Answers] = &[
        parvi_app_kit::ask::Answers::Reveal("mission"),
        parvi_app_kit::ask::Answers::Reveal("run"),
    ];
    fn answer(
        &mut self,
        ask: parvi_app_kit::Ask,
        _: &'static str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let parvi_app_kit::Ask::Reveal {
            what,
            id,
            node,
            part,
            ..
        } = ask
            && (what == "mission" || what == "run")
        {
            self.wanted = Some(Wanted {
                what,
                id,
                node,
                part,
            });
            self.resolve_wanted(cx);
        }
    }

    fn focus(&self, _: &gpui::App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn tabs(
        &mut self,
        shown: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = theme(cx).clone();
        let count = self
            .state
            .nodes
            .values()
            .flat_map(|feed| feed.runs.values())
            .filter(|run| crate::store::words::needs_you(run))
            .count();
        let mut tab = kit::tab(&t, "missions-tab", shown, cx)
            .debug_selector(|| "tab-missions".into())
            .child(kit::tab_label(Self::TITLE, cx));
        if count > 0 {
            tab = tab.child(
                div()
                    .debug_selector(move || format!("missions-count-{count}"))
                    .child(count.to_string()),
            );
        }
        Some(
            tab.on_click(cx.listener(|page, _, _, cx| page.host.show(cx)))
                .into_any_element(),
        )
    }

    fn link(
        &mut self,
        node: &NodeId,
        state: Option<parvi_app_kit::link::client::LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state != Some(parvi_app_kit::link::client::LinkState::Open)
            && self
                .state
                .selected
                .as_ref()
                .is_some_and(|(selected, _)| selected == node.as_str())
        {
            self.validation.clear();
        }
        if state.is_none() {
            self.state.nodes.remove(node.as_str());
            self.seen_run_snapshots.remove(node.as_str());
            cx.notify();
        }
    }

    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == parvi_missions_contract::RUNS_SNAPSHOT {
            self.report_new_asks(node, body, window, cx);
        }
        self.state.frame(node.as_str(), kind, body);
        self.resolve_wanted(cx);
        if kind == parvi_missions_contract::RUNS_SNAPSHOT {
            self.sync_gate(cx);
        }
        if kind == parvi_missions_contract::SNAPSHOT
            && self
                .state
                .selected
                .as_ref()
                .is_some_and(|(selected, _)| selected == node.as_str())
        {
            self.refresh_selected_if_newer(cx);
        }
        cx.notify();
    }
}
impl Render for MissionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.setup_start_keys(window, cx);
        self.sync_validation(cx);
        let t = theme(cx).clone();
        let mut list = self.render_navigator(&t, cx);
        if self.state.selected.is_some()
            && (!self.mission_canvas_mode || self.note.read(cx).text != self.note_base)
            && self.editor.is_none()
            && self.note_paper.open.is_none()
            && let Some(notes) = &self.notes
        {
            list = list.child(
                div()
                    .px(px(PAD))
                    .pt(px(PAD))
                    .child(kit::label(&t, "Mission notes")),
            );
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
                            p.select_note(&selected, &text, cx);
                        })),
                    );
                } else {
                    list = list.child(div().child(note.text.clone()));
                }
            }
            list = list
                .child(
                    div()
                        .debug_selector(|| "mission-note-draft".into())
                        .child(self.note.clone()),
                )
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
                    .debug_selector(|| "mission-add-note".into())
                    .on_click(cx.listener(|p, _, _, cx| p.add_note(false, cx))),
                )
                .when(
                    self.note_id.is_some() || self.note.read(cx).text != self.note_base,
                    |d| {
                        d.child(
                            kit::button(
                                &t,
                                "mission-note-cancel",
                                "Discard note draft",
                                Kind::Quiet,
                            )
                            .on_click(cx.listener(|p, _, _, cx| p.discard_note(cx))),
                        )
                    },
                )
                .when(self.note_id.is_some(), |d| {
                    d.child(
                        kit::button(&t, "mission-delete-note", "Delete note", Kind::Quiet)
                            .on_click(cx.listener(|p, _, _, cx| p.add_note(true, cx))),
                    )
                });
        }
        let content: Div = if let Some(view) = self.run_view(&t, cx) {
            view
        } else if let Some(m) = self.state.mission.clone() {
            if self.mission_canvas_mode {
                self.render_canvas(&t, &m, false, cx)
            } else {
                self.render_mission_flow(&t, &m, cx)
            }
        } else {
            let mut empty = div().p(px(PAD)).flex().flex_col().gap(px(TAB_GAP));
            empty = match self.state.error.clone() {
                Some(error) => empty.child(div().text_color(t.error).child(error)),
                None => empty
                    .child(kit::label(&t, "Missions"))
                    .child("Pick a mission on the left to see its steps, or a run to see what it did.")
                    .child(kit::note(&t, "+ Mission starts a new one; parvi mission --help does the same from a terminal.", None)),
            };
            if self.state.error.is_some() && self.state.selected.is_some() {
                empty = empty.child(
                    kit::button(&t, "retry-mission-definition", "Try again", Kind::Plain)
                        .debug_selector(|| "retry-mission-definition".into())
                        .on_click(cx.listener(|page, _, _, cx| {
                            if let Some((node, slug)) = page.state.selected.clone() {
                                page.open(node, slug, cx);
                            }
                        })),
                );
            }
            empty
        };
        let authoring = self.state.mission.is_some() && !self.is_showing_run();
        let form = self.mission_form(&t, cx);
        let start_form = self.run_start_form(&t, window, cx);
        let attempts_form = self.attempts_form(&t, cx);
        let stop_form = self.stop_form(&t, cx);
        div()
            .id("missions-root")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .on_mouse_down(MouseButton::Left, cx.listener(|page, _, window, cx| {
                if window.focused(cx).is_none() { window.focus(&page.focus, cx); }
            }))
            .on_key_down(cx.listener(|page, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if (key.modifiers.control || key.modifiers.platform) && (key.key == "z" || key.key == "y") && window.focused(cx).is_some_and(|focus| focus == page.focus) {
                    page.step_history(key.key == "y" || key.modifiers.shift, cx);
                    cx.stop_propagation();
                } else if key.key == "escape" {
                    if page.canvas.close_menu() { cx.notify(); }
                    else if page.manage_is_open() { page.close_manage(cx); }
                    else if page.note_paper.open.is_some() { page.close_canvas_note(cx); }
                    else if page.escape_run(cx) {}
                    else if page.editor.is_some() {
                        if page.unsaved(cx).is_none() { page.editor = None; page.resolve_wanted(cx); }
                        else { page.state.error = Some("Save or discard the current draft first.".into()); }
                        cx.notify();
                    }
                    cx.stop_propagation();
                }
            }))
            .bg(t.bg)
            .text_color(t.text)
            .child(
                div()
                    .id("missions-list")
                    .debug_selector(|| "missions-rail".into())
                    .w(px(size::LABEL_COL))
                    .flex_none()
                    .overflow_y_scroll()
                    .child(kit::button(&t, "new-mission", "+ Mission", Kind::Quiet).debug_selector(|| "new-mission".into()).on_click(cx.listener(|p, _, window, cx| p.open_mission_create(window, cx))))
                    .child(list),
            )
            .child(
                div()
                    .id("missions-canvas")
                    .flex_1()
                    .min_w_0()
                    .flex().flex_col()
                    .when(authoring, |d| d.child(div().flex().items_center().px(px(PAD)).py(px(TAB_GAP)).gap(px(TAB_GAP)).border_b_1().border_color(t.divider)
                        .child(kit::button(&t, "mission-mode-flow", "Flow", if !self.mission_canvas_mode { Kind::Plain } else { Kind::Quiet }).debug_selector(|| "mission-mode-flow".into()).on_click(cx.listener(|p, _, _, cx| { p.mission_canvas_mode = false; cx.notify(); })))
                        .child(kit::button(&t, "mission-mode-canvas", "Canvas", if self.mission_canvas_mode { Kind::Plain } else { Kind::Quiet }).debug_selector(|| "mission-mode-canvas".into()).on_click(cx.listener(|p, _, _, cx| { p.mission_canvas_mode = true; cx.notify(); })))
                        .child(div().debug_selector({ let complete = self.validation_complete(); move || if complete { "mission-validation-complete".into() } else { "mission-validation-pending".into() } }).child(div().debug_selector({ let ready = self.mission_run_ready(); move || if ready { "mission-run-ready".into() } else { "mission-run-quiet".into() } }).child(kit::button(&t, "prepare-run", "Run", if self.mission_run_ready() { Kind::Primary } else { Kind::Quiet }).debug_selector(|| "prepare-run".into()).on_click(cx.listener(|p, _, _, cx| p.prepare_run(cx))))))
                        .when(self.validation_failed(), |d| d.child(kit::button(&t, "retry-validation", "Retry validation", Kind::Quiet).debug_selector(|| "retry-validation".into()).on_click(cx.listener(|p, _, _, cx| { p.validation.clear(); p.sync_validation(cx); cx.notify(); }))))
                        .child(kit::button(&t, "new-input", "+ Input", Kind::Quiet).debug_selector(|| "new-input".into()).on_click(cx.listener(|p, _, _, cx| p.add_card(board::Kind::Input, cx))))
                        .child(kit::button(&t, "new-step", "+ Step", Kind::Quiet).debug_selector(|| "new-step".into()).on_click(cx.listener(|p, _, _, cx| p.add_card(board::Kind::Formation, cx))))
                        .child(kit::button(&t, "new-gate", "+ Gate", Kind::Quiet).debug_selector(|| "new-gate".into()).on_click(cx.listener(|p, _, _, cx| p.add_card(board::Kind::Gate, cx))))
                        .child(kit::button(&t, "new-end", "+ End", Kind::Quiet).debug_selector(|| "new-end".into()).on_click(cx.listener(|p, _, _, cx| p.add_card(board::Kind::End, cx))))
                        .child(kit::button(&t, "undo", "Undo", Kind::Quiet).debug_selector({ let busy = self.writing; move || if busy { "undo-busy".into() } else { "undo".into() } }).on_click(cx.listener(|p, _, _, cx| p.undo(false, cx))))
                        .child(kit::button(&t, "redo", "Redo", Kind::Quiet).debug_selector({ let busy = self.writing; move || if busy { "redo-busy".into() } else { "redo".into() } }).on_click(cx.listener(|p, _, _, cx| p.undo(true, cx))))))
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
                        .gap(px(TAB_GAP))
                        .border_l_1()
                        .border_color(t.divider)
                        .bg(t.surface)
                        .child(div().flex().items_center().gap(px(TAB_GAP)).pb(px(TAB_GAP))
                            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(match ed.kind {
                                EditKind::Step => "Edit step",
                                EditKind::Input => "Edit input",
                                EditKind::Gate => "Edit gate",
                                EditKind::End => "Edit end",
                            }))
                            .when(!ed.deleting, |d| d.child(left(kit::button(&t, "step-delete", "Delete", Kind::Quiet).debug_selector(|| "step-delete".into()).on_click(cx.listener(|p, _, _, cx| p.delete_card(cx))))))
                            .child(
                                kit::button(&t, "step-save", "Save", Kind::Primary)
                                    .debug_selector(|| "step-save".into())
                                    .on_click(cx.listener(|p, _, _, cx| p.save_step(cx))),
                            ))
                        .when(ed.deleting, |d| d.child(div().child("Delete this card? Its wires go with it; Undo brings both back.").child(left(kit::button(&t, "delete-keep", "Keep", Kind::Quiet).debug_selector(|| "delete-keep".into()).on_click(cx.listener(|p, _, _, cx| { if let Some(ed) = &mut p.editor { ed.deleting = false; } cx.notify(); })))).child(left(kit::button(&t, "delete-yes", "Delete", Kind::Quiet).debug_selector(|| "delete-yes".into()).on_click(cx.listener(|p, _, _, cx| p.delete_card(cx)))))))
                        .when(matches!(ed.kind, EditKind::Step), |d| { let id = ed.id.clone(); d.child(left(kit::button(&t, "run-step", "Run this step", Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| p.prepare_step_run(id.clone(), cx))))) })
                        .when(matches!(ed.kind, EditKind::Step), |mut d| {
                            if let Some((node, slug)) = &self.state.selected && let Some(feed) = self.state.nodes.get(node) {
                                for seat in feed.runs.values().filter(|run| &run.mission_slug == slug).flat_map(crate::store::seats::seats).filter(|seat| seat.step == ed.id && seat.state == crate::store::seats::State::Live) {
                                    d = d.child(self.seat_control(&t, seat, "step", cx));
                                }
                            }
                            d
                        })
                        .child(kit::label(&t, "Title"))
                        .child(
                            div()
                                .debug_selector(|| "step-title".into())
                                .child(self.title.clone()),
                        )
                        .child(kit::label(&t, match ed.kind {
                            EditKind::Step => "Brief",
                            EditKind::Input => "Goal",
                            EditKind::Gate => "Criterion",
                            EditKind::End => "Description",
                        }))
                        .child(
                            div()
                                .debug_selector(|| "step-goal".into())
                                .child(self.goal.clone()),
                        )
                        .when(matches!(ed.kind, EditKind::Step), |d| d.child(kit::label(&t, "Formation type"))
                            .child({ let current = self.state.mission.as_ref().and_then(|m| m.formations.iter().find(|f| f.id == ed.id)).map(|f| f.kind.as_str()); let pick = |id: &'static str, text: &'static str, ty: &'static str| kit::button(&t, id, text, if current == Some(ty) { Kind::Plain } else { Kind::Quiet }).on_click(cx.listener(move |p, _, _, cx| p.set_type(ty, cx))); div().flex().gap(px(TAB_GAP)).child(pick("type-solo", "Solo", "solo")).child(pick("type-peer", "Peer", "peer")).child(pick("type-orchestrated", "Orchestrated", "orchestrated")) })
                            .child(kit::label(&t, "Device")).child(div().flex().gap(px(TAB_GAP)).child(div().flex_1().child(self.extra.clone()))
                            .child(left(kit::button(&t, "set-device", "Set device", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.set_device(cx)))))))
                        .when(matches!(ed.kind, EditKind::Step), |d| d.child(kit::label(&t, "Repository")).child(div().child(self.repository.clone()))
                            .child(kit::label(&t, "Work")).child(div().child(self.work.clone()))
                            .child(kit::label(&t, "Files")).child(div().child(self.files.clone()))
                            .child(kit::label(&t, "Links")).child(div().child(self.links.clone())))
                        .when(!matches!(ed.kind, EditKind::Step), |d| {
                            d.child(kit::label(&t, match ed.kind {
                                EditKind::Input => "Human channel",
                                EditKind::Gate => "Kinds (comma separated)",
                                EditKind::End => "Outcome",
                                _ => "",
                            }))
                            .child(div().debug_selector(|| "step-extra".into()).child(self.extra.clone()))
                        })
                        .when(matches!(ed.kind, EditKind::Step), |mut d| {
                            if let Some(f) = self.state.mission.as_ref().and_then(|m| m.formations.iter().find(|f| f.id == ed.id)) {
                                d = d.child(kit::label(&t, "Ports"));
                                for (dir, port) in f.inputs.iter().map(|p| ("in", p)).chain(f.outputs.iter().map(|p| ("out", p))) {
                                    let id = port.id.clone(); let selected = id.clone(); let label = port.label.clone(); let kind = port.kind.clone(); let selector = format!("port-{id}");
                                    d = d.child(left(kit::button(&t, gpui::SharedString::from(format!("port-{id}")), format!("{dir} · {}", if port.label.trim().is_empty() { &id } else { &port.label }), Kind::Quiet).debug_selector(move || selector.clone()).on_click(cx.listener(move |p, _, _, cx| { p.select_port(&selected, &label, &kind, cx); }))));
                                }
                                d = d.child(div().child(self.port_label.clone())).child(div().child(self.port_kind.clone()))
                                    .child(left(kit::button(&t, "port-add-in", "+ Input port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("input", cx)))))
                                    .child(left(kit::button(&t, "port-add-out", "+ Output port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("output", cx)))))
                                    .when(self.port.is_some(), |d| d.child(left(kit::button(&t, "port-update", "Rename port", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.port_edit("update", cx))))).child(left(kit::button(&t, "port-remove", "Remove port", Kind::Quiet).debug_selector(|| "port-remove".into()).on_click(cx.listener(|p, _, _, cx| p.port_edit("remove", cx))))))
                                    .child(kit::label(&t, "Slots"));
                                for slot in &f.slots {
                                    let id = slot.id.clone(); let selected = id.clone(); let selector = format!("slot-{id}");
                                    d = d.child(left(kit::button(&t, gpui::SharedString::from(format!("slot-{id}")), crate::store::words::slot_label(slot), Kind::Quiet).debug_selector(move || selector.clone()).on_click(cx.listener(move |p, _, _, cx| p.select_slot(&selected, cx)))));
                                }
                            }
                            d
                        })
                        .when(matches!(ed.kind, EditKind::Step) && self.slot.is_some(), |mut d| {
                            d = d.child(kit::label(&t, "Staff selected slot"));
                            d = d.child(self.slot_pickers(&t, cx));
                            d.child(left(kit::button(&t, "assign-slot", "Assign", Kind::Quiet).debug_selector(|| "assign-slot".into()).on_click(cx.listener(|p, _, _, cx| p.assign_slot(cx)))))
                        })
                        .when(matches!(ed.kind, EditKind::Step | EditKind::Input), |mut d| {
                            d = d.child(kit::label(&t, "Limits"));
                            if let Some(mission) = &self.state.mission { for limit in mission.limits.iter().filter(|l| l.target == ed.id) {
                                let id = limit.id.clone(); let selected = id.clone();
                                d = d.child(left(kit::button(&t, gpui::SharedString::from(format!("limit-{id}")), limit.title.clone(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| p.select_limit(&selected, cx)))));
                            } }
                            d = d.child(left(kit::button(&t, "create-limit", "Add limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("create", cx)))));
                            d.when(self.limit.is_some(), |d| d.child(div().child(self.limit_title.clone())).child(div().child(self.limit_rounds.clone())).child(div().child(self.limit_seconds.clone())).child(div().child(self.limit_warn.clone())).child(div().child(self.limit_tokens.clone())).child(div().child(self.limit_spent.clone())).child(left(kit::button(&t, "save-limit", "Save limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("save", cx))))).child(left(kit::button(&t, "delete-limit", "Delete limit", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.limit_action("delete", cx))))) )
                        })
                        .when(matches!(ed.kind, EditKind::Input), |mut d| {
                            d = d.child(kit::label(&t, "Outputs"));
                            for (i, output) in ed.outputs.iter().enumerate() {
                                d = d.child(div().child(output.name.clone())).child(div().child(output.description.clone()))
                                    .child(left(kit::button(&t, ("required", i), if output.required { "Required" } else { "Optional" }, Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { if let Some(e) = &mut p.editor && let Some(o) = e.outputs.get_mut(i) { o.required = !o.required; } cx.notify(); }))))
                                    .child(left(kit::button(&t, ("remove-output", i), "Remove output", Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { if let Some(e) = &mut p.editor && i < e.outputs.len() { e.outputs.remove(i); } cx.notify(); }))))
                                     .child(div().child(output.kind.clone())).child(div().child(output.from.clone()));
                            }
                            d.child(left(kit::button(&t, "add-output", "Add output", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_output(cx)))))
                        })
                        .when(matches!(ed.kind, EditKind::Gate), |d| d.child(kit::label(&t, "Check profile")).child(div().child(self.check.clone())).child(kit::label(&t, "Profile version")).child(div().child(self.check_version.clone())).child(kit::label(&t, "Check value")).child(div().child(self.check_value.clone())).child(kit::label(&t, "Files")).child(div().child(self.files.clone())))
                        .when_some(self.notes.as_ref(), |mut d, notes| {
                            d = d.child(kit::label(&t, "Notes"));
                            if let Some(thread) = notes.elements.iter().find(|n| n.node_id == ed.id) {
                                for note in &thread.entries {
                                    let id = note.id.clone(); let selected = id.clone(); let text = note.text.clone();
                                    if note.author == "human:ui" {
                                        d = d.child(left(kit::button(&t, gpui::SharedString::from(format!("note-{id}")), note.text.clone(), Kind::Quiet).on_click(cx.listener(move |p, _, _, cx| { p.select_note(&selected, &text, cx); }))));
                                    } else { d = d.child(div().child(note.text.clone())); }
                                }
                            }
                            d.child(div().child(self.note.clone()))
                                .child(left(kit::button(&t, "add-note", if self.note_id.is_some() { "Save note" } else { "Add note" }, Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_note(false, cx)))))
                                .when(self.note_id.is_some() || self.note.read(cx).text != self.note_base, |d| d.child(left(kit::button(&t, "note-cancel", "Discard note draft", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.discard_note(cx))))))
                                .when(self.note_id.is_some(), |d| d.child(left(kit::button(&t, "delete-note", "Delete note", Kind::Quiet).on_click(cx.listener(|p, _, _, cx| p.add_note(true, cx))))))
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
                                    if !p.editor.as_ref().is_some_and(|e| e.busy) && !p.note_saving && p.note.read(cx).text == p.note_base { p.note_id = None; p.note_base.clear(); p.note.update(cx, |f, cx| f.set("", false, cx)); p.editor = None; p.resolve_wanted(cx); }
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
            .children(start_form)
            .children(form)
            .children(attempts_form)
            .children(stop_form)
    }
}

/// A control held at its own width on the left of the step editor.
fn left(control: impl IntoElement) -> Div {
    div().flex().child(control)
}

#[cfg(test)]
mod tests {
    #[test]
    fn saving_staffing_keeps_other_and_later_drafts_unsaved() {
        let mut baseline = super::EditorSnapshot {
            fields: vec![String::new(); 22],
            outputs: vec![],
        };
        let mut submitted = baseline.clone();
        submitted.fields[1] = "unsaved brief".into();
        submitted.fields[11] = "saved harness".into();
        let mut now = submitted.clone();
        now.fields[11] = "newer harness".into();
        baseline.accept_fields(&submitted, 10..14);
        assert_eq!(baseline.fields[11], "saved harness");
        assert_ne!(baseline.fields[11], now.fields[11]);
        assert_ne!(baseline.fields[1], now.fields[1]);
    }
    #[test]
    fn a_mission_path_encodes_its_slug() {
        assert_eq!(super::mission_path("daily"), "/api/missions/daily");
        assert_eq!(
            super::mission_path("my plan #2?"),
            "/api/missions/my%20plan%20%232%3F"
        );
    }
}
