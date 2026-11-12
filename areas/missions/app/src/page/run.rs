//! Run controls and views; the channel projection remains in the store.
use super::*;
use crate::store::run_state;
use gpui::{FocusHandle, KeyDownEvent, SharedString, Subscription};
use parvi_app_kit::{ask::Ask, theme::Theme};
use parvi_missions_contract::{
    AdmissionRejection, AttemptsResponse, Mission, RunId, RunProjection, RunStart,
};
use std::collections::BTreeMap;

pub(super) struct RunUi {
    selected: Option<String>,
    reveal_step: Option<String>,
    frozen: Option<Mission>,
    starting: bool,
    start_keys_ready: bool,
    start_subs: Vec<Subscription>,
    target: Option<String>,
    start_card: Option<String>,
    start_definition: Option<(Mission, String)>,
    request_id: String,
    uncertain: Option<(serde_json::Value, String)>,
    cwd: Entity<TextField>,
    repository: Entity<TextField>,
    base: Entity<TextField>,
    work: Entity<TextField>,
    paths: Entity<TextField>,
    busy: bool,
    abort_armed: bool,
    stop_reason: Entity<TextField>,
    events_open: bool,
    canvas_mode: bool,
    fields: BTreeMap<String, Entity<TextField>>,
    responses: BTreeMap<String, Entity<TextField>>,
    pub error: Option<String>,
    pub findings: Vec<String>,
    attempts: Option<AttemptsResponse>,
    attempt_window: Option<crate::store::attempts::AttemptsWindow>,
    attempts_focus: FocusHandle,
    attempt_reason: Entity<TextField>,
    attempts_at: Option<usize>,
    pending_attempt: Option<(String, bool)>,
    evidence: crate::store::evidence::Store,
    gates: BTreeMap<
        String,
        (
            u64,
            Result<Option<parvi_missions_contract::GateRequest>, String>,
        ),
    >,
}
impl RunUi {
    pub fn new(cx: &mut Context<MissionsPage>) -> Self {
        Self {
            selected: None,
            reveal_step: None,
            frozen: None,
            starting: false,
            start_keys_ready: false,
            start_subs: Vec::new(),
            target: None,
            start_card: None,
            start_definition: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            uncertain: None,
            cwd: cx.new(|cx| TextField::new(cx, "Working directory")),
            repository: cx.new(|cx| TextField::new(cx, "Repository")),
            base: cx.new(|cx| TextField::new(cx, "Base")),
            work: cx.new(|cx| TextField::new(cx, "Work reference")),
            paths: cx.new(|cx| TextField::new(cx, "Context paths, one per line").prose()),
            busy: false,
            abort_armed: false,
            stop_reason: cx.new(|cx| TextField::new(cx, "Why (optional)")),
            events_open: false,
            canvas_mode: false,
            fields: BTreeMap::new(),
            responses: BTreeMap::new(),
            error: None,
            findings: vec![],
            attempts: None,
            attempt_window: None,
            attempts_focus: cx.focus_handle(),
            attempt_reason: cx.new(|cx| TextField::new(cx, "Why this attempt?")),
            attempts_at: None,
            pending_attempt: None,
            evidence: Default::default(),
            gates: BTreeMap::new(),
        }
    }
}
impl MissionsPage {
    pub(super) fn run_unsaved(&self, cx: &gpui::App) -> Option<String> {
        if self.run.uncertain.is_some() {
            return Some("Unconfirmed run start".into());
        }
        if self.run.starting
            && (self
                .run
                .fields
                .values()
                .any(|f| !f.read(cx).text.is_empty())
                || [
                    &self.run.cwd,
                    &self.run.repository,
                    &self.run.base,
                    &self.run.work,
                    &self.run.paths,
                ]
                .iter()
                .any(|f| !f.read(cx).text.is_empty()))
        {
            return Some("Run input".into());
        }
        if self
            .run
            .responses
            .values()
            .any(|f| !f.read(cx).text.is_empty())
        {
            return Some("Gate response".into());
        }
        None
    }
    pub(super) fn reveal_run(&mut self, id: String, part: Option<String>, cx: &mut Context<Self>) {
        self.show_run(id, cx);
        self.run.reveal_step = part;
        cx.notify();
    }
    pub(super) fn start_uncertain(&self) -> bool {
        self.run.uncertain.is_some()
    }
    pub(super) fn is_showing_run(&self) -> bool {
        self.run.selected.is_some() || self.run.starting
    }
    pub(super) fn selected_run(&self) -> Option<&RunProjection> {
        let (node, _) = self.state.selected.as_ref()?;
        self.state
            .nodes
            .get(node)?
            .runs
            .get(self.run.selected.as_ref()?)
    }
    pub(super) fn show_run(&mut self, run: String, cx: &mut Context<Self>) {
        if self.run_unsaved(cx).is_some() {
            self.run.error =
                Some("Save or send the current run draft before opening another run.".into());
            cx.notify();
            return;
        }
        let Some((node, _)) = self.state.selected.clone() else {
            return;
        };
        self.canvas.invalidate_fit();
        self.run.selected = Some(run.clone());
        self.run.reveal_step = None;
        self.run.abort_armed = false;
        self.run.events_open = false;
        self.run.canvas_mode = false;
        self.run.starting = false;
        self.run.frozen = None;
        self.run.attempts = None;
        self.run.attempt_window = None;
        self.run.attempts_at = None;
        self.run.pending_attempt = None;
        self.run.evidence = Default::default();
        self.run.gates.clear();
        self.run.responses.clear();
        self.run.error = None;
        let req = Request::get(NodeId(node), format!("/api/runs/{}/mission", segment(&run)));
        let asked = self.host.request(req, cx);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.run.selected.as_deref() != Some(&run) {
                    return;
                }
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<MissionResponse>(&a.body) {
                        Ok(m) => page.run.frozen = Some(m.mission.mission),
                        Err(e) => page.run.error = Some(e.to_string()),
                    },
                    Ok(a) => page.run.error = Some(a.error_text()),
                    Err(e) => page.run.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
        self.sync_gate(cx);
        cx.notify();
    }
    pub(super) fn sync_gate(&mut self, cx: &mut Context<Self>) {
        let (Some((node, _)), Some(run_id), Some(run)) = (
            self.state.selected.clone(),
            self.run.selected.clone(),
            self.selected_run().cloned(),
        ) else {
            return;
        };
        self.run
            .responses
            .retain(|id, _| run.waiting_gates.iter().any(|g| g.gate_id == *id));
        for gate in &run.waiting_gates {
            if self
                .run
                .gates
                .get(&gate.gate_id)
                .is_none_or(|(seq, _)| *seq != gate.requested_seq)
            {
                self.run.responses.insert(
                    gate.gate_id.clone(),
                    cx.new(|cx| TextField::new(cx, "Your response").prose()),
                );
            }
        }
        self.run.gates.retain(|id, (seq, _)| {
            run.waiting_gates
                .iter()
                .any(|g| g.gate_id == *id && g.requested_seq == *seq)
        });
        if self
            .run
            .evidence
            .read_at
            .is_none_or(|at| at < run.event_count)
        {
            self.read_evidence(&node, &run_id, run.event_count, cx);
        }
        if self.run.attempts_at.is_some_and(|at| at < run.event_count) {
            self.attempts(cx);
        }
        for gate in run.waiting_gates {
            if self
                .run
                .gates
                .get(&gate.gate_id)
                .is_some_and(|(seq, _)| *seq == gate.requested_seq)
            {
                continue;
            }
            let id = gate.gate_id.clone();
            let seq = gate.requested_seq;
            self.run.gates.insert(id.clone(), (seq, Ok(None)));
            let path = format!(
                "/api/runs/{}/gates/{}/request",
                segment(&run_id),
                segment(&id)
            );
            let asked = self
                .host
                .request(Request::get(NodeId(node.clone()), path), cx);
            let selected = run_id.clone();
            cx.spawn(async move |this, cx| {
                let result = asked.await;
                let _ = this.update(cx, |page, cx| {
                    if page.run.selected.as_deref() != Some(&selected)
                        || page
                            .run
                            .gates
                            .get(&id)
                            .is_none_or(|(current, _)| *current != seq)
                    {
                        return;
                    }
                    match result {
                        Ok(a) if a.ok() => match serde_json::from_slice::<
                            parvi_missions_contract::GateRequestResponse,
                        >(&a.body)
                        {
                            Ok(v) if v.request.requested_seq == seq => {
                                page.run.gates.insert(id, (seq, Ok(Some(v.request))));
                            }
                            Ok(_) => {
                                page.run.gates.insert(
                                    id,
                                    (seq, Err("Gate request changed. Try again.".into())),
                                );
                            }
                            Err(e) => {
                                page.run.gates.insert(id, (seq, Err(e.to_string())));
                            }
                        },
                        Ok(a) => {
                            page.run.gates.insert(id, (seq, Err(a.error_text())));
                        }
                        Err(e) => {
                            page.run.gates.insert(id, (seq, Err(e.message)));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }
    fn clear_start_context(&mut self, cx: &mut Context<Self>) {
        for field in [
            &self.run.cwd,
            &self.run.repository,
            &self.run.base,
            &self.run.work,
            &self.run.paths,
        ] {
            field.update(cx, |field, cx| field.set("", false, cx));
        }
    }
    pub(super) fn prepare_run(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self
            .state
            .mission
            .as_ref()
            .and_then(|m| m.input_cards.first())
            .map(|c| c.id.clone())
        else {
            self.state.error = Some("This mission has no input card.".into());
            cx.notify();
            return;
        };
        self.prepare_card_run(&card, cx);
    }
    /// Start at the Input card whose play control was used.
    pub(super) fn prepare_card_run(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.unsaved(cx).is_some() {
            self.state.error = Some("Save or discard the current draft first.".into());
            cx.notify();
            return;
        }
        let Some(input) = self
            .state
            .mission
            .as_ref()
            .and_then(|m| m.input_cards.iter().find(|c| c.id == id))
        else {
            return;
        };
        let fields: Vec<(String, bool)> = if input.inputs.is_empty() {
            vec![("brief".into(), true)]
        } else {
            input
                .inputs
                .iter()
                .map(|i| (i.name.clone(), i.kind.is_empty() || i.kind == "text"))
                .collect()
        };
        let Some(definition) = self
            .state
            .mission
            .clone()
            .zip(self.state.mission_etag.clone())
        else {
            return;
        };
        self.clear_start_context(cx);
        self.run.start_definition = Some(definition);
        self.run.target = None;
        self.run.start_card = Some(id.to_owned());
        self.run.request_id = uuid::Uuid::new_v4().to_string();
        self.run.uncertain = None;
        self.run.fields = fields
            .into_iter()
            .map(|(name, prose)| {
                let field = cx.new(|cx| {
                    let field = TextField::new(cx, &name);
                    if prose { field.prose() } else { field }
                });
                (name, field)
            })
            .collect();
        self.run.selected = None;
        self.run.findings.clear();
        self.run.error = None;
        self.run.start_keys_ready = false;
        self.run.starting = true;
        cx.notify();
    }
    pub(super) fn prepare_step_run(&mut self, id: String, cx: &mut Context<Self>) {
        if self.start_uncertain() {
            return;
        }
        if self.unsaved(cx).is_some() {
            self.state.error = Some("Save or discard the current draft first.".into());
            cx.notify();
            return;
        }
        let Some(definition) = self
            .state
            .mission
            .clone()
            .zip(self.state.mission_etag.clone())
        else {
            return;
        };
        self.clear_start_context(cx);
        self.run.start_definition = Some(definition);
        self.run.target = Some(id);
        self.run.start_card = None;
        self.run.request_id = uuid::Uuid::new_v4().to_string();
        self.run.uncertain = None;
        self.run.fields = [(
            "brief".to_string(),
            cx.new(|cx| TextField::new(cx, "Brief").prose()),
        )]
        .into_iter()
        .collect();
        self.run.selected = None;
        self.run.findings.clear();
        self.run.error = None;
        self.run.start_keys_ready = false;
        self.run.starting = true;
        self.editor = None;
        cx.notify();
    }
    pub(super) fn setup_start_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.run.starting || self.run.start_keys_ready {
            return;
        }
        self.run.start_keys_ready = true;
        self.run.start_subs.clear();
        let mut order: Vec<Entity<TextField>> = self
            .state
            .mission
            .as_ref()
            .and_then(|mission| {
                mission
                    .input_cards
                    .iter()
                    .find(|card| self.run.start_card.as_deref() == Some(card.id.as_str()))
            })
            .map(|card| {
                card.inputs
                    .iter()
                    .filter_map(|input| self.run.fields.get(&input.name).cloned())
                    .collect()
            })
            .unwrap_or_default();
        if order.is_empty() {
            order.extend(self.run.fields.values().cloned());
        }
        order.extend([
            self.run.work.clone(),
            self.run.cwd.clone(),
            self.run.repository.clone(),
            self.run.base.clone(),
            self.run.paths.clone(),
        ]);
        for (index, field) in order.iter().enumerate() {
            let order = order.clone();
            self.run.start_subs.push(cx.subscribe_in(
                field,
                window,
                move |page, _, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Submit | FieldEvent::SubmitAlt => page.start_run(cx),
                    FieldEvent::Tab => parvi_app_kit::widgets::field::focus(
                        &order[(index + 1) % order.len()],
                        window,
                        cx,
                    ),
                    _ => {}
                },
            ));
        }
        if let Some(first) = order.first() {
            parvi_app_kit::widgets::field::focus(first, window, cx);
        }
    }
    pub(super) fn escape_run(&mut self, cx: &mut Context<Self>) -> bool {
        if self.run.attempt_window.take().is_some() {
            cx.notify();
            return true;
        }
        if self.run.starting && !self.run.busy && self.run.uncertain.is_none() {
            self.run.starting = false;
            self.run.fields.clear();
            self.run.error = None;
            self.run.findings.clear();
            cx.notify();
            return true;
        }
        if self.run.abort_armed {
            self.run.abort_armed = false;
            cx.notify();
            return true;
        }
        if self.run.events_open {
            self.run.events_open = false;
            cx.notify();
            return true;
        }
        false
    }
    pub(super) fn start_run(&mut self, cx: &mut Context<Self>) {
        if self.run.busy {
            return;
        }
        let (Some((node, slug)), Some((m, etag))) = (
            self.state.selected.clone(),
            self.run.start_definition.as_ref(),
        ) else {
            return;
        };
        if self.run.uncertain.is_none()
            && self
                .state
                .mission
                .as_ref()
                .is_none_or(|current| current.rev != m.rev)
        {
            self.run.findings = vec!["The mission changed since this dialog opened. Cancel and start again to see what would run.".into()];
            cx.notify();
            return;
        }
        let etag = etag.clone();
        let target = self.run.target.clone();
        let input_card_id = if target.is_none() {
            self.run.start_card.clone().unwrap_or_default()
        } else {
            String::new()
        };
        // An uncertain start must retry its exact body. For a fresh start,
        // tell the operator about missing required inputs before sending it.
        let input = m.input_cards.iter().find(|card| card.id == input_card_id);
        if self.run.uncertain.is_none()
            && target.is_none()
            && let Some(card) = input
        {
            let required: Vec<&str> = if card.inputs.is_empty() {
                vec!["brief"]
            } else {
                card.inputs
                    .iter()
                    .filter(|i| i.required)
                    .map(|i| i.name.as_str())
                    .collect()
            };
            let missing: Vec<String> = required
                .into_iter()
                .filter(|name| {
                    self.run
                        .fields
                        .get(*name)
                        .is_none_or(|f| f.read(cx).text.trim().is_empty())
                })
                .map(|name| format!("{name} is required."))
                .collect();
            if !missing.is_empty() {
                self.run.findings = missing;
                self.run.error = None;
                cx.notify();
                return;
            }
        }
        let values = self
            .run
            .fields
            .iter()
            .filter_map(|(name, field)| {
                let value = &field.read(cx).text;
                if value.trim().is_empty() {
                    return None;
                }
                let kind = input
                    .and_then(|card| card.inputs.iter().find(|i| i.name == *name))
                    .map(|i| i.kind.as_str())
                    .unwrap_or("text");
                Some((
                    name.clone(),
                    if matches!(kind, "file" | "folder") {
                        value.trim().to_owned()
                    } else {
                        value.clone()
                    },
                ))
            })
            .collect();
        let start = RunStart {
            mission: slug,
            input_card_id,
            formation_id: target.unwrap_or_default(),
            expected_rev: m.rev,
            inputs: values,
            actor: "human:operator".into(),
            request_id: self.run.request_id.clone(),
            cwd: self.run.cwd.read(cx).text.clone(),
            repository: self.run.repository.read(cx).text.clone(),
            base: self.run.base.read(cx).text.clone(),
            work: self.run.work.read(cx).text.clone(),
            context_paths: self
                .run
                .paths
                .read(cx)
                .text
                .lines()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        };
        let (body, etag) = self
            .run
            .uncertain
            .clone()
            .unwrap_or_else(|| (serde_json::to_value(start).expect("start serializes"), etag));
        self.run.uncertain = Some((body.clone(), etag.clone()));
        let req = Request::new("POST", NodeId(node), "/api/runs")
            .header("If-Match", etag)
            .within(std::time::Duration::from_secs(60))
            .json(&body);
        self.run.busy = true;
        self.run.error = None;
        self.run.findings.clear();
        let asked = self.host.request(req, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                page.run.busy = false;
                match start_answer(result) {
                    StartAnswer::Started(id) => {
                        page.run.uncertain = None;
                        page.run.starting = false;
                        page.show_run(id, cx);
                    }
                    StartAnswer::Refused(a) => {
                        page.run.uncertain = None;
                        match serde_json::from_slice::<AdmissionRejection>(&a.body) {
                            Ok(rejection) => {
                                page.run.error = Some(format!(
                                    "{}: {}",
                                    rejection.error.code, rejection.error.message
                                ));
                                page.run.findings = rejection
                                    .findings
                                    .into_iter()
                                    .map(|f| {
                                        format!(
                                            "{} · {} · {}",
                                            f.code.as_str(),
                                            f.node_id,
                                            f.message
                                        )
                                    })
                                    .collect();
                            }
                            Err(_) => page.run.error = Some(a.error_text()),
                        }
                    }
                    StartAnswer::Unknown(reason) => {
                        page.run.error = Some(format!(
                            "Start outcome unknown: {reason}. Retry the same request."
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn run_post(&mut self, suffix: String, body: serde_json::Value, cx: &mut Context<Self>) {
        if self.run.busy {
            return;
        }
        let (Some((node, _)), Some(run)) = (self.state.selected.clone(), self.run.selected.clone())
        else {
            return;
        };
        let req = Request::new(
            "POST",
            NodeId(node),
            format!("/api/runs/{}{}", segment(&run), suffix),
        )
        .json(&body);
        self.run.busy = true;
        self.run.error = None;
        let asked = self.host.request(req, cx);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.run.selected.as_deref() != Some(&run) {
                    return;
                }
                page.run.busy = false;
                match result {
                    Ok(a) if a.ok() => {}
                    Ok(a) => page.run.error = Some(a.error_text()),
                    Err(e) => page.run.error = Some(format!("{}: {}", e.code, e.message)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn verdict(&mut self, gate: &str, seq: u64, verdict: &str, cx: &mut Context<Self>) {
        let body = json!({"requestedSeq":seq, "verdict":verdict, "reason":self.run.responses.get(gate).map(|f| f.read(cx).text.clone()).unwrap_or_default(), "actor":"human:operator"});
        self.run_post(format!("/gates/{}/verdict", segment(gate)), body, cx);
    }
    fn pick(&mut self, gate: &str, seq: u64, attempt: i64, cx: &mut Context<Self>) {
        let body = json!({"requestedSeq":seq, "attempt":attempt, "reason":self.run.responses.get(gate).map(|f| f.read(cx).text.clone()).unwrap_or_default(), "actor":"human:operator"});
        self.run_post(format!("/gates/{}/pick", segment(gate)), body, cx);
    }
    fn read_evidence(&mut self, node: &str, run: &str, at: usize, cx: &mut Context<Self>) {
        self.run.evidence.read_at = Some(at);
        let node = node.to_owned();
        let run = run.to_owned();
        let asked = self.host.request(
            Request::get(
                NodeId(node.clone()),
                format!("/api/runs/{}/evidence", segment(&run)),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.run.selected.as_deref() != Some(&run)
                    || page.state.selected.as_ref().is_none_or(|(n, _)| n != &node)
                    || page.run.evidence.read_at != Some(at)
                {
                    return;
                }
                match result {
                    Ok(answer) if answer.ok() => match serde_json::from_slice::<
                        crate::store::evidence::Response,
                    >(&answer.body)
                    {
                        Ok(response) => {
                            page.run.evidence.evidence = Some(response.evidence);
                            page.run.evidence.error = None;
                        }
                        Err(error) => page.run.evidence.error = Some(error.to_string()),
                    },
                    Ok(answer) => page.run.evidence.error = Some(answer.error_text()),
                    Err(error) => page.run.evidence.error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// Read image bytes through Files on the node that owns the run.
    fn load_attempt_images(&mut self, cx: &mut Context<Self>) {
        let Some(window) = self.run.attempt_window.as_ref() else {
            return;
        };
        let node = window.node.clone();
        let run = window.run.clone();
        let paths: Vec<_> = window
            .attempts()
            .iter()
            .flat_map(|a| &a.outputs)
            .filter_map(|o| {
                crate::store::evidence::image_format(&o.path).map(|format| (o.path.clone(), format))
            })
            .collect();
        for (path, format) in paths {
            if self.run.evidence.images.contains_key(&path) {
                continue;
            }
            self.run
                .evidence
                .images
                .insert(path.clone(), crate::store::evidence::Image::Loading);
            let asked = self.host.request(
                Request::get(
                    NodeId(node.clone()),
                    format!("/api/files/file?path={}", segment(&path)),
                ),
                cx,
            );
            let svg = cx.svg_renderer();
            let selected_node = node.clone();
            let selected_run = run.clone();
            cx.spawn(async move |this, cx| {
                let result = asked.await;
                let image = match result {
                    Ok(answer) if answer.ok() => {
                        let task = cx.background_executor().spawn(async move {
                            gpui::Image::from_bytes(format, answer.body).to_image_data(svg)
                        });
                        match task.await {
                            Ok(image) => crate::store::evidence::Image::Ready(image),
                            Err(error) => crate::store::evidence::Image::Failed(error.to_string()),
                        }
                    }
                    Ok(answer) => crate::store::evidence::Image::Failed(answer.error_text()),
                    Err(error) => crate::store::evidence::Image::Failed(error.message),
                };
                let _ = this.update(cx, |page, cx| {
                    if page.run.selected.as_deref() != Some(&selected_run)
                        || page
                            .state
                            .selected
                            .as_ref()
                            .is_none_or(|(n, _)| n != &selected_node)
                    {
                        return;
                    }
                    page.run.evidence.images.insert(path, image);
                    cx.notify();
                });
            })
            .detach();
        }
    }
    fn attempts(&mut self, cx: &mut Context<Self>) {
        let (Some((node, _)), Some(run)) = (self.state.selected.clone(), self.run.selected.clone())
        else {
            return;
        };
        let at = self
            .selected_run()
            .map(|r| r.event_count)
            .unwrap_or_default();
        self.run.attempts_at = Some(at);
        let asked = self.host.request(
            Request::get(
                NodeId(node),
                format!("/api/runs/{}/evidence/attempts", segment(&run)),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.run.selected.as_deref() != Some(&run) || page.run.attempts_at != Some(at) {
                    return;
                }
                match result {
                    Ok(a) if a.ok() => match serde_json::from_slice::<AttemptsResponse>(&a.body) {
                        Ok(v) => {
                            if let Some(window) = &mut page.run.attempt_window
                                && let Some(step) =
                                    v.attempts.steps.iter().find(|s| s.node_id == window.step)
                            {
                                window.read(step.clone(), None);
                                window.read_at = at;
                            }
                            page.run.attempts = Some(v);
                            if let Some((step_id, compare)) = page.run.pending_attempt.take()
                                && let (Some(data), Some((node, _))) =
                                    (&page.run.attempts, &page.state.selected)
                                && let Some(step) = data
                                    .attempts
                                    .steps
                                    .iter()
                                    .find(|s| s.node_id == step_id)
                                    .cloned()
                            {
                                let mut window = crate::store::attempts::AttemptsWindow::new(
                                    node,
                                    &run,
                                    &step_id,
                                    &step.title,
                                    at,
                                );
                                window.read(step, None);
                                if compare {
                                    window.toggle_compare();
                                }
                                page.run.attempt_window = Some(window);
                            }
                            page.load_attempt_images(cx);
                        }
                        Err(e) => page.run.error = Some(format!("{e}")),
                    },
                    Ok(a) => page.run.error = Some(a.error_text()),
                    Err(e) => page.run.error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn show_attempt_step(&mut self, step_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(data), Some(run), Some((node, _))) =
            (&self.run.attempts, &self.run.selected, &self.state.selected)
        else {
            return;
        };
        let Some(step) = data
            .attempts
            .steps
            .iter()
            .find(|s| s.node_id == step_id)
            .cloned()
        else {
            return;
        };
        let mut attempts =
            crate::store::attempts::AttemptsWindow::new(node, run, step_id, &step.title, 0);
        attempts.read(step, None);
        self.run.attempt_window = Some(attempts);
        window.focus(&self.run.attempts_focus, cx);
        self.load_attempt_images(cx);
        cx.notify();
    }
    /// A running canvas card opens the selected step's attempts, as the
    /// Flow and gate views do; retain the run/node identity across the read.
    pub(super) fn open_canvas_attempts(
        &mut self,
        step: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run.pending_attempt = Some((step.to_owned(), false));
        window.focus(&self.run.attempts_focus, cx);
        self.attempts(cx);
    }
    /// The old attempts window's shortcuts move the focused attempt or the
    /// other comparison side; text-entry and modified shortcuts stay theirs.
    fn attempts_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.run.attempt_window.as_mut() else {
            return;
        };
        let key = &event.keystroke;
        if key.modifiers.control || key.modifiers.alt || key.modifiers.platform {
            return;
        }
        let numbers = open.numbers();
        match key.key.as_str() {
            "left" | "right" => {
                let delta = if key.key == "left" { -1 } else { 1 };
                if key.modifiers.shift && open.compare.is_none() {
                    open.toggle_compare();
                }
                if key.modifiers.shift && open.compare.is_some() {
                    open.side = 1 - open.side;
                }
                open.step(delta);
                if key.modifiers.shift && open.compare.is_some() {
                    open.side = 1 - open.side;
                }
            }
            "home" => {
                if let Some(n) = numbers.first() {
                    open.choose(*n);
                }
            }
            "end" => {
                if let Some(n) = numbers.last() {
                    open.choose(*n);
                }
            }
            "c" if numbers.len() > 1 => open.toggle_compare(),
            "escape" => {
                self.run.attempt_window = None;
                cx.stop_propagation();
                cx.notify();
                return;
            }
            digit if digit.len() == 1 && digit.bytes().all(|c| c.is_ascii_digit()) => {
                let n = digit.parse::<i64>().unwrap_or(0);
                if !numbers.contains(&n) {
                    return;
                }
                open.choose(n);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn seat_control(
        &self,
        t: &Theme,
        seat: crate::store::seats::Seat,
        scope: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::store::seats::State;
        let state = match seat.state {
            State::Live => "live",
            State::Ended => "ended",
            State::Unavailable => "unavailable",
        };
        let selector = format!(
            "{scope}-seat-{}-{}-{state}-{}",
            seat.run, seat.seq, seat.name
        );
        let text = match seat.state {
            State::Live => format!("Open terminal · {}", seat.name),
            State::Ended => format!("Seat ended · {}", seat.name),
            State::Unavailable => format!("Seat unavailable · {}", seat.name),
        };
        let button = kit::button(t, SharedString::from(selector.clone()), text, Kind::Quiet)
            .debug_selector(move || selector.clone());
        let button = if seat.state == State::Live {
            button.on_click(cx.listener(move |page, _, _, cx| {
                if let Err(error) = page.host.ask(
                    Ask::Reveal {
                        what: "session".into(),
                        id: seat.session.clone(),
                        node: Some(NodeId(seat.node.clone())),
                        part: None,
                        beside: false,
                    },
                    cx,
                ) {
                    page.state.error = Some(error.message);
                    cx.notify();
                }
            }))
        } else {
            button.cursor_default().text_color(t.dim)
        };
        div().child(button)
    }
    pub(super) fn run_start_form(
        &self,
        t: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.run.starting {
            let mut form = div()
                .p(px(PAD))
                .flex()
                .flex_col()
                .gap(px(PAD))
                .child("Start mission");
            let declared = self.run.start_definition.as_ref().and_then(|(mission, _)| {
                mission
                    .input_cards
                    .iter()
                    .find(|card| self.run.start_card.as_deref() == Some(card.id.as_str()))
            });
            let names: Vec<&str> = match declared {
                Some(card) if self.run.target.is_none() && !card.inputs.is_empty() => card
                    .inputs
                    .iter()
                    .map(|input| input.name.as_str())
                    .collect(),
                _ => self.run.fields.keys().map(String::as_str).collect(),
            };
            for name in names {
                let Some(field) = self.run.fields.get(name) else {
                    continue;
                };
                let input =
                    declared.and_then(|card| card.inputs.iter().find(|input| input.name == name));
                let required = input.is_some_and(|input| input.required)
                    || (name == "brief" && input.is_none());
                let description = input
                    .map(|input| input.description.as_str())
                    .or_else(|| declared.map(|card| card.input_hint.as_str()))
                    .unwrap_or("");
                let kind = input
                    .map(|input| input.kind.as_str())
                    .filter(|kind| !kind.is_empty())
                    .unwrap_or("text");
                let selector = format!("run-input-{name}");
                form = form
                    .child(format!(
                        "{name}{} · {kind}",
                        if required { " (required)" } else { "" }
                    ))
                    .child(
                        div()
                            .debug_selector(move || selector.clone())
                            .child(field.clone()),
                    );
                if !description.is_empty() {
                    form = form.child(div().text_color(t.dim).child(description.to_owned()));
                }
            }
            form = form
                .child("Working directory")
                .child(div().child(self.run.cwd.clone()))
                .child("Repository")
                .child(div().child(self.run.repository.clone()))
                .child("Base")
                .child(div().child(self.run.base.clone()))
                .child("Work reference")
                .child(div().child(self.run.work.clone()))
                .child("Context paths")
                .child(
                    div()
                        .debug_selector(|| "run-context-paths".into())
                        .child(self.run.paths.clone()),
                );
            if self.run.uncertain.is_some() && !self.run.busy {
                form = form.child("Retry sends the same start request and its original values.");
            }
            form = form.when_some(self.run.error.clone(), |d, e| {
                let message = self
                    .run
                    .start_definition
                    .as_ref()
                    .map(|(mission, _)| crate::store::validation::readable_message(mission, &e))
                    .unwrap_or(e);
                d.child(div().text_color(t.error).child(message))
            });
            for (i, finding) in self.run.findings.iter().enumerate() {
                let selector = if finding.ends_with(" is required.") {
                    format!("run-required-{}", finding.trim_end_matches(" is required."))
                } else {
                    format!("run-finding-{i}-{finding}")
                };
                let display = self
                    .run
                    .start_definition
                    .as_ref()
                    .map(|(mission, _)| {
                        crate::store::validation::readable_message(mission, finding)
                    })
                    .unwrap_or_else(|| finding.clone());
                form = form.child(
                    div()
                        .debug_selector(move || selector.clone())
                        .text_color(t.error)
                        .child(display),
                );
            }
            let mut actions = div()
                .p(px(PAD))
                .flex_none()
                .flex()
                .justify_end()
                .gap(px(PAD))
                .debug_selector(|| "run-start-actions".into());
            if !self.run.busy && self.run.uncertain.is_none() {
                actions = actions.child(
                    kit::button(t, "run-cancel", "Cancel", Kind::Quiet)
                        .debug_selector(|| "run-cancel".into())
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.run.starting = false;
                            page.run.fields.clear();
                            page.run.error = None;
                            page.run.findings.clear();
                            cx.notify();
                        })),
                );
            }
            actions = actions.child(
                kit::button(t, "run-start", "Start run", Kind::Primary)
                    .debug_selector(|| "run-start".into())
                    .on_click(cx.listener(|p, _, _, cx| p.start_run(cx))),
            );
            let available = (f32::from(window.viewport_size().height)
                - size::TAB_BAR * 2.
                - size::STATUS
                - PAD)
                .clamp(size::CONTROL * 3., START_FORM_MAX);
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(parvi_app_kit::theme::alpha(t.bg, 0.3))
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(size::TAB_BAR))
                    .child(
                        kit::panel(t)
                            .id("start-dialog")
                            .debug_selector(|| "start-dialog".into())
                            .w(px(size::NOTICE))
                            .max_h(px(available))
                            .flex()
                            .flex_col()
                            .child(form.id("start-fields").min_h_0().overflow_y_scroll())
                            .child(actions),
                    ),
            );
        }
        None
    }
    pub(super) fn stop_form(&self, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        if !self.run.abort_armed {
            return None;
        }
        let run = self.selected_run()?;
        if run.r#final {
            return None;
        }
        let mission = self.run.frozen.as_ref().or(self.state.mission.as_ref());
        let mut body = kit::panel(t)
            .id("stop-run")
            .debug_selector(|| "stop-run".into())
            .w(px(size::NOTICE))
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(PAD))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(format!(
                "Stop {}?",
                mission
                    .map(|m| m.title.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&run.mission_slug)
            ));
        for point in run_state::points(run) {
            if point.kind == run_state::PointKind::Running && !point.node.is_empty() {
                let title = mission
                    .map(|mission| mission.node_name(&point.node))
                    .unwrap_or(&point.node);
                body = body.child(format!("The agents working on {title} are interrupted."));
            }
        }
        for gate in &run.waiting_gates {
            let title = mission
                .map(|mission| mission.node_name(&gate.gate_id))
                .unwrap_or(&gate.gate_id);
            body = body.child(format!("{title} stops waiting for you."));
        }
        body = body
            .child(
                "The run ends as canceled and cannot be resumed. Its workspace and outputs stay.",
            )
            .child("Why (optional)")
            .child(
                div()
                    .debug_selector(|| "stop-reason".into())
                    .child(self.run.stop_reason.clone()),
            )
            .child(
                kit::button(t, "stop-keep", "Keep running", Kind::Plain)
                    .debug_selector(|| "stop-keep".into())
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.run.abort_armed = false;
                        cx.notify();
                    })),
            )
            .child(
                kit::button(t, "stop-confirm", "Stop run", Kind::Primary)
                    .debug_selector(|| "stop-confirm".into())
                    .on_click(cx.listener(|page, _, _, cx| {
                        let reason = page.run.stop_reason.read(cx).text.trim().to_owned();
                        page.run.abort_armed = false;
                        page.run_post(
                            "/abort".into(),
                            json!({"requestedBy":"human:operator", "reason":reason}),
                            cx,
                        );
                    })),
            );
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(parvi_app_kit::theme::alpha(t.bg, 0.3))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(size::TAB_BAR))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|page, _, _, cx| {
                        page.run.abort_armed = false;
                        cx.notify();
                    }),
                )
                .child(body),
        )
    }
    pub(super) fn attempts_form(&self, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let run = self.selected_run()?.clone();
        let (node, _) = self.state.selected.clone()?;
        if let Some(window) = &self.run.attempt_window {
            let mut panel = div()
                .id("attempts-window")
                .track_focus(&self.run.attempts_focus)
                .on_key_down(cx.listener(Self::attempts_key))
                .flex()
                .flex_col()
                .gap(px(PAD))
                .debug_selector(|| "attempts-window".into())
                .child(format!("Attempts: {}", window.title))
                .child(
                    kit::button(t, "attempts-close", "Close", Kind::Quiet)
                        .debug_selector(|| "attempts-close".into())
                        .on_click(cx.listener(|p, _, _, cx| {
                            p.run.attempt_window = None;
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(t, "attempt-prev", "Previous", Kind::Quiet).on_click(cx.listener(
                        |p, _, _, cx| {
                            if let Some(w) = &mut p.run.attempt_window {
                                w.step(-1);
                            }
                            cx.notify();
                        },
                    )),
                )
                .child(
                    kit::button(t, "attempt-next", "Next", Kind::Quiet).on_click(cx.listener(
                        |p, _, _, cx| {
                            if let Some(w) = &mut p.run.attempt_window {
                                w.step(1);
                            }
                            cx.notify();
                        },
                    )),
                )
                .child(
                    kit::button(t, "attempt-compare", "Compare", Kind::Quiet)
                        .debug_selector(|| "attempt-compare".into())
                        .on_click(cx.listener(|p, _, _, cx| {
                            if let Some(w) = &mut p.run.attempt_window {
                                w.toggle_compare();
                            }
                            cx.notify();
                        })),
                );
            if window.compare.is_some() {
                panel = panel.child(
                    kit::button(
                        t,
                        "attempt-side",
                        if window.side == 0 {
                            "Focus right"
                        } else {
                            "Focus left"
                        },
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(|p, _, _, cx| {
                        if let Some(w) = &mut p.run.attempt_window {
                            w.side = 1 - w.side;
                        }
                        cx.notify();
                    })),
                );
            }
            let mut pips = div().flex().gap(px(PAD));
            for attempt in window.attempts() {
                let number = attempt.attempt;
                let label = format!(
                    "Attempt {number} · {}",
                    crate::store::attempts::outcome_word(crate::store::attempts::outcome(
                        attempt,
                        &run.waiting_gates
                            .iter()
                            .map(|g| g.gate_id.clone())
                            .collect()
                    ))
                );
                let selector = format!("attempt-pip-{number}");
                pips = pips.child(
                    kit::button(t, SharedString::from(selector.clone()), label, Kind::Quiet)
                        .debug_selector(move || selector.clone())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            if let Some(w) = &mut p.run.attempt_window {
                                w.choose(number);
                            }
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(pips);
            let selected: Vec<i64> = window
                .compare
                .map(|(a, b)| vec![a, b])
                .unwrap_or_else(|| window.chosen().into_iter().collect());
            let mut columns = div().flex().gap(px(PAD));
            for number in selected {
                if let Some(attempt) = window.attempts().iter().find(|a| a.attempt == number) {
                    let selector = format!("attempt-card-{}", attempt.attempt);
                    let mut card = div()
                        .debug_selector(move || selector.clone())
                        .flex_1()
                        .min_w_0()
                        .p(px(PAD))
                        .border_1()
                        .border_color(t.divider)
                        .child(format!(
                            "Attempt {} · {}",
                            attempt.attempt,
                            crate::store::attempts::outcome_word(crate::store::attempts::outcome(
                                attempt,
                                &run.waiting_gates
                                    .iter()
                                    .map(|g| g.gate_id.clone())
                                    .collect()
                            ))
                        ));
                    let timing =
                        crate::store::attempts::timing(attempt, time::OffsetDateTime::now_utc());
                    if !timing.is_empty() {
                        card = card.child(timing);
                    }
                    if let Some(reason) = &attempt.stop_reason {
                        card = card.child(reason.text.clone());
                    }
                    for verdict in &attempt.verdicts {
                        let waiting = run
                            .waiting_gates
                            .iter()
                            .map(|g| g.gate_id.clone())
                            .collect();
                        card = card.child(format!(
                            "{} · {}",
                            crate::store::attempts::verdict_headline(verdict, &waiting),
                            verdict.reason.text
                        ));
                        for evidence in &verdict.evidence {
                            card = card.child(format!("• {}", evidence.text.text));
                        }
                    }
                    if let Some(pick) = &attempt.picked {
                        let by = crate::store::attempts::picker_words(&pick.decided_by, |id| {
                            self.run
                                .frozen
                                .as_ref()
                                .map(|m| m.node_name(id).to_owned())
                                .unwrap_or_else(|| id.to_owned())
                        });
                        card =
                            card.child(format!("Picked as the best · {by}: {}", pick.reason.text));
                    }
                    let recommendation = run
                        .waiting_gates
                        .iter()
                        .filter_map(|gate| gate.pick.as_ref())
                        .filter(|pick| {
                            pick.get("nodeId").and_then(|v| v.as_str())
                                == Some(window.step.as_str())
                        })
                        .filter_map(|pick| pick.get("recommendation"))
                        .find(|rec| {
                            rec.get("attempt").and_then(|v| v.as_i64()) == Some(attempt.attempt)
                        });
                    if let Some(rec) = recommendation {
                        card = card.child(format!(
                            "The judge picks this one: {}",
                            rec.get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                        ));
                    }
                    if attempt.outputs.is_empty() && attempt.stop_reason.is_none() {
                        card = card.child(match attempt.status.as_str() {
                            "running" => "No output yet.",
                            "stopped" => "Stopped before its output.",
                            _ => "No output recorded.",
                        });
                    }
                    for output in &attempt.outputs {
                        card = card.child(format!("{}: {}", output.label, output.text.text));
                        match self.run.evidence.images.get(&output.path) {
                            Some(crate::store::evidence::Image::Ready(image)) => {
                                let selector = format!("attempt-image-{}", attempt.attempt);
                                card = card.child(
                                    div().debug_selector(move || selector.clone()).child(
                                        gpui::img(image.clone())
                                            .w_full()
                                            .max_h(px(size::NOTICE))
                                            .object_fit(gpui::ObjectFit::Contain),
                                    ),
                                );
                            }
                            Some(crate::store::evidence::Image::Loading) => {
                                card = card.child("Loading image…");
                            }
                            Some(crate::store::evidence::Image::Failed(error)) => {
                                card = card.child(format!("Image unavailable: {error}"));
                            }
                            None => (),
                        }
                        if output.text.truncated {
                            card = card.child("Showing the start of a long output.");
                        }
                        if !output.path.is_empty() {
                            let path = output.path.clone();
                            let node = node.clone();
                            card = card.child(
                                kit::button(
                                    t,
                                    SharedString::from(format!(
                                        "attempt-path-{}-{}",
                                        window.step, attempt.attempt
                                    )),
                                    "Open file",
                                    Kind::Quiet,
                                )
                                .on_click(cx.listener(
                                    move |p, _, _, cx| {
                                        if let Err(e) = p.host.ask(
                                            Ask::OpenPath {
                                                node: NodeId(node.clone()),
                                                path: path.clone(),
                                                beside: false,
                                            },
                                            cx,
                                        ) {
                                            p.run.error = Some(e.message);
                                            cx.notify();
                                        }
                                    },
                                )),
                            );
                        }
                    }
                    columns = columns.child(card);
                }
            }
            panel = panel.child(columns);
            if let Some((gate, seq, candidates)) = run.waiting_gates.iter().find_map(|gate| {
                let pick = gate.pick.as_ref()?;
                (pick.get("nodeId")?.as_str()? == window.step).then(|| {
                    (
                        gate.gate_id.clone(),
                        gate.requested_seq,
                        pick.get("candidates")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
            }) && let Some(chosen) = window
                .chosen()
                .filter(|number| candidates.iter().any(|c| c.as_i64() == Some(*number)))
            {
                panel = panel.child(div().debug_selector(|| "attempt-pick-reason".into()).child(self.run.attempt_reason.clone()))
                        .child(kit::button(t, "attempts-pick-go", format!("Pick attempt {chosen}"), Kind::Primary)
                        .debug_selector(|| "attempts-pick-go".into())
                        .on_click(cx.listener(move |page, _, _, cx| {
                            let reason = page.run.attempt_reason.read(cx).text.trim().to_owned();
                            page.run_post(format!("/gates/{}/pick", segment(&gate)), json!({"requestedSeq":seq,"attempt":chosen,"reason":reason,"actor":"human:operator"}), cx);
                        })));
            }
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(parvi_app_kit::theme::alpha(t.bg, 0.3))
                    .flex()
                    .justify_center()
                    .items_center()
                    .child(
                        kit::panel(t)
                            .w(px(900.))
                            .max_h(px(650.))
                            .id("attempts-body")
                            .overflow_y_scroll()
                            .p(px(PAD))
                            .child(panel),
                    ),
            );
        }
        None
    }
    pub(super) fn run_view(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        if self.run.starting {
            return None;
        }
        self.run.selected.as_ref()?;
        let mut view = div().p(px(PAD)).flex().flex_col().gap(px(PAD));
        let Some(run) = self.selected_run().cloned() else {
            return Some(view.child("Waiting for run state…"));
        };
        let mission = self.run.frozen.as_ref().or(self.state.mission.as_ref());
        let title = mission
            .map(|m| m.title.trim())
            .filter(|title| !title.is_empty())
            .unwrap_or(&run.mission_slug);
        view = view.child(
            div()
                .debug_selector({
                    let title = title.to_owned();
                    move || format!("run-mission-{title}")
                })
                .child(format!("Run · {title}")),
        );
        if self.run.frozen.is_none() && self.run.error.is_some() {
            view = view.child(
                kit::button(t, "retry-run-definition", "Try again", Kind::Plain)
                    .debug_selector(|| "retry-run-definition".into())
                    .on_click(cx.listener(|page, _, _, cx| {
                        if let Some(run) = page.run.selected.clone() {
                            page.show_run(run, cx);
                        }
                    })),
            );
        }
        if let Some(part) = &self.run.reveal_step {
            let name = self
                .run
                .frozen
                .as_ref()
                .map(|m| m.node_name(part))
                .unwrap_or(part.as_str());
            let selector = format!("run-reveal-{part}");
            view = view.child(
                div()
                    .debug_selector(move || selector.clone())
                    .p(px(PAD))
                    .border_1()
                    .border_color(t.accent)
                    .child(format!("Step: {name}")),
            );
        }
        let status_selector = format!("run-finished-{}", run.status);
        view = view.child(
            div()
                .debug_selector(move || status_selector.clone())
                .child(format!(
                    "Status: {}",
                    crate::store::words::state_word(&run.status)
                )),
        );
        let now = time::OffsetDateTime::now_utc();
        let when = crate::store::words::short_time(&run.started_at, now);
        let who = crate::store::words::actor_label(&run.started_by);
        let mut facts = vec![format!("Started {when} by {who}")];
        if run.r#final {
            facts.push(format!("Took {}", crate::store::words::run_span(&run, now)));
        }
        if !run.work.is_empty() {
            facts.push(crate::store::words::work_label(&run.work));
        }
        if let Some(branch) = crate::store::words::run_branch(&run) {
            facts.push(format!("Branch {branch}"));
        }
        view = view
            .child(div().text_color(t.text2).child(facts.join(" · ")))
            .child(
                kit::button(
                    t,
                    "run-events",
                    format!("Events {}", run.event_count),
                    Kind::Quiet,
                )
                .debug_selector(|| "run-events".into())
                .on_click(cx.listener(|page, _, _, cx| {
                    page.run.events_open = !page.run.events_open;
                    cx.notify();
                })),
            );
        if self.run.events_open {
            let mut events = div()
                .id("run-events-drawer")
                .debug_selector(|| "run-events-drawer".into())
                .max_h(px(size::NOTICE))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for event in &run.events {
                let id = if event.node_id.is_empty() {
                    &event.gate_id
                } else {
                    &event.node_id
                };
                let detail = [&event.status, &event.verdict, &event.outcome, &event.detail]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let node = mission
                    .map(|m| m.node_title(id))
                    .filter(|s| !s.is_empty())
                    .unwrap_or(id);
                let text = format!("{} · {} · {} {}", event.seq, event.kind, node, detail);
                let selector = format!("run-event-{}-{}", event.kind, event.detail);
                events = events.child(div().debug_selector(move || selector.clone()).child(text));
            }
            view = view.child(events);
        }
        let node = self
            .state
            .selected
            .as_ref()
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        let place = self
            .host
            .nodes(cx)
            .into_iter()
            .find(|n| n.id.as_str() == node)
            .map(|n| format!("{} ({})", n.device, n.environment))
            .unwrap_or_else(|| node.clone());
        for seat in crate::store::seats::seats(&run) {
            view = view.child(self.seat_control(t, seat, "run-bar", cx));
        }
        for gate in &run.waiting_gates {
            let gate_id = gate.gate_id.clone();
            let seq = gate.requested_seq;
            let gate_title = mission
                .map(|m| m.node_title(&gate_id))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Review");
            let mut row = div()
                .debug_selector(|| "run-gate".into())
                .p(px(PAD))
                .flex()
                .flex_col()
                .gap(px(PAD))
                .border_1()
                .border_color(t.accent)
                .child(
                    div()
                        .debug_selector({
                            let title = gate_title.to_owned();
                            move || format!("gate-title-{title}")
                        })
                        .child(format!("Waiting for your answer · {gate_title}")),
                );
            if let Some((_, Ok(Some(request)))) = self.run.gates.get(&gate_id) {
                let source = self
                    .run
                    .frozen
                    .as_ref()
                    .or(self.state.mission.as_ref())
                    .map(|mission| crate::store::gate::source(mission, &request.input.from_node_id))
                    .unwrap_or_else(|| "upstream step".into());
                let quote = if request.input.text.trim().is_empty() {
                    "The upstream step sent no text.".to_owned()
                } else {
                    request.input.text.clone()
                };
                let question = if request.criterion.trim().is_empty() {
                    "Review the result and decide whether to approve it.".to_owned()
                } else {
                    request.criterion.clone()
                };
                row = row
                    .child(
                        div().debug_selector(|| "gate-request-loaded".into()).child(
                            div()
                                .debug_selector({
                                    let question = question.clone();
                                    move || format!("gate-question-{question}")
                                })
                                .child(question),
                        ),
                    )
                    .child(
                        kit::label(t, format!("From {source}"))
                            .debug_selector(move || format!("gate-source-{source}")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("gate-input-{seq}")))
                            .debug_selector(|| "gate-input-quote".into())
                            .max_h(px(size::NOTICE))
                            .overflow_y_scroll()
                            .p(px(PAD))
                            .rounded(px(size::RADIUS))
                            .border_1()
                            .border_color(t.divider)
                            .bg(t.raised)
                            .text_color(t.text2)
                            .child(quote),
                    );
                if request.input.truncated {
                    row = row.child("Showing the start of a long input.");
                }
            } else if let Some((_, Err(error))) = self.run.gates.get(&gate_id) {
                let retry_gate = gate_id.clone();
                row = row
                    .child(
                        div()
                            .text_color(t.error)
                            .child(format!("Could not load gate input: {error}")),
                    )
                    .child(
                        kit::button(
                            t,
                            SharedString::from(format!("gate-retry-{gate_id}")),
                            "Retry loading gate",
                            Kind::Plain,
                        )
                        .debug_selector(|| "gate-retry".into())
                        .on_click(cx.listener(move |page, _, _, cx| {
                            let response = page.run.responses.get(&retry_gate).cloned();
                            page.run.gates.remove(&retry_gate);
                            page.sync_gate(cx);
                            if let Some(response) = response {
                                page.run.responses.insert(retry_gate.clone(), response);
                            }
                        })),
                    );
            } else {
                row = row.child(div().text_color(t.dim).child("Loading the gate input…"));
            }
            let gate_reference = crate::store::words::gate_reference(&run, gate, &place);
            row = row.child(
                kit::button(
                    t,
                    SharedString::from(format!("gate-send-{gate_id}")),
                    "Send gate to session",
                    Kind::Quiet,
                )
                .on_click(cx.listener(move |p, _, _, cx| {
                    if let Err(e) = p.host.ask(
                        Ask::Send {
                            reference: gate_reference.clone(),
                            target: None,
                        },
                        cx,
                    ) {
                        p.run.error = Some(e.message);
                        cx.notify();
                    }
                })),
            );
            if let Some(response) = self.run.responses.get(&gate_id) {
                row = row.child(
                    div()
                        .debug_selector(|| "gate-response".into())
                        .child(response.clone()),
                );
            }
            let mut approve_label = "Approve".to_owned();
            let mut reject_label = "Send back".to_owned();
            if let Some(mission) = self.run.frozen.as_ref().or(self.state.mission.as_ref()) {
                let flow = flow::build(mission);
                let gate_flow = flow
                    .sections
                    .iter()
                    .flat_map(|s| &s.steps)
                    .find(|s| s.id == gate_id);
                if let Some(flow::Step {
                    kind: flow::StepKind::Gate { pass, fail, .. },
                    ..
                }) = gate_flow
                {
                    let alone =
                        !run_state::project(&run.events)
                            .states
                            .iter()
                            .any(|(id, state)| {
                                id != &gate_id
                                    && matches!(
                                        state,
                                        run_state::NodeState::Running
                                            | run_state::NodeState::Waiting
                                    )
                            });
                    let (approve_words, approve) =
                        crate::store::gate::route(pass, "Approve", alone);
                    let (reject_words, reject) =
                        crate::store::gate::route(fail, "Send back", alone);
                    approve_label = approve;
                    reject_label = reject;
                    row = row.child(approve_words).child(reject_words);
                }
            }
            if let Some(pick) = &gate.pick {
                if let Some(step) = pick.get("nodeId").and_then(|v| v.as_str()) {
                    let step = step.to_owned();
                    row = row.child(
                        kit::button(
                            t,
                            SharedString::from(format!("gate-compare-{gate_id}")),
                            "Compare attempts",
                            Kind::Quiet,
                        )
                        .debug_selector(|| "gate-compare".into())
                        .on_click(cx.listener(
                            move |page, _, window, cx| {
                                page.run.pending_attempt = Some((step.clone(), true));
                                window.focus(&page.run.attempts_focus, cx);
                                page.attempts(cx);
                            },
                        )),
                    );
                }
                for line in crate::store::gate::picking_words(pick, "") {
                    if !line.is_empty() {
                        row = row.child(line);
                    }
                }
                if let Some(candidates) = pick.get("candidates").and_then(|v| v.as_array()) {
                    for candidate in candidates.iter().filter_map(|v| v.as_i64()) {
                        let gate = gate_id.clone();
                        row = row.child(
                            kit::button(
                                t,
                                (
                                    SharedString::from(format!("pick-{}", gate_id)),
                                    candidate as usize,
                                ),
                                format!("Pick {candidate}"),
                                Kind::Primary,
                            )
                            .on_click(
                                cx.listener(move |p, _, _, cx| p.pick(&gate, seq, candidate, cx)),
                            ),
                        );
                    }
                }
            } else {
                let mut actions = div().flex().gap(px(PAD));
                for (word, verdict, kind) in [
                    (approve_label, "pass", Kind::Primary),
                    (reject_label, "fail", Kind::Plain),
                ] {
                    let gate = gate_id.clone();
                    actions = actions.child(
                        kit::button(
                            t,
                            SharedString::from(format!("gate-{}-{verdict}", gate_id)),
                            word,
                            kind,
                        )
                        .debug_selector(move || format!("gate-{verdict}"))
                        .on_click(
                            cx.listener(move |p, _, _, cx| p.verdict(&gate, seq, verdict, cx)),
                        ),
                    );
                }
                row = row.child(actions);
            }
            view = view.child(row);
        }
        if let Some(evidence) = &self.run.evidence.evidence {
            if !evidence.problems.is_empty() {
                view = view.child("Problems");
                for problem in &evidence.problems {
                    let selector = format!("run-problem-{}-{}", problem.seq, problem.reason.text);
                    let name = self
                        .run
                        .frozen
                        .as_ref()
                        .map(|m| m.node_name(&problem.node_id))
                        .unwrap_or(&problem.node_id);
                    view = view.child(
                        div()
                            .debug_selector(move || selector.clone())
                            .text_color(t.error)
                            .child(format!("{name}: {}", problem.reason.text)),
                    );
                }
            }
            if !evidence.files.is_empty() {
                view = view.child("Produced files");
            }
            for (index, file) in evidence.files.iter().enumerate() {
                let path = file.path.clone();
                let owner = node.clone();
                let selector = format!("run-file-{index}");
                let name = self
                    .run
                    .frozen
                    .as_ref()
                    .map(|m| m.node_name(&file.node_id))
                    .unwrap_or(&file.node_id);
                view = view.child(
                    kit::button(
                        t,
                        SharedString::from(selector.clone()),
                        format!("{} · {} · {} bytes", name, file.path, file.size),
                        Kind::Quiet,
                    )
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let Err(error) = p.host.ask(
                            Ask::OpenPath {
                                node: NodeId(owner.clone()),
                                path: path.clone(),
                                beside: true,
                            },
                            cx,
                        ) {
                            p.run.error = Some(error.message);
                            cx.notify();
                        }
                    })),
                );
            }
            if evidence.truncated {
                view = view.child("Some files are not shown.");
            }
            if !evidence.briefs.is_empty() {
                view = view.child("Briefs");
            }
            for brief in &evidence.briefs {
                let path = brief.path.clone();
                let owner = node.clone();
                let selector = format!("run-brief-{}", brief.dispatch_seq);
                let name = self
                    .run
                    .frozen
                    .as_ref()
                    .map(|m| m.node_name(&brief.node_id))
                    .unwrap_or(&brief.node_id);
                view = view.child(
                    kit::button(
                        t,
                        SharedString::from(selector.clone()),
                        format!("{} · {}", name, brief.path),
                        Kind::Quiet,
                    )
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let Err(error) = p.host.ask(
                            Ask::OpenPath {
                                node: NodeId(owner.clone()),
                                path: path.clone(),
                                beside: true,
                            },
                            cx,
                        ) {
                            p.run.error = Some(error.message);
                            cx.notify();
                        }
                    })),
                );
            }
        } else if let Some(error) = &self.run.evidence.error {
            view = view.child(
                div()
                    .text_color(t.error)
                    .child(format!("Could not load evidence: {error}")),
            );
        }
        if !run.r#final {
            let reference = crate::store::words::run_reference(&run, &place);
            view = view.child(
                kit::button(t, "run-send", "Send run to session", Kind::Quiet)
                    .debug_selector(|| "run-send".into())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let Err(e) = p.host.ask(
                            Ask::Send {
                                reference: reference.clone(),
                                target: None,
                            },
                            cx,
                        ) {
                            p.run.error = Some(e.message);
                            cx.notify();
                        }
                    })),
            );
        }
        if !run.r#final {
            view = view.child(
                kit::button(t, "run-abort", "Stop run", Kind::Quiet)
                    .debug_selector(|| "run-abort".into())
                    .on_click(cx.listener(|page, _, window, cx| {
                        page.run
                            .stop_reason
                            .update(cx, |field, cx| field.set("", false, cx));
                        page.run.abort_armed = true;
                        parvi_app_kit::widgets::field::focus(&page.run.stop_reason, window, cx);
                        cx.notify();
                    })),
            );
        } else if !run.repository.is_empty() {
            view = view.child(
                kit::button(t, "run-land", "Land repository run", Kind::Quiet)
                    .debug_selector(|| "run-land".into())
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.run_post("/land".into(), json!({"force":false}), cx)
                    })),
            );
        }
        if run.resume_allowed {
            view = view.child(
                kit::button(t, "run-resume", "Resume", Kind::Primary).on_click(cx.listener(
                    |p, _, _, cx| {
                        p.run_post(
                            "/resume".into(),
                            json!({"mode":"reattach", "actor":"human:operator"}),
                            cx,
                        )
                    },
                )),
            );
        }
        if run.resume_allowed {
            view = view
                .child(
                    kit::button(t, "run-redispatch", "Redispatch", Kind::Quiet).on_click(
                        cx.listener(|p, _, _, cx| {
                            p.run_post(
                                "/resume".into(),
                                json!({"mode":"redispatch", "actor":"human:operator"}),
                                cx,
                            )
                        }),
                    ),
                )
                .child(
                    kit::button(t, "run-grant", "Grant another allowance", Kind::Quiet).on_click(
                        cx.listener(|p, _, _, cx| {
                            p.run_post(
                                "/resume".into(),
                                json!({"mode":"reattach", "grant":true, "actor":"human:operator"}),
                                cx,
                            )
                        }),
                    ),
                )
                .child(
                    kit::button(t, "run-take-fail", "Take fail route", Kind::Quiet).on_click(
                        cx.listener(|p, _, _, cx| {
                            p.run_post(
                            "/resume".into(),
                            json!({"mode":"reattach", "takeFail":true, "actor":"human:operator"}),
                            cx,
                        )
                        }),
                    ),
                )
                .child(
                    kit::button(t, "run-pick-best", "Pick best attempt", Kind::Quiet).on_click(
                        cx.listener(|p, _, _, cx| {
                            p.run_post(
                            "/resume".into(),
                            json!({"mode":"reattach", "pickBest":true, "actor":"human:operator"}),
                            cx,
                        )
                        }),
                    ),
                );
        }
        for point in run_state::points(&run) {
            let title = self
                .run
                .frozen
                .as_ref()
                .map(|m| m.node_name(&point.node))
                .unwrap_or(point.node.as_str());
            let phrase = run_state::phrase(&point, title);
            let selector = format!("run-sentence-{phrase}");
            view = view.child(div().debug_selector(move || selector.clone()).child(phrase));
        }
        let mission = self
            .run
            .frozen
            .as_ref()
            .or(self.state.mission.as_ref())
            .cloned();
        let state = run_state::project(&run.events);
        let mut modes = div().flex().gap(px(TAB_GAP));
        for (label, canvas) in [("Flow", false), ("Canvas", true)] {
            let selector = if canvas {
                "run-mode-canvas"
            } else {
                "run-mode-flow"
            };
            modes = modes.child(
                kit::button(
                    t,
                    selector,
                    label,
                    if self.run.canvas_mode == canvas {
                        Kind::Plain
                    } else {
                        Kind::Quiet
                    },
                )
                .debug_selector(move || selector.into())
                .on_click(cx.listener(move |page, _, _, cx| {
                    page.run.canvas_mode = canvas;
                    cx.notify();
                })),
            );
        }
        view = view.child(modes);
        if self.run.canvas_mode {
            if let Some(mission) = mission.as_ref() {
                view = view.child(
                    self.render_canvas(t, mission, true, cx)
                        .h(px(size::NOTICE))
                        .w_full(),
                );
            }
        } else if let Some(mission) = mission.as_ref() {
            let flow = flow::build(mission);
            for section in &flow.sections {
                if let Some(input) = &section.input {
                    let mut card = div()
                        .p(px(PAD))
                        .border_1()
                        .border_color(t.accent)
                        .bg(t.surface)
                        .child(format!("Input · {}", mission.node_name(input)));
                    if let Some(definition) =
                        mission.input_cards.iter().find(|card| card.id == *input)
                        && !definition.goal.is_empty()
                    {
                        card = card.child(definition.goal.clone());
                    }

                    for value in &run.inputs {
                        card = card.child(format!("{} · {}", value.name, value.value));
                    }
                    if !run.work.is_empty() {
                        card = card.child(format!(
                            "Work · {}",
                            crate::store::words::work_label(&run.work)
                        ));
                    }
                    if !run.repository.is_empty() {
                        card = card.child(format!("Repository · {}", run.repository));
                    } else if !run.cwd.is_empty() {
                        card = card.child(format!("Folder · {}", run.cwd));
                    }
                    view = view.child(card);
                }
                if !section.start.is_empty() {
                    view = view.child(format!("Starts at {}", target_words(&section.start)));
                }
                for step in &section.steps {
                    let status = state.states.get(&step.id);
                    let label = format!(
                        "{} · {} · {}",
                        step.number,
                        mission.node_name(&step.id),
                        status
                            .map(|s| format!("{s:?}"))
                            .unwrap_or_else(|| "Pending".into())
                    );
                    let selector = format!(
                        "run-step-{}-{}",
                        step.id,
                        status
                            .map(|s| format!("{s:?}").to_lowercase())
                            .unwrap_or_else(|| "pending".into())
                    );
                    let mut card = div()
                        .debug_selector(move || selector.clone())
                        .border_1()
                        .border_color(
                            if self.run.reveal_step.as_deref() == Some(step.id.as_str()) {
                                t.accent
                            } else {
                                t.divider
                            },
                        )
                        .p(px(PAD))
                        .child(label);
                    if let Some(formation) = mission.formations.iter().find(|f| f.id == step.id) {
                        if let Some(brief) = &formation.brief {
                            card = card.child(brief.goal.clone());
                        }
                        for slot in &formation.slots {
                            card = card.child(crate::store::words::staffing(slot));
                        }
                    }
                    match &step.kind {
                        flow::StepKind::Formation { next } => {
                            card = card.child(format!("Next: {}", target_words(next)));
                        }
                        flow::StepKind::Gate {
                            deciders,
                            judges,
                            pass,
                            fail,
                        } => {
                            card = card
                                .child(format!(
                                    "Decided by: {}",
                                    deciders
                                        .iter()
                                        .map(|d| match d {
                                            flow::Decider::You => "you",
                                            flow::Decider::Judge => "judge",
                                            flow::Decider::Code => "code",
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ))
                                .child(format!("Pass: {}", target_words(pass)))
                                .child(format!("Fail: {}", target_words(fail)));
                            if let Some(gate) = mission.gates.iter().find(|gate| gate.id == step.id)
                                && !gate.criterion.is_empty()
                            {
                                card = card.child(gate.criterion.clone());
                            }
                            for judge in judges {
                                let staffing = mission
                                    .formations
                                    .iter()
                                    .find(|f| f.id == *judge)
                                    .map(|f| {
                                        f.slots
                                            .iter()
                                            .map(crate::store::words::staffing)
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    })
                                    .unwrap_or_default();
                                card = card.child(format!(
                                    "Judge: {} · {staffing}",
                                    mission.node_name(judge)
                                ));
                            }
                        }
                    }
                    if step.parallel.len() > 1 {
                        card = card.child(format!(
                            "Alongside · {}",
                            step.parallel
                                .iter()
                                .filter(|id| **id != step.id)
                                .map(|id| mission.node_name(id))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    for limit in mission
                        .limits
                        .iter()
                        .filter(|limit| limit.target == step.id)
                    {
                        let mut words = Vec::new();
                        if let Some(n) = limit.rounds {
                            words.push(format!("{n} rounds"));
                        }
                        if let Some(n) = limit.seconds {
                            words.push(crate::store::words::duration_text(n));
                        }
                        if let Some(n) = limit.tokens {
                            words.push(format!("{n} tokens"));
                        }
                        card = card.child(format!(
                            "Limit · {} · {}",
                            words.join(", "),
                            limit.when_spent
                        ));
                    }
                    if let Some(point) = run_state::points(&run)
                        .iter()
                        .find(|point| point.node == step.id && !point.reason.is_empty())
                    {
                        card = card.child(div().text_color(t.error).child(point.reason.clone()));
                    }
                    if run.waiting_gates.iter().any(|gate| gate.gate_id == step.id) {
                        card = card.child("Your answer is asked for here.");
                    }
                    let produced: Vec<_> = run
                        .outputs
                        .iter()
                        .filter(|output| output.node_id == step.id)
                        .map(|output| output.name.clone())
                        .collect();
                    if !produced.is_empty() {
                        card = card.child(format!("Produced {}", produced.join(", ")));
                    }
                    for seat in crate::store::seats::seats(&run)
                        .into_iter()
                        .filter(|seat| seat.step == step.id)
                    {
                        card = card.child(self.seat_control(
                            t,
                            seat,
                            &format!("flow-{}", step.id),
                            cx,
                        ));
                    }
                    if let Some(count) = state
                        .attempts
                        .get(&step.id)
                        .copied()
                        .filter(|count| *count > 0)
                    {
                        let step_id = step.id.clone();
                        let selector = format!("run-step-attempts-{step_id}");
                        card = card.child(
                            kit::button(
                                t,
                                SharedString::from(selector.clone()),
                                format!("{count} attempt{}", if count == 1 { "" } else { "s" }),
                                Kind::Plain,
                            )
                            .debug_selector(move || selector.clone())
                            .on_click(cx.listener(
                                move |page, _, window, cx| {
                                    page.open_canvas_attempts(&step_id, window, cx)
                                },
                            )),
                        );
                    }
                    view = view.child(card);
                }
            }
        }
        view = view.child(
            kit::button(t, "attempts", "Attempts", Kind::Quiet)
                .debug_selector(|| "run-attempts".into())
                .on_click(cx.listener(|p, _, _, cx| p.attempts(cx))),
        );
        if let Some(attempts) = &self.run.attempts {
            for step in &attempts.attempts.steps {
                let id = step.node_id.clone();
                let chosen = id.clone();
                view =
                    view.child(
                        kit::button(
                            t,
                            SharedString::from(format!("attempt-step-{id}")),
                            format!("Attempts: {}", step.title),
                            Kind::Quiet,
                        )
                        .debug_selector(move || format!("attempt-step-{id}"))
                        .on_click(cx.listener(move |p, _, window, cx| {
                            p.show_attempt_step(&chosen, window, cx)
                        })),
                    );
            }
        }
        for output in &run.outputs {
            let text = format!("{}: {}", output.name, output.value);
            let reference = format!(
                "{} from run {} (mission {}) on {}",
                text, run.run_id, run.mission_slug, place
            );
            let send_id = format!("send-output-{}", output.name);
            view = view.child(text).child(
                kit::button(
                    t,
                    SharedString::from(send_id),
                    "Send output to session",
                    Kind::Quiet,
                )
                .on_click(cx.listener(move |p, _, _, cx| {
                    if let Err(e) = p.host.ask(
                        Ask::Send {
                            reference: reference.clone(),
                            target: None,
                        },
                        cx,
                    ) {
                        p.run.error = Some(e.message);
                        cx.notify();
                    }
                })),
            );
            if matches!(output.kind.as_str(), "file" | "folder") {
                let path = output.value.clone();
                let node = node.clone();
                view = view.child(
                    kit::button(
                        t,
                        SharedString::from(format!("output-{}", output.name)),
                        "Open",
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let Err(e) = p.host.ask(
                            Ask::OpenPath {
                                node: NodeId(node.clone()),
                                path: path.clone(),
                                beside: false,
                            },
                            cx,
                        ) {
                            p.run.error = Some(e.message);
                        }
                    })),
                );
            }
        }
        view = view.when_some(self.run.error.clone(), |d, e| {
            d.child(div().text_color(t.error).child(e))
        });
        Some(view)
    }
}
fn segment(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(super) fn target_words(targets: &[flow::Target]) -> String {
    targets
        .iter()
        .map(|target| match target {
            flow::Target::Step {
                node,
                number,
                title,
                back,
            } => format!(
                "{}{}{}",
                number.map(|n| format!("{n}. ")).unwrap_or_default(),
                if title.is_empty() { node } else { title },
                if *back { " ↩" } else { "" }
            ),
            flow::Target::End {
                node,
                title,
                outcome,
            } => format!(
                "{} ({})",
                if title.is_empty() { node } else { title },
                outcome
            ),
            flow::Target::Nowhere => "not connected".into(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

enum StartAnswer {
    Started(String),
    Refused(parvi_app_kit::host::Answer),
    Unknown(String),
}
fn start_answer(result: Result<parvi_app_kit::host::Answer, parvi_core::Error>) -> StartAnswer {
    match result {
        Ok(answer) if answer.ok() => match serde_json::from_slice::<RunId>(&answer.body) {
            Ok(run) if !run.run_id.is_empty() => StartAnswer::Started(run.run_id),
            _ => StartAnswer::Unknown("the node did not name the run".into()),
        },
        Ok(answer) => StartAnswer::Refused(answer),
        Err(error) => StartAnswer::Unknown(error.message),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use parvi_app_kit::host::Answer;
    #[test]
    fn a_start_without_an_answer_stays_unknown_until_the_node_answers() {
        assert!(matches!(
            start_answer(Err(parvi_core::Error::new("offline", "lost"))),
            StartAnswer::Unknown(_)
        ));
        assert!(matches!(
            start_answer(Ok(Answer {
                status: 200,
                body: b"{}".to_vec()
            })),
            StartAnswer::Unknown(_)
        ));
        assert!(
            matches!(start_answer(Ok(Answer { status:202, body:br#"{"runId":"run_1"}"#.to_vec() })), StartAnswer::Started(id) if id == "run_1")
        );
        assert!(matches!(
            start_answer(Ok(Answer {
                status: 409,
                body: vec![]
            })),
            StartAnswer::Refused(_)
        ));
    }
}
