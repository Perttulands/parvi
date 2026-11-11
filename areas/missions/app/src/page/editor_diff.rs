//! Three-way editor save: only fields changed by the operator are written,
//! and a simultaneous edit of the same field is refused.
use super::*;
use parvi_missions_contract::{Mission, MissionOutput};

fn changed<T: PartialEq>(
    name: &str,
    base: &T,
    draft: &T,
    now: &T,
    problems: &mut Vec<String>,
) -> bool {
    if draft == base {
        return false;
    }
    if now != base {
        problems.push(format!("{name} was changed elsewhere while this window was open. Close it and open the card again to see the change."));
        false
    } else {
        true
    }
}
fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}
fn fields(snapshot: &EditorSnapshot, index: usize) -> &String {
    &snapshot.fields[index]
}
fn number(value: &str, name: &str) -> Result<i64, String> {
    if value.trim().is_empty() {
        return Ok(0);
    }
    value
        .trim()
        .parse()
        .map_err(|_| format!("{name} must be a whole number"))
}
fn output_values(outputs: &[MissionOutput]) -> Vec<(String, String, String, bool, String)> {
    outputs
        .iter()
        .map(|o| {
            (
                o.name.clone(),
                o.description.clone(),
                o.kind.clone(),
                o.required,
                o.from.clone(),
            )
        })
        .collect()
}

/// The full set of changed fields, with conflicts found before any write.
#[allow(clippy::too_many_arguments)]
fn operations(
    kind: EditKind,
    id: &str,
    slot: Option<&str>,
    port: Option<&str>,
    limit: Option<&str>,
    base: &EditorSnapshot,
    draft: &EditorSnapshot,
    now: &Mission,
) -> Result<Vec<undo::Step>, Vec<String>> {
    let mut ops = Vec::new();
    let mut problems = Vec::new();
    macro_rules! compare {
        ($index:expr, $name:expr, $current:expr) => {
            changed(
                $name,
                fields(base, $index),
                fields(draft, $index),
                &$current.to_string(),
                &mut problems,
            )
        };
    }
    match kind {
        EditKind::Step => {
            let Some(f) = now.formations.iter().find(|f| f.id == id) else {
                return Err(vec!["The step is gone: it was deleted elsewhere.".into()]);
            };
            let original = f.brief.clone().unwrap_or_default();
            let mut brief = original.clone();
            let title_changed = compare!(0, "The title", &f.title);
            let mut brief_changed = false;
            if compare!(1, "The brief", &original.goal) {
                brief.goal = draft.fields[1].clone();
                brief_changed = true;
            }
            if compare!(3, "The repository", &original.repository) {
                brief.repository = draft.fields[3].clone();
                brief_changed = true;
            }
            if compare!(4, "The work reference", &original.work) {
                brief.work = draft.fields[4].clone();
                brief_changed = true;
            }
            if compare!(5, "The files", &original.files.join("\n")) {
                brief.files = lines(&draft.fields[5]);
                brief_changed = true;
            }
            if compare!(6, "The links", &original.links.join("\n")) {
                brief.links = lines(&draft.fields[6]);
                brief_changed = true;
            }
            if title_changed || brief_changed {
                ops.push(undo::Step::Board(json!({"editFormation":{"id":id,"title":if title_changed {&draft.fields[0]} else {&f.title},"brief":brief}})));
            }
            if compare!(2, "Where the step is placed", &f.device) {
                ops.push(undo::Step::Board(
                    json!({"updateFormation":{"id":id,"device":draft.fields[2]}}),
                ));
            }
            if let Some(slot) = slot.filter(|_| base.fields[10..14] != draft.fields[10..14]) {
                let Some(current) = f.slots.iter().find(|s| s.id == slot) else {
                    return Err(vec![
                        "The selected slot is gone: it was deleted elsewhere.".into(),
                    ]);
                };
                let current = [
                    &current.agent_id,
                    &current.harness,
                    &current.model,
                    &current.effort,
                ];
                let mut update = false;
                for (index, name) in [
                    (10, "The slot's role"),
                    (11, "The slot's harness"),
                    (12, "The slot's model"),
                    (13, "The slot's effort"),
                ] {
                    update |= compare!(index, name, current[index - 10]);
                }
                if update {
                    let result: Vec<_> = (10..14)
                        .map(|index| {
                            if draft.fields[index] != base.fields[index] {
                                draft.fields[index].clone()
                            } else {
                                (*current[index - 10]).clone()
                            }
                        })
                        .collect();
                    ops.push(undo::Step::Board(json!({"assignSlot":{"formationId":id,"slotId":slot,"agentId":result[0],"harness":result[1],"model":result[2],"effort":result[3]}})));
                }
            }
            if let Some(port) = port.filter(|_| base.fields[14..16] != draft.fields[14..16]) {
                let Some(current) = f.inputs.iter().chain(&f.outputs).find(|p| p.id == port) else {
                    return Err(vec![
                        "The selected port is gone: it was deleted elsewhere.".into(),
                    ]);
                };
                let label = compare!(14, "The port label", &current.label);
                let kind = compare!(15, "The port kind", &current.kind);
                if label || kind {
                    let mut fields = json!({"formationId":id,"portId":port});
                    if label {
                        fields["label"] = json!(draft.fields[14]);
                    }
                    if kind {
                        fields["kind"] = json!(draft.fields[15]);
                    }
                    ops.push(undo::Step::Board(json!({"updatePort":fields})));
                }
            }
        }
        EditKind::Input => {
            let Some(card) = now.input_cards.iter().find(|c| c.id == id) else {
                return Err(vec![
                    "The Input card is gone: it was deleted elsewhere.".into(),
                ]);
            };
            let mut fields = json!({"id":id});
            if compare!(0, "The title", &card.title) {
                fields["title"] = json!(draft.fields[0]);
            }
            if compare!(1, "The goal", &card.goal) {
                fields["goal"] = json!(draft.fields[1]);
            }
            if compare!(2, "The human channel", &card.human_channel) {
                fields["humanChannel"] = json!(draft.fields[2]);
            }
            if draft.outputs.iter().any(|o| o.0.trim().is_empty()) {
                problems.push("Every output needs a name.".into());
            }
            if changed(
                "The outputs",
                &base.outputs,
                &draft.outputs,
                &output_values(&card.outputs),
                &mut problems,
            ) {
                let outputs: Vec<_> = draft
                    .outputs
                    .iter()
                    .map(|(name, description, kind, required, from)| MissionOutput {
                        name: name.clone(),
                        description: description.clone(),
                        kind: kind.clone(),
                        required: *required,
                        from: from.clone(),
                    })
                    .collect();
                fields["outputs"] = json!(outputs);
            }
            if fields.as_object().unwrap().len() > 1 {
                ops.push(undo::Step::Board(json!({"updateInputCard":fields})));
            }
        }
        EditKind::Gate => {
            let Some(gate) = now.gates.iter().find(|g| g.id == id) else {
                return Err(vec!["The gate is gone: it was deleted elsewhere.".into()]);
            };
            let mut fields = json!({"id":id});
            for (index, name, key, current) in [
                (0, "The title", "title", gate.title.clone()),
                (1, "The criterion", "criterion", gate.criterion.clone()),
                (2, "Who decides", "kinds", gate.kinds.join(",")),
                (5, "The files", "files", gate.files.join("\n")),
                (7, "The check", "check", gate.check.clone()),
                (
                    8,
                    "The check version",
                    "checkVersion",
                    gate.check_version.clone(),
                ),
                (9, "The check value", "checkValue", gate.check_value.clone()),
            ] {
                if compare!(index, name, &current) {
                    fields[key] = match key {
                        "kinds" => json!(
                            draft.fields[index]
                                .split(',')
                                .map(str::trim)
                                .filter(|v| !v.is_empty())
                                .collect::<Vec<_>>()
                        ),
                        "files" => json!(lines(&draft.fields[index])),
                        _ => json!(draft.fields[index]),
                    };
                }
            }
            if fields.as_object().unwrap().len() > 1 {
                ops.push(undo::Step::Board(json!({"updateGate":fields})));
            }
        }
        EditKind::End => {
            let Some(end) = now.ends.iter().find(|e| e.id == id) else {
                return Err(vec!["The End is gone: it was deleted elsewhere.".into()]);
            };
            let mut fields = json!({"id":id});
            if compare!(0, "The title", &end.title) {
                fields["title"] = json!(draft.fields[0]);
            }
            if compare!(2, "The outcome", &end.outcome) {
                fields["outcome"] = json!(draft.fields[2]);
            }
            if fields.as_object().unwrap().len() > 1 {
                ops.push(undo::Step::Board(json!({"updateEnd":fields})));
            }
        }
    }
    if let Some(id) = limit.filter(|_| base.fields[16..22] != draft.fields[16..22]) {
        let Some(limit) = now.limits.iter().find(|l| l.id == id) else {
            return Err(vec![
                "The selected limit is gone: it was deleted elsewhere.".into(),
            ]);
        };
        let mut fields = json!({"id":id});
        for (index, name, key, current) in [
            (16, "The limit title", "title", limit.title.clone()),
            (
                17,
                "The limit rounds",
                "rounds",
                limit.rounds.map(|n| n.to_string()).unwrap_or_default(),
            ),
            (
                18,
                "The limit time",
                "seconds",
                limit.seconds.map(|n| n.to_string()).unwrap_or_default(),
            ),
            (
                19,
                "The warning time",
                "warnSeconds",
                limit
                    .warn_seconds
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            ),
            (
                20,
                "The token limit",
                "tokens",
                limit.tokens.map(|n| n.to_string()).unwrap_or_default(),
            ),
            (
                21,
                "What the limit does",
                "whenSpent",
                limit.when_spent.clone(),
            ),
        ] {
            if compare!(index, name, &current) {
                fields[key] = if matches!(index, 17..=20) {
                    match number(&draft.fields[index], name) {
                        Ok(value) => json!(value),
                        Err(error) => {
                            problems.push(error);
                            continue;
                        }
                    }
                } else {
                    json!(draft.fields[index])
                };
            }
        }
        if fields.as_object().unwrap().len() > 1 {
            ops.push(undo::Step::Board(json!({"updateLimit":fields})));
        }
    }
    if problems.is_empty() {
        Ok(ops)
    } else {
        Err(problems)
    }
}
impl MissionsPage {
    pub(super) fn save_editor_merged(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        if editor.busy || self.writing {
            return;
        }
        let Some(base) = editor.baseline.clone() else {
            return;
        };
        let draft = self.editor_snapshot(editor, cx);
        let (Some((node, slug)), Some(_)) =
            (self.state.selected.clone(), self.state.mission.as_ref())
        else {
            return;
        };
        let (kind, id) = (editor.kind, editor.id.clone());
        let slot = self.slot.clone();
        let port = self.port.clone();
        let limit = self.limit.clone();
        if let Some(editor) = &mut self.editor {
            editor.busy = true;
            editor.error = None;
        }
        self.writing = true;
        let asked = self
            .host
            .request(Request::get(NodeId(node.clone()), mission_path(&slug)), cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }
                let response = match answer {
                    Ok(answer) if answer.ok() => {
                        serde_json::from_slice::<MissionResponse>(&answer.body)
                            .map_err(|e| e.to_string())
                    }
                    Ok(answer) => Err(answer.error_text()),
                    Err(error) => Err(format!("{}: {}", error.code, error.message)),
                };
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        page.writing = false;
                        if let Some(editor) = &mut page.editor {
                            editor.busy = false;
                            editor.error = Some(error);
                        }
                        cx.notify();
                        return;
                    }
                };
                let ops = operations(
                    kind,
                    &id,
                    slot.as_deref(),
                    port.as_deref(),
                    limit.as_deref(),
                    &base,
                    &draft,
                    &response.mission.mission,
                );
                page.state.mission_etag = response.mission.etag;
                page.state.mission = Some(response.mission.mission);
                match ops {
                    Err(problems) => {
                        page.writing = false;
                        if let Some(editor) = &mut page.editor {
                            editor.busy = false;
                            editor.error = Some(problems.join("; "));
                        }
                        cx.notify();
                    }
                    Ok(steps) if steps.is_empty() => {
                        page.writing = false;
                        let unchanged = page
                            .editor
                            .as_ref()
                            .is_none_or(|editor| page.editor_snapshot(editor, cx) == draft);
                        if unchanged {
                            page.editor = None;
                            page.resolve_wanted(cx);
                        } else if let Some(editor) = &mut page.editor {
                            editor.busy = false;
                        }
                        cx.notify();
                    }
                    Ok(steps) => page.write_editor_steps(node, slug, steps, draft, cx),
                }
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_merge_untouched_fields_and_refuse_the_same_field() {
        let base = EditorSnapshot {
            fields: vec![String::new(); 22],
            outputs: vec![],
        };
        let mut base = base;
        base.fields[0] = "Original title".into();
        base.fields[1] = "Original goal".into();
        let mut draft = base.clone();
        draft.fields[1] = "My goal".into();
        let mut mission = Mission::default();
        mission.formations.push(parvi_missions_contract::Formation {
            id: "work".into(),
            title: "Concurrent title".into(),
            brief: Some(parvi_missions_contract::Brief {
                goal: "Original goal".into(),
                repository: "kept elsewhere".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ops = operations(
            EditKind::Step,
            "work",
            None,
            None,
            None,
            &base,
            &draft,
            &mission,
        )
        .unwrap();
        let undo::Step::Board(patch) = &ops[0] else {
            panic!("board edit")
        };
        assert_eq!(patch["editFormation"]["title"], "Concurrent title");
        assert_eq!(
            patch["editFormation"]["brief"]["repository"],
            "kept elsewhere"
        );
        assert_eq!(patch["editFormation"]["brief"]["goal"], "My goal");
        mission.formations[0].brief.as_mut().unwrap().goal = "Other goal".into();
        assert!(
            operations(
                EditKind::Step,
                "work",
                None,
                None,
                None,
                &base,
                &draft,
                &mission
            )
            .unwrap_err()[0]
                .contains("The brief was changed elsewhere")
        );
    }
}
