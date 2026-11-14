//! A run on the page skeleton, as the operator reads it on coming back:
//! where it stands in one line, what it produced, then each step with who
//! does it, its seat and its brief.
use super::run::sentence;
use super::*;
use crate::store::{run_state, words};
use gpui::{ClickEvent, SharedString, rgb};
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::theme::Theme;
use parvi_app_kit::widgets::pane::{self, Col};
use parvi_app_kit::{Host, ask::Ask};
use parvi_missions_contract::{Mission, RunOutput, RunProjection};

const STATE_W: f32 = 72.;
const STEP_ACTS_W: f32 = 150.;
/// Characters of a file's path shown beside its name.
const PATH_CHARS: usize = 48;

/// The steps of a run: the step, who does it, how it stands, its controls.
/// The step and who share what is left, so a narrow detail keeps both.
const STEPS: [Col; 4] = [
    Col::fill("Step"),
    Col::fill("Who"),
    Col::fixed("State", STATE_W),
    Col::fixed("", STEP_ACTS_W),
];

impl MissionsPage {
    /// The run shown, as the detail's head and body; `None` while no run is
    /// chosen or the start form stands in for one.
    pub(super) fn run_detail(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<(Div, Div)> {
        if self.run.starting {
            return None;
        }
        self.run.selected.as_ref()?;
        let Some(run) = self.selected_run().cloned() else {
            return Some((
                pane::head(t),
                pane::empty(t, "Reading the run…", None).p_0(),
            ));
        };
        if !run.r#final && !self.run.ticking {
            // Only a drawn running run asks for the next second; a hidden
            // page or a finished run draws no more and so stops asking.
            self.run.ticking = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let _ = this.update(cx, |page, cx| {
                    page.run.ticking = false;
                    cx.notify();
                });
            })
            .detach();
        }
        let mission = self
            .run
            .frozen
            .clone()
            .or_else(|| self.state.mission.clone());
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
        let name_of = |id: &str| -> String {
            mission
                .as_ref()
                .map(|m| m.node_name(id).to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.to_owned())
        };
        let head = self.run_head(t, &run, mission.as_ref(), &node, &place, cx);
        let shown = format!("run-detail-{}", run.run_id);
        let mut body = div()
            .debug_selector(move || shown.clone())
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP));
        body = body.child(self.run_standing(t, &run, &name_of, &node, cx));
        if let Some(error) = &self.run.error {
            body = body.child(
                div()
                    .debug_selector(|| "run-error".into())
                    .text_color(t.error)
                    .child(error.clone()),
            );
        }
        if self.run.frozen.is_none() && self.run.error.is_some() {
            body = body.child(
                div().flex().child(
                    pane::act(t, "retry-run-definition", "Try again")
                        .debug_selector(|| "retry-run-definition".into())
                        .on_click(cx.listener(|page, _, _, cx| {
                            if let Some(run) = page.run.selected.clone() {
                                page.show_run(run, cx);
                            }
                        })),
                ),
            );
        }
        if run.resume_allowed {
            body = body.child(self.resume_choices(t, cx));
        }
        body = body.children(self.run_gates(t, &run, mission.as_ref(), &place, cx));
        body = body.children(self.run_result(t, &run, &node, &place, &name_of, cx));
        if let Some(mission) = &mission {
            body = body.child(self.run_steps(t, &run, mission, &node, cx));
        }
        if let Some(error) = &self.run.evidence.error {
            body = body.child(pane::meta(
                t,
                format!("Could not read what the run left behind: {error}"),
                Some(t.error),
            ));
        }
        Some((head, body))
    }

    fn run_head(
        &self,
        t: &Theme,
        run: &RunProjection,
        mission: Option<&Mission>,
        node: &str,
        place: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = mission
            .map(|m| m.title.trim())
            .filter(|title| !title.is_empty())
            .unwrap_or(&run.mission_slug)
            .to_owned();
        let number = crate::store::navigator::groups(&self.state)
            .into_iter()
            .flat_map(|group| group.runs)
            .find(|row| row.run.run_id == run.run_id)
            .map(|row| row.number);
        let heading = match number {
            Some(n) => format!("{title} · run {n}"),
            None => title.clone(),
        };
        let mut head = pane::head(t)
            .child(pane::title(t, heading).debug_selector(move || format!("run-mission-{title}")))
            .child(
                pane::send(t, "run-send", &self.host, words::run_reference(run, place))
                    .debug_selector(|| "run-send".into()),
            );
        if !run.r#final {
            return head.child(
                pane::act(t, "run-abort", "Stop")
                    .debug_selector(|| "run-abort".into())
                    .tooltip(kit::tip("Stop the run; its seats close"))
                    .on_click(cx.listener(|page, _, window, cx| {
                        page.run
                            .stop_reason
                            .update(cx, |field, cx| field.set("", false, cx));
                        page.run.abort_armed = true;
                        parvi_app_kit::widgets::field::focus(&page.run.stop_reason, window, cx);
                        cx.notify();
                    })),
            );
        }
        if let Some(value) = primary_output(run) {
            let node = node.to_owned();
            let folder = run.cwd.clone();
            head = head.child(
                pane::act(t, "run-next", "Start next")
                    .debug_selector(|| "run-next".into())
                    .on_click(cx.listener(move |page, e: &ClickEvent, _, cx| {
                        let rows = page.next_menu(&node, &value, &folder, cx);
                        page.host.menu(rows, e.position(), cx);
                    })),
            );
        }
        if self.state.mission.is_some()
            && self.state.selected.as_ref().map(|(_, slug)| slug) == Some(&run.mission_slug)
        {
            let inputs: Vec<(String, String)> = run
                .inputs
                .iter()
                .map(|value| (value.name.clone(), value.value.clone()))
                .collect();
            let card = run.input_card_id.clone();
            head = head.child(
                pane::act(t, "run-again", "Run again")
                    .debug_selector(|| "run-again".into())
                    .tooltip(kit::tip("Start this mission again with the same inputs"))
                    .on_click(cx.listener(move |p, _, _, cx| p.run_again(&card, &inputs, cx))),
            );
        }
        if !run.repository.is_empty() {
            head = head.child(
                pane::act(t, "run-land", "Land")
                    .debug_selector(|| "run-land".into())
                    .tooltip(kit::tip("Merge the run's branch into its repository"))
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.run_post("/land".into(), json!({"force":false}), cx)
                    })),
            );
        }
        head
    }

    /// The missions this node can start, each taking `value` as its first
    /// input and `folder` as where it works.
    fn next_menu(
        &self,
        node: &str,
        value: &str,
        folder: &str,
        cx: &mut Context<Self>,
    ) -> Vec<MenuRow> {
        let page = cx.entity().downgrade();
        crate::store::navigator::groups(&self.state)
            .into_iter()
            .filter(|group| group.node == node && group.steps > 0)
            .map(|group| {
                let (page, node, slug) = (page.clone(), group.node.clone(), group.slug.clone());
                let (value, folder) = (value.to_owned(), folder.to_owned());
                MenuRow::new(group.title, move |_, cx| {
                    let _ = page.update(cx, |p, cx| {
                        p.use_output(
                            node.clone(),
                            slug.clone(),
                            value.clone(),
                            folder.clone(),
                            cx,
                        )
                    });
                })
            })
            .collect()
    }

    /// Where the run stands, in one line coloured by its state, then what it
    /// was given and where it works.
    fn run_standing(
        &self,
        t: &Theme,
        run: &RunProjection,
        name_of: &dyn Fn(&str) -> String,
        node: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let now = time::OffsetDateTime::now_utc();
        let (words, colour) = standing(self, t, run, name_of, now);
        let selector = format!("run-finished-{}", run.status);
        let mut out = div().flex().flex_col().child(
            div()
                .debug_selector(move || selector.clone())
                .pb(px(size::HEAD_GAP))
                .text_color(colour)
                .children(run_state::points(run).iter().filter_map(|point| {
                    let phrase = run_state::phrase(point, &name_of(&point.node));
                    (!phrase.is_empty())
                        .then(|| div().debug_selector(move || format!("run-sentence-{phrase}")))
                }))
                .child(words),
        );
        if let Some(note) = run_state::points(run)
            .iter()
            .find(|p| p.kind == run_state::PointKind::Running)
            .and_then(|p| run_state::seat_note(&run.events, &p.node))
        {
            out = out.child(
                div()
                    .pb(px(size::HEAD_GAP))
                    .text_color(rgb(t.ansi[3]))
                    .child(sentence(&note)),
            );
        }
        for (index, value) in run.inputs.iter().enumerate() {
            if !matches!(value.kind.as_str(), "file" | "folder") {
                out = out.child(pane::line(t, value.name.clone(), value.value.clone()));
                continue;
            }
            let (owner, open) = (node.to_owned(), value.value.clone());
            let name = value
                .value
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&value.value)
                .to_owned();
            out = out.child(pane::line(
                t,
                value.name.clone(),
                div()
                    .flex()
                    .gap(px(size::HEAD_GAP))
                    .min_w_0()
                    .child(
                        div()
                            .id(("run-input", index))
                            .flex_none()
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .tooltip(kit::tip("Open it beside the run"))
                            .child(name)
                            .on_click(
                                cx.listener(move |p, _, _, cx| p.open_beside(&owner, &open, cx)),
                            ),
                    )
                    .child(pane::cell(pane::middle(&value.value, PATH_CHARS)).text_color(t.dim)),
            ));
        }
        let (name, path) = if !run.repository.is_empty() {
            ("Repository", &run.repository)
        } else {
            ("Folder", &run.cwd)
        };
        if !path.is_empty() {
            let (owner, open) = (node.to_owned(), path.clone());
            out = out.child(pane::line(
                t,
                name,
                div()
                    .id("run-folder")
                    .debug_selector(|| "run-folder".into())
                    .cursor_pointer()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .hover(|s| s.underline())
                    .tooltip(kit::tip("Open in Files"))
                    .child(path.clone())
                    .on_click(cx.listener(move |p, _, _, cx| p.open_beside(&owner, &open, cx))),
            ));
        }
        if let Some(branch) = words::run_branch(run) {
            out = out.child(pane::line(t, "Branch", branch));
        }
        if !run.work.is_empty() {
            out = out.child(pane::line(t, "Work", words::work_label(&run.work)));
        }
        out
    }

    /// What a blocked run can do next; Resume is the usual one.
    fn resume_choices(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(size::HEAD_GAP))
            .child(
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
        for (id, label, body) in [
            (
                "run-redispatch",
                "Run the step again",
                json!({"mode":"redispatch", "actor":"human:operator"}),
            ),
            (
                "run-grant",
                "Allow more",
                json!({"mode":"reattach", "grant":true, "actor":"human:operator"}),
            ),
            (
                "run-take-fail",
                "Take the fail route",
                json!({"mode":"reattach", "takeFail":true, "actor":"human:operator"}),
            ),
            (
                "run-pick-best",
                "Pick the best attempt",
                json!({"mode":"reattach", "pickBest":true, "actor":"human:operator"}),
            ),
        ] {
            row = row.child(pane::act(t, id, label).on_click(
                cx.listener(move |p, _, _, cx| p.run_post("/resume".into(), body.clone(), cx)),
            ));
        }
        row
    }

    /// What the run produced: the mission's outputs, then the files its
    /// agents wrote that no output names.
    fn run_result(
        &self,
        t: &Theme,
        run: &RunProjection,
        node: &str,
        place: &str,
        name_of: &dyn Fn(&str) -> String,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let written = self
            .run
            .evidence
            .evidence
            .as_ref()
            .map(|e| e.written_outputs(&run.cwd, &run.outputs))
            .unwrap_or_default();
        if run.outputs.is_empty() && written.is_empty() {
            return None;
        }
        let mut section = pane::section(t, "Result").debug_selector(|| "run-result".into());
        for output in &run.outputs {
            section = section.child(self.output(t, run, output, node, place, name_of, cx));
        }
        for output in &written {
            section = section.child(self.output(t, run, output, node, place, name_of, cx));
        }
        Some(section)
    }

    /// One thing the run produced: a text reads in place; a file is a row
    /// that opens beside the run, with Send at its end.
    #[allow(clippy::too_many_arguments)]
    fn output(
        &self,
        t: &Theme,
        run: &RunProjection,
        output: &RunOutput,
        node: &str,
        place: &str,
        name_of: &dyn Fn(&str) -> String,
        cx: &mut Context<Self>,
    ) -> Div {
        let reference = format!(
            "{}: {} from run {} (mission {}) on {}",
            output.name, output.value, run.run_id, run.mission_slug, place
        );
        let send = pane::send(
            t,
            SharedString::from(format!("send-output-{}", output.name)),
            &self.host,
            reference,
        );
        let from = if output.node_id.is_empty() {
            String::new()
        } else {
            format!("from {}", name_of(&output.node_id))
        };
        if matches!(output.kind.as_str(), "file" | "folder") {
            let (owner, path) = (node.to_owned(), output.value.clone());
            let selector = format!("output-{}", output.name);
            let name = words::output_excerpt(output);
            return div()
                .flex()
                .items_center()
                .gap(px(size::HEAD_GAP))
                .min_h(px(size::ROW))
                .child(
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .flex_none()
                        .cursor_pointer()
                        .text_color(t.text)
                        .hover(|s| s.underline())
                        .tooltip(kit::tip("Open it beside the run"))
                        .child(name)
                        .on_click(cx.listener(move |p, _, _, cx| p.open_beside(&owner, &path, cx))),
                )
                .child(pane::cell(pane::middle(&output.value, PATH_CHARS)).text_color(t.dim))
                .child(pane::meta(t, from, None))
                .child(send);
        }
        div()
            .flex()
            .flex_col()
            .pb(px(size::HEAD_GAP))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(size::HEAD_GAP))
                    .min_h(px(size::ROW))
                    .child(pane::cell(output.name.clone()).text_color(t.text2))
                    .child(pane::meta(t, from, None))
                    .child(send),
            )
            .child(
                div()
                    .debug_selector({
                        let name = output.name.clone();
                        move || format!("output-text-{name}")
                    })
                    .text_color(t.text)
                    .child(output.value.clone()),
            )
    }

    /// Each step in the order it runs: who does it, how it stands, its seat
    /// one click away, its brief, and the files it wrote.
    fn run_steps(
        &self,
        t: &Theme,
        run: &RunProjection,
        mission: &Mission,
        node: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = run_state::project(&run.events);
        let evidence = self.run.evidence.evidence.as_ref();
        let seats = crate::store::seats::seats(run);
        let mut rows = Vec::new();
        for step in flow::build(mission).sections.iter().flat_map(|s| &s.steps) {
            let status = state.states.get(&step.id).copied();
            let word = state_word(
                status,
                run.waiting_gates.iter().any(|g| g.gate_id == step.id),
            );
            let selector = format!("run-step-{}-{}", step.id, word.replace(' ', "-"));
            let who = mission
                .formations
                .iter()
                .find(|f| f.id == step.id)
                .map(|f| {
                    f.slots
                        .iter()
                        .map(words::who)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| match &step.kind {
                    flow::StepKind::Gate { .. } => "gate".into(),
                    _ => String::new(),
                });
            let mut name =
                div()
                    .debug_selector(move || selector.clone())
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .py(px(size::LABEL_BOTTOM))
                    .child({
                        let revealed = self.run.reveal_step.as_deref() == Some(step.id.as_str());
                        let marker = format!("run-reveal-{}", step.id);
                        pane::cell(format!("{}  {}", step.number, mission.node_name(&step.id)))
                            .when(revealed, |d| {
                                d.font_weight(FontWeight::SEMIBOLD)
                                    .debug_selector(move || marker.clone())
                            })
                    });
            // A gate's answer, and the words given with it.
            if let Some(event) = run.events.iter().rfind(|e| {
                e.gate_id == step.id
                    && matches!(e.kind.as_str(), "human_verdict_recorded" | "gate_verdict")
                    && !e.verdict.is_empty()
            }) {
                let word = match event.verdict.as_str() {
                    "pass" => "passed",
                    "fail" => "sent back",
                    other => other,
                };
                // The engine's verdict follows the person's answer; the
                // words came with the answer.
                let detail = if event.detail.is_empty() {
                    run.events
                        .iter()
                        .rfind(|e| {
                            e.kind == "human_verdict_recorded"
                                && e.gate_id == step.id
                                && e.seq <= event.seq
                        })
                        .map(|e| e.detail.clone())
                        .unwrap_or_default()
                } else {
                    event.detail.clone()
                };
                let line = if detail.is_empty() {
                    word.to_owned()
                } else {
                    format!("{word}: {detail}")
                };
                let selector = format!("run-verdict-{detail}");
                name = name.child(
                    div()
                        .debug_selector(move || selector.clone())
                        .text_size(px(size::SMALL))
                        .text_color(t.text2)
                        .child(line),
                );
            }
            // What last went wrong at the step, such as a check that sent
            // its work back, in plain words under its name.
            if let Some(problem) = evidence
                .into_iter()
                .flat_map(|e| &e.problems)
                .rfind(|p| p.node_id == step.id || p.gate_id == step.id)
            {
                let reason = parvi_missions_contract::plain_reason(&run_state::problem_now(
                    &run.events,
                    problem.seq,
                    &problem.reason.text,
                ));
                // Under the step's own name the reason need not name it again.
                let reason = without_name(&reason, mission.node_name(&step.id));
                let selector = format!("run-problem-{}-{}", problem.seq, problem.reason.text);
                let colour = if status == Some(run_state::NodeState::Failed) {
                    t.error
                } else {
                    t.dim
                };
                name = name.child(
                    div()
                        .debug_selector(move || selector.clone())
                        .text_size(px(size::SMALL))
                        .text_color(colour)
                        .child(reason),
                );
            }
            let files: Vec<_> = evidence
                .into_iter()
                .flat_map(|e| e.files.iter().enumerate())
                .filter(|(_, file)| file.node_id == step.id)
                .collect();
            if !files.is_empty() {
                let mut line = div().flex().flex_wrap().gap(px(size::HEAD_GAP));
                for (index, file) in files {
                    line = line.child(self.file_link(t, index, file, node, cx));
                }
                name = name.child(line);
            }
            let mut acts = div().flex().justify_end().gap(px(size::HEAD_GAP));
            for seat in seats.iter().filter(|seat| seat.step == step.id) {
                acts =
                    acts.children(self.seat_control(t, seat.clone(), &format!("flow-{}", step.id)));
            }
            if let Some(count) = state.attempts.get(&step.id).copied().filter(|n| *n > 1) {
                let step_id = step.id.clone();
                let selector = format!("run-step-attempts-{step_id}");
                acts = acts.child(
                    pane::act(
                        t,
                        SharedString::from(selector.clone()),
                        format!("{count} tries"),
                    )
                    .debug_selector(move || selector.clone())
                    .tooltip(kit::tip("What each attempt made"))
                    .on_click(cx.listener(move |page, _, window, cx| {
                        page.open_canvas_attempts(&step_id, window, cx)
                    })),
                );
            }
            if let Some(brief) = evidence
                .into_iter()
                .flat_map(|e| &e.briefs)
                .rfind(|brief| brief.node_id == step.id)
            {
                let selector = format!("run-brief-{}", brief.dispatch_seq);
                let (path, owner) = (brief.path.clone(), node.to_owned());
                acts = acts.child(
                    pane::act(t, SharedString::from(selector.clone()), "Brief")
                        .debug_selector(move || selector.clone())
                        .tooltip(kit::tip("What its agent was told"))
                        .on_click(cx.listener(move |p, _, _, cx| p.open_beside(&owner, &path, cx))),
                );
            }
            rows.push(
                pane::table_row(
                    t,
                    &STEPS,
                    vec![
                        name.into_any_element(),
                        pane::cell(who).text_color(t.text2).into_any_element(),
                        pane::cell(word)
                            .text_color(state_colour(t, status, word))
                            .into_any_element(),
                        acts.into_any_element(),
                    ],
                )
                .into_any_element(),
            );
        }
        pane::section(t, "Steps").child(pane::table(t, &STEPS, rows))
    }

    /// A file a step wrote: its name, opening beside the run.
    fn file_link(
        &self,
        t: &Theme,
        index: usize,
        file: &crate::store::evidence::File,
        node: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let selector = format!("run-file-{index}");
        let (path, owner) = (file.path.clone(), node.to_owned());
        let name = file
            .path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&file.path)
            .to_owned();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .cursor_pointer()
            .text_size(px(size::SMALL))
            .text_color(t.text2)
            .hover(|s| s.underline())
            .tooltip(kit::tip(format!(
                "{} · {}",
                file.path,
                words::size_text(file.size)
            )))
            .child(name)
            .on_click(cx.listener(move |p, _, _, cx| p.open_beside(&owner, &path, cx)))
    }

    /// What a step's seat offers: its session, while it lives. A closed
    /// seat offers nothing.
    pub(super) fn seat_control(
        &self,
        t: &Theme,
        seat: crate::store::seats::Seat,
        scope: &str,
    ) -> Option<gpui::Stateful<Div>> {
        use crate::store::seats::State;
        if seat.state != State::Live {
            return None;
        }
        let selector = format!("{scope}-seat-{}-{}-live-{}", seat.run, seat.seq, seat.name);
        let host: Host = self.host.clone();
        Some(
            pane::act(t, SharedString::from(selector.clone()), "Session")
                .debug_selector(move || selector.clone())
                .tooltip(kit::tip(format!("Show {} in Terminal", seat.name)))
                .on_click(move |_, _, cx| {
                    if let Err(error) = host.ask(
                        Ask::Reveal {
                            what: "session".into(),
                            id: seat.session.clone(),
                            node: Some(NodeId(seat.node.clone())),
                            part: None,
                            beside: false,
                        },
                        cx,
                    ) {
                        host.report(
                            parvi_app_kit::host::Status::failure(format!(
                                "Not shown: {}",
                                error.message
                            )),
                            cx,
                        );
                    }
                }),
        )
    }
}

/// The run's standing in plain words and its state's colour: running and
/// where, done and when, or failed where and why.
fn standing(
    page: &MissionsPage,
    t: &Theme,
    run: &RunProjection,
    name_of: &dyn Fn(&str) -> String,
    now: time::OffsetDateTime,
) -> (String, gpui::Hsla) {
    let points = run_state::points(run);
    let phrases: Vec<String> = points
        .iter()
        .map(|point| run_state::phrase(point, &name_of(&point.node)))
        .filter(|p| !p.is_empty())
        .collect();
    let span = words::run_span(run, now);
    match run.status.as_str() {
        "succeeded" => (
            format!(
                "Done at {} · took {span}",
                words::short_time(&run.updated_at, now)
            ),
            t.text,
        ),
        "failed" | "canceled" => {
            let lead = phrases.first().cloned().unwrap_or_else(|| {
                if run.status == "failed" {
                    "failed".into()
                } else {
                    "stopped".into()
                }
            });
            // A reason that names its step says it all: "Write tests ran out
            // of time (8 s)"; one that does not follows where it stopped.
            // A stop says why when it was given a reason of its own.
            let stopped_for = run
                .events
                .iter()
                .rfind(|e| e.kind == "run_canceled")
                .map(|e| e.detail.trim())
                .filter(|why| !why.is_empty() && *why != "operator abort")
                .map(|why| (sentence(why).trim_end_matches('.').to_owned(), false));
            let reason = if run.status == "canceled" {
                stopped_for
            } else {
                page.last_reason(run, name_of)
            };
            let words = match reason {
                Some((reason, true)) => reason,
                Some((reason, false)) => {
                    format!("{}: {reason}", sentence(&lead).trim_end_matches('.'))
                }
                None => sentence(&lead),
            };
            let colour = if run.status == "failed" {
                t.error
            } else {
                t.text2
            };
            (words, colour)
        }
        "blocked" | "waiting_human" => (
            format!(
                "{} · {span}",
                sentence(&phrases.join("; ")).trim_end_matches('.')
            ),
            rgb(t.ansi[3]).into(),
        ),
        _ if phrases.is_empty() => (format!("Starting · {span}"), t.accent),
        _ => (
            format!(
                "{} · {span}",
                sentence(&phrases.join("; ")).trim_end_matches('.')
            ),
            t.accent,
        ),
    }
}

impl MissionsPage {
    /// Why a failed or stopped run ended, in plain words, and whether
    /// those words begin with the step they are about.
    fn last_reason(
        &self,
        run: &RunProjection,
        name_of: &dyn Fn(&str) -> String,
    ) -> Option<(String, bool)> {
        let problem = self.run.evidence.evidence.as_ref()?.problems.last()?;
        let reason = parvi_missions_contract::plain_reason(&run_state::problem_now(
            &run.events,
            problem.seq,
            &problem.reason.text,
        ));
        let name = name_of(&problem.node_id);
        let named = !name.is_empty() && reason.starts_with(&format!("{name} "));
        (!reason.is_empty()).then_some((reason, named))
    }
}

/// `reason` without `name` in front, its first word made a capital:
/// "Write tests ran out of time" under Write tests reads "Ran out of time".
fn without_name(reason: &str, name: &str) -> String {
    let Some(rest) = reason
        .strip_prefix(name)
        .filter(|rest| !name.is_empty() && rest.starts_with(' '))
    else {
        return reason.to_owned();
    };
    let rest = rest.trim_start();
    let mut chars = rest.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The value a next mission starts from: the run's first file, else its
/// first text.
fn primary_output(run: &RunProjection) -> Option<String> {
    run.outputs
        .iter()
        .find(|o| matches!(o.kind.as_str(), "file" | "folder"))
        .or_else(|| run.outputs.first())
        .map(|o| o.value.clone())
        .filter(|v| !v.is_empty())
}

fn state_word(state: Option<run_state::NodeState>, asks_you: bool) -> &'static str {
    use run_state::NodeState;
    if asks_you {
        return "needs you";
    }
    match state {
        Some(NodeState::Running) => "running",
        Some(NodeState::Done) => "done",
        Some(NodeState::Blocked) => "blocked",
        Some(NodeState::Waiting) => "waiting",
        Some(NodeState::Failed) => "failed",
        Some(NodeState::Canceled) => "stopped",
        None => "not yet",
    }
}

/// Colour means state: running, failed, needs you. Done and not yet are
/// quiet.
fn state_colour(t: &Theme, state: Option<run_state::NodeState>, word: &str) -> gpui::Hsla {
    use run_state::NodeState;
    if word == "needs you" {
        return rgb(t.ansi[3]).into();
    }
    match state {
        Some(NodeState::Running) => t.accent,
        Some(NodeState::Failed) => t.error,
        Some(NodeState::Blocked | NodeState::Waiting) => rgb(t.ansi[3]).into(),
        Some(NodeState::Done) => t.text2,
        Some(NodeState::Canceled) | None => t.dim,
    }
}

impl MissionsPage {
    /// Each gate waiting on the operator: what it asks, what it was handed,
    /// and its answers.
    fn run_gates(
        &self,
        t: &Theme,
        run: &RunProjection,
        mission: Option<&Mission>,
        place: &str,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let mut out = Vec::new();
        for gate in &run.waiting_gates {
            let gate_id = gate.gate_id.clone();
            let seq = gate.requested_seq;
            let gate_title = mission
                .map(|m| m.node_title(&gate_id))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Review")
                .to_owned();
            let mut row = pane::section(t, format!("Needs you · {gate_title}"))
                .debug_selector(|| "run-gate".into())
                .gap(px(size::HEAD_GAP))
                .child(div().debug_selector({
                    let title = gate_title.clone();
                    move || format!("gate-title-{title}")
                }));
            if let Some((_, Ok(Some(request)))) = self.run.gates.get(&gate_id) {
                let source = mission
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
                                .text_color(t.text)
                                .child(question),
                        ),
                    )
                    .child(
                        pane::meta(t, format!("From {source}"), None)
                            .debug_selector(move || format!("gate-source-{source}")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("gate-input-{seq}")))
                            .debug_selector(|| "gate-input-quote".into())
                            .max_h(px(size::NOTICE))
                            .overflow_y_scroll()
                            .pl(px(size::ROW_PAD))
                            .border_l_1()
                            .border_color(t.divider)
                            .text_color(t.text2)
                            .child(quote),
                    );
                if request.input.truncated {
                    row = row.child(pane::meta(t, "Showing the start of a long input.", None));
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
                        div().flex().child(
                            pane::act(
                                t,
                                SharedString::from(format!("gate-retry-{gate_id}")),
                                "Try again",
                            )
                            .debug_selector(|| "gate-retry".into())
                            .on_click(cx.listener(
                                move |page, _, _, cx| {
                                    let response = page.run.responses.get(&retry_gate).cloned();
                                    page.run.gates.remove(&retry_gate);
                                    page.sync_gate(cx);
                                    if let Some(response) = response {
                                        page.run.responses.insert(retry_gate.clone(), response);
                                    }
                                },
                            )),
                        ),
                    );
            } else {
                row = row.child(pane::meta(t, "Reading what it was handed…", None));
            }
            if let Some(response) = self.run.responses.get(&gate_id) {
                row = row.child(
                    div()
                        .debug_selector(|| "gate-response".into())
                        .child(response.clone()),
                );
            }
            let mut approve_label = "Approve".to_owned();
            let mut reject_label = "Send back".to_owned();
            let mut routes = Vec::new();
            if let Some(mission) = mission
                && let Some(flow::Step {
                    kind: flow::StepKind::Gate { pass, fail, .. },
                    ..
                }) = flow::build(mission)
                    .sections
                    .iter()
                    .flat_map(|s| &s.steps)
                    .find(|s| s.id == gate_id)
            {
                let alone = !run_state::project(&run.events)
                    .states
                    .iter()
                    .any(|(id, state)| {
                        id != &gate_id
                            && matches!(
                                state,
                                run_state::NodeState::Running | run_state::NodeState::Waiting
                            )
                    });
                let (approve_words, approve) = crate::store::gate::route(pass, "Approve", alone);
                let (reject_words, reject) = crate::store::gate::route(fail, "Send back", alone);
                approve_label = approve;
                reject_label = reject;
                routes.push(approve_words);
                routes.push(reject_words);
            }
            let mut actions = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(size::HEAD_GAP));
            if let Some(pick) = &gate.pick {
                for line in crate::store::gate::picking_words(pick, "") {
                    if !line.is_empty() {
                        row = row.child(div().text_color(t.text2).child(line));
                    }
                }
                if let Some(step) = pick.get("nodeId").and_then(|v| v.as_str()) {
                    let step = step.to_owned();
                    actions = actions.child(
                        pane::act(
                            t,
                            SharedString::from(format!("gate-compare-{gate_id}")),
                            "Compare attempts",
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
                if let Some(candidates) = pick.get("candidates").and_then(|v| v.as_array()) {
                    for candidate in candidates.iter().filter_map(|v| v.as_i64()) {
                        let gate = gate_id.clone();
                        actions = actions.child(
                            kit::button(
                                t,
                                (
                                    SharedString::from(format!("pick-{gate_id}")),
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
                for line in routes {
                    row = row.child(div().text_color(t.text2).child(line));
                }
                for (word, verdict, kind) in [
                    (approve_label, "pass", Kind::Primary),
                    (reject_label, "fail", Kind::Plain),
                ] {
                    let gate = gate_id.clone();
                    actions = actions.child(
                        kit::button(
                            t,
                            SharedString::from(format!("gate-{gate_id}-{verdict}")),
                            word,
                            kind,
                        )
                        .debug_selector(move || format!("gate-{verdict}"))
                        .on_click(
                            cx.listener(move |p, _, _, cx| p.verdict(&gate, seq, verdict, cx)),
                        ),
                    );
                }
            }
            actions = actions.child(pane::send(
                t,
                SharedString::from(format!("gate-send-{gate_id}")),
                &self.host,
                crate::store::words::gate_reference(run, gate, place),
            ));
            out.push(row.child(actions));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reason_under_its_step_drops_the_steps_name() {
        assert_eq!(
            super::without_name("Write tests ran out of time (8 s)", "Write tests"),
            "Ran out of time (8 s)"
        );
        // Only a whole name comes off, and a reason about something else stays.
        assert_eq!(
            super::without_name("Writers stopped", "Write"),
            "Writers stopped"
        );
        assert_eq!(
            super::without_name("The agent stopped", "Write tests"),
            "The agent stopped"
        );
    }
}
