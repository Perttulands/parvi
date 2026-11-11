//! Run controls and views; the channel projection remains in the store.
use super::*;
use crate::store::run_state;
use gpui::SharedString;
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
    target: Option<String>,
    request_id: String,
    uncertain: Option<(serde_json::Value, String)>,
    cwd: Entity<TextField>,
    repository: Entity<TextField>,
    base: Entity<TextField>,
    work: Entity<TextField>,
    paths: Entity<TextField>,
    busy: bool,
    abort_armed: bool,
    fields: BTreeMap<String, Entity<TextField>>,
    responses: BTreeMap<String, Entity<TextField>>,
    pub error: Option<String>,
    pub findings: Vec<String>,
    attempts: Option<AttemptsResponse>,
    attempt_window: Option<crate::store::attempts::AttemptsWindow>,
    attempts_at: Option<usize>,
    evidence: crate::store::evidence::Store,
    gates: BTreeMap<String, (u64, Option<parvi_missions_contract::GateRequest>)>,
}
impl RunUi {
    pub fn new(cx: &mut Context<MissionsPage>) -> Self {
        Self {
            selected: None,
            reveal_step: None,
            frozen: None,
            starting: false,
            target: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            uncertain: None,
            cwd: cx.new(|cx| TextField::new(cx, "Working directory")),
            repository: cx.new(|cx| TextField::new(cx, "Repository")),
            base: cx.new(|cx| TextField::new(cx, "Base")),
            work: cx.new(|cx| TextField::new(cx, "Work reference")),
            paths: cx.new(|cx| TextField::new(cx, "Context paths, one per line").prose()),
            busy: false,
            abort_armed: false,
            fields: BTreeMap::new(),
            responses: BTreeMap::new(),
            error: None,
            findings: vec![],
            attempts: None,
            attempt_window: None,
            attempts_at: None,
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
        self.run.selected = Some(run.clone());
        self.run.reveal_step = None;
        self.run.abort_armed = false;
        self.run.starting = false;
        self.run.frozen = None;
        self.run.attempts = None;
        self.run.attempt_window = None;
        self.run.attempts_at = None;
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
            self.run.gates.insert(id.clone(), (seq, None));
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
                                page.run.gates.insert(id, (seq, Some(v.request)));
                            }
                            Ok(_) => {}
                            Err(e) => page.run.error = Some(e.to_string()),
                        },
                        Ok(a) => page.run.error = Some(a.error_text()),
                        Err(e) => page.run.error = Some(e.message),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }
    pub(super) fn prepare_run(&mut self, cx: &mut Context<Self>) {
        if self.run_unsaved(cx).is_some() {
            return;
        }
        let Some(m) = &self.state.mission else {
            return;
        };
        let Some(input) = m.input_cards.first() else {
            self.state.error = Some("This mission has no input card.".into());
            cx.notify();
            return;
        };
        let names: Vec<String> = if input.inputs.is_empty() {
            vec!["brief".into()]
        } else {
            input.inputs.iter().map(|i| i.name.clone()).collect()
        };
        self.run.target = None;
        self.run.request_id = uuid::Uuid::new_v4().to_string();
        self.run.uncertain = None;
        self.run.fields = names
            .into_iter()
            .map(|name| {
                let field = cx.new(|cx| TextField::new(cx, &name).prose());
                (name, field)
            })
            .collect();
        self.run.selected = None;
        self.run.findings.clear();
        self.run.error = None;
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
        self.run.target = Some(id);
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
        self.run.starting = true;
        self.editor = None;
        cx.notify();
    }
    pub(super) fn start_run(&mut self, cx: &mut Context<Self>) {
        if self.run.busy {
            return;
        }
        let (Some((node, slug)), Some(m), Some(etag)) = (
            self.state.selected.clone(),
            self.state.mission.as_ref(),
            self.state.mission_etag.clone(),
        ) else {
            return;
        };
        let target = self.run.target.clone();
        let input_card_id = if target.is_none() {
            m.input_cards
                .first()
                .map(|c| c.id.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let values = self
            .run
            .fields
            .iter()
            .map(|(name, f)| (name.clone(), f.read(cx).text.clone()))
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
    fn show_attempt_step(&mut self, step_id: &str, cx: &mut Context<Self>) {
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
        let mut window =
            crate::store::attempts::AttemptsWindow::new(node, run, step_id, &step.title, 0);
        window.read(step, None);
        self.run.attempt_window = Some(window);
        self.load_attempt_images(cx);
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
    pub(super) fn run_view(&self, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        if self.run.starting {
            let mut form = div()
                .p(px(PAD))
                .flex()
                .flex_col()
                .gap(px(PAD))
                .child("Start mission");
            for (name, field) in &self.run.fields {
                let selector = format!("run-input-{name}");
                form = form.child(name.clone()).child(
                    div()
                        .debug_selector(move || selector.clone())
                        .child(field.clone()),
                );
            }
            form = form.child(
                kit::button(t, "run-start", "Start run", Kind::Primary)
                    .debug_selector(|| "run-start".into())
                    .on_click(cx.listener(|p, _, _, cx| p.start_run(cx))),
            );
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
                .child(div().child(self.run.paths.clone()));
            if self.run.uncertain.is_some() && !self.run.busy {
                form = form.child("Retry sends the same start request and its original values.");
            }
            form = form.when_some(self.run.error.clone(), |d, e| {
                d.child(div().text_color(t.error).child(e))
            });
            for (i, finding) in self.run.findings.iter().enumerate() {
                let selector = format!("run-finding-{i}-{finding}");
                form = form.child(
                    div()
                        .debug_selector(move || selector.clone())
                        .text_color(t.error)
                        .child(finding.clone()),
                );
            }
            return Some(form);
        }
        let id = self.run.selected.as_ref()?;
        let mut view = div()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(PAD))
            .child(format!("Run {id}"));
        let Some(run) = self.selected_run().cloned() else {
            return Some(view.child("Waiting for run state…"));
        };
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
                .child(format!("Status: {}", run.status)),
        );
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
            let mut row = div()
                .debug_selector(|| "run-gate".into())
                .p(px(PAD))
                .border_1()
                .border_color(t.accent)
                .child(format!("Waiting for you: {gate_id}"));
            if let Some((_, Some(request))) = self.run.gates.get(&gate_id) {
                if let Some(mission) = self.run.frozen.as_ref().or(self.state.mission.as_ref()) {
                    row = row.child(format!(
                        "From {}",
                        crate::store::gate::source(mission, &request.input.from_node_id)
                    ));
                }
                row = row
                    .child(
                        div()
                            .debug_selector(|| "gate-request-loaded".into())
                            .child(request.criterion.clone()),
                    )
                    .child(request.input.text.clone());
            }
            let gate_reference = crate::store::words::gate_reference(&run, gate, &place);
            row = row.child(
                kit::button(
                    t,
                    SharedString::from(format!("gate-send-{gate_id}")),
                    "Send",
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
                    let alone = run_state::points(&run).len() <= 1;
                    row = row
                        .child(crate::store::gate::route(pass, "Approve", alone).0)
                        .child(crate::store::gate::route(fail, "Reject", alone).0);
                }
            }
            if let Some(pick) = &gate.pick {
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
                for (word, verdict) in [("Approve", "pass"), ("Reject", "fail")] {
                    let gate = gate_id.clone();
                    row = row.child(
                        kit::button(
                            t,
                            SharedString::from(format!("gate-{}-{verdict}", gate_id)),
                            word,
                            Kind::Primary,
                        )
                        .debug_selector(move || format!("gate-{verdict}"))
                        .on_click(
                            cx.listener(move |p, _, _, cx| p.verdict(&gate, seq, verdict, cx)),
                        ),
                    );
                }
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
        let reference = crate::store::words::run_reference(&run, &place);
        view = view.child(
            kit::button(t, "run-send", "Send", Kind::Quiet).on_click(cx.listener(
                move |p, _, _, cx| {
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
                },
            )),
        );
        if !run.r#final {
            view = view.child(
                kit::button(
                    t,
                    "run-abort",
                    if self.run.abort_armed {
                        "Confirm abort"
                    } else {
                        "Abort run"
                    },
                    Kind::Quiet,
                )
                .on_click(cx.listener(|p, _, _, cx| {
                    if !p.run.abort_armed {
                        p.run.abort_armed = true;
                        cx.notify();
                        return;
                    }
                    p.run.abort_armed = false;
                    p.run_post(
                        "/abort".into(),
                        json!({"requestedBy":"human:operator", "reason":"Stopped by operator"}),
                        cx,
                    );
                })),
            );
        } else if run.status == "succeeded" {
            view = view.child(kit::button(t, "run-land", "Land", Kind::Quiet).on_click(
                cx.listener(|p, _, _, cx| p.run_post("/land".into(), json!({"force":false}), cx)),
            ));
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
            view = view.child(run_state::phrase(&point, title));
        }
        let mission = self.run.frozen.as_ref().or(self.state.mission.as_ref());
        let state = run_state::project(&run.events);
        if let Some(mission) = mission {
            let flow = flow::build(mission);
            for section in &flow.sections {
                if let Some(input) = &section.input {
                    view = view.child(format!("Input · {}", mission.node_name(input)));
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
                            for judge in judges {
                                card = card.child(format!("Judge: {}", mission.node_name(judge)));
                            }
                        }
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
                view = view.child(
                    kit::button(
                        t,
                        SharedString::from(format!("attempt-step-{id}")),
                        format!("Attempts: {}", step.title),
                        Kind::Quiet,
                    )
                    .debug_selector(move || format!("attempt-step-{id}"))
                    .on_click(cx.listener(move |p, _, _, cx| p.show_attempt_step(&chosen, cx))),
                );
            }
        }
        if let Some(window) = &self.run.attempt_window {
            view = view
                .child(format!("Attempts: {}", window.title))
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
                view = view.child(
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
            let selected: Vec<i64> = window
                .compare
                .map(|(a, b)| vec![a, b])
                .unwrap_or_else(|| window.chosen().into_iter().collect());
            let mut columns = div().flex().gap(px(PAD));
            for number in selected {
                if let Some(attempt) = window.attempts().iter().find(|a| a.attempt == number) {
                    let mut card = div()
                        .flex_1()
                        .min_w_0()
                        .p(px(PAD))
                        .border_1()
                        .border_color(t.divider)
                        .child(format!("Attempt {} · {}", attempt.attempt, attempt.status));
                    for verdict in &attempt.verdicts {
                        card = card.child(format!(
                            "{} · {} · {}",
                            verdict.gate_title, verdict.verdict, verdict.reason.text
                        ));
                    }
                    if let Some(pick) = &attempt.picked {
                        card = card.child(format!("Picked: {}", pick.reason.text));
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
            view = view.child(columns);
        }
        for output in &run.outputs {
            let text = format!("{}: {}", output.name, output.value);
            let reference = format!(
                "{} from run {} (mission {}) on {}",
                text, run.run_id, run.mission_slug, place
            );
            let send_id = format!("send-output-{}", output.name);
            view = view.child(text).child(
                kit::button(t, SharedString::from(send_id), "Send", Kind::Quiet).on_click(
                    cx.listener(move |p, _, _, cx| {
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
                    }),
                ),
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

fn target_words(targets: &[flow::Target]) -> String {
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
