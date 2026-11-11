//! Render the complete, private instructions a seat reads by pointer.
use crate::FormationExecution;
use parvi_mission_model::Slot;
use std::fmt::Write;

pub(super) fn render(request: &FormationExecution, slot: &Slot) -> String {
    render_for(request, slot, true)
}

/// A worker has the same context, but the controller owns the final output.
pub(super) fn render_worker(request: &FormationExecution, slot: &Slot) -> String {
    let mut brief = render_for(request, slot, false);
    let _ = writeln!(
        brief,
        "You are a worker on this team. Work on the controller's assigned task, save your output in the attempt directory as worker-{}.md, and report its path and findings to the controller. The controller owns the formation's final output.",
        slot.id
    );
    brief
}

fn render_for(request: &FormationExecution, slot: &Slot, final_output: bool) -> String {
    let mut brief = String::new();
    let _ = writeln!(
        brief,
        "Run: {}\nStep: {} ({})\nSlot: {}\nAgent: {}\nHarness: {}\nWorking directory: {}\nAttempt directory: {}\n",
        request.run_id,
        request.formation.title,
        request.formation.id,
        slot.id,
        if slot.agent_id.is_empty() {
            "vanilla (no role)"
        } else {
            &slot.agent_id
        },
        slot.harness,
        request.cwd.display(),
        request.artifact_dir.display()
    );
    if let Some(turn) = &request.turn {
        let _ = writeln!(brief, "Starting commit: {}", turn.commit);
    }
    if let Ok(work) = parvi_work_ref::parse(&request.work) {
        let _ = writeln!(brief, "Mission {}: {}", work.noun(), work.describe());
    }
    if let Some(details) = &request.formation.brief {
        let _ = writeln!(brief, "Goal:\n{}\n", details.goal);
        if let Ok(work) = parvi_work_ref::parse(&details.work) {
            let _ = writeln!(brief, "Brief {}: {}", work.noun(), work.describe());
        }
        for file in &details.files {
            let _ = writeln!(brief, "Reference file: {file}");
        }
        for link in &details.links {
            let _ = writeln!(brief, "Reference link: {link}");
        }
    }
    if let Some(binding) = request
        .bindings
        .bindings
        .iter()
        .find(|binding| binding.node_id == request.formation.id && binding.slot_id == slot.id)
        && !binding.card_toml.is_empty()
    {
        let _ = writeln!(brief, "\nRole card:\n{}", binding.card_toml);
    }
    for path in &request.context_paths {
        let _ = writeln!(brief, "Context path: {}", path.display());
    }
    let _ = writeln!(
        brief,
        "\nRouted input and feedback:\n{}\n",
        request.input_text
    );
    if final_output && !request.formation.outputs.is_empty() {
        brief.push_str("Return all and only these output ports in one fenced `parvi-outputs` JSON block. Each port maps to an object with `text`; file and folder outputs must also carry an absolute `ref` path.\n");
        for port in &request.formation.outputs {
            let _ = writeln!(brief, "- {} ({})", port.id, port.kind());
        }
        let example: serde_json::Map<String, serde_json::Value> = request
            .formation
            .outputs
            .iter()
            .map(|port| {
                let payload = if port.kind() == "file" || port.kind() == "folder" {
                    serde_json::json!({"text":"result", "ref":"/absolute/path/to/artifact"})
                } else {
                    serde_json::json!({"text":"result"})
                };
                (port.id.clone(), payload)
            })
            .collect();
        let _ = writeln!(
            brief,
            "Example shape (replace values):\n```parvi-outputs\n{}\n```",
            serde_json::Value::Object(example)
        );
    }
    if final_output && !request.pick_attempts.is_empty() {
        brief.push_str(&crate::run_pick::contract(
            request.judge,
            &request.pick_attempts,
        ));
    } else if final_output && request.judge {
        brief.push_str("Return exactly one fenced `parvi-verdict` block containing JSON with exactly `verdict` (pass or fail), `reason` (string), and `evidence` (array of strings).\n");
    }
    let _ = writeln!(
        brief,
        "\nWhen finished, end your response with <<<PARVI-DONE run-id={} status=ok artifact=<path-or-ref>>>.",
        request.run_id
    );
    brief
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BindingsSnapshot;
    use parvi_mission_model::{Brief, Formation};

    fn execution(run_work: &str, brief_work: &str) -> FormationExecution {
        FormationExecution {
            run_id: "run_1".into(),
            formation: Formation {
                id: "fmn_work".into(),
                title: "Work".into(),
                brief: Some(Brief {
                    goal: "Ship".into(),
                    work: brief_work.into(),
                    ..Brief::default()
                }),
                ..Formation::default()
            },
            bindings: BindingsSnapshot {
                schema: 1,
                run_id: "run_1".into(),
                mission_id: String::new(),
                mission_slug: String::new(),
                mission_rev: 0,
                input_card_id: String::new(),
                bindings: Vec::new(),
                gate_bindings: Vec::new(),
            },
            cwd: "/w".into(),
            turn: None,
            artifact_dir: "/a".into(),
            context_paths: Vec::new(),
            work: run_work.into(),
            inputs: Vec::new(),
            input_text: String::new(),
            attempt: 1,
            judge: false,
            pick_attempts: Vec::new(),
            canceled: Default::default(),
            stopping: Default::default(),
            limits: Default::default(),
        }
    }

    /// Archon's brief names the mission bead and the brief bead; Parvi says
    /// bead or issue by tracker, with a Bead's store so bd runs in it.
    #[test]
    fn names_the_runs_work_item_and_the_briefs_own() {
        let text = render(
            &execution("bead:n1:/srv/x#x-1", "gh:a/b#2"),
            &Slot::default(),
        );
        assert!(
            text.contains("Mission bead: x-1 (store: /srv/x)\n"),
            "{text}"
        );
        assert!(text.contains("Brief issue: a/b#2\n"), "{text}");
        let text = render(&execution("gh:a/b#2", "x-9"), &Slot::default());
        assert!(text.contains("Mission issue: a/b#2\n"), "{text}");
        assert!(text.contains("Brief bead: x-9\n"), "{text}");
        let text = render(&execution("", ""), &Slot::default());
        assert!(
            !text.contains("Mission ") && !text.contains("Brief "),
            "{text}"
        );
    }
}
