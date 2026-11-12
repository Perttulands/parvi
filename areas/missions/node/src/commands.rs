//! Commands use the same store and coordinator as the node's routes.
use crate::Missions;
use parvi_core::Error;
use parvi_mission_engine::{AdmissionFailure, Harnesses, validate};
use parvi_missions_contract::{self as wire, LayoutPatch, NotePatch, Precondition, RunStart};
use parvi_node_kit::{
    Command,
    args::{flag, maybe_text, object, text},
};
use serde_json::{Value, json};
use std::time::Duration;

pub const NAMES: &[&str] = &[
    "mission_list",
    "mission_archives",
    "mission_purge_archive",
    "mission_inspect",
    "mission_validate",
    "mission_new",
    "mission_edit",
    "mission_notes",
    "mission_note",
    "layout_show",
    "layout_edit",
    "mission_run",
    "formation_run",
    "run_list",
    "run_status",
    "run_wait",
    "run_evidence",
    "run_attempts",
    "run_gates",
    "run_seats",
    "run_logs",
    "run_abort",
    "run_resume",
    "run_land",
    "gate_request",
    "gate_approve",
    "gate_reject",
    "gate_pick",
    "mission_arrange",
    "mission_rename",
    "mission_move",
    "mission_create",
    "mission_update",
    "mission_delete",
    "mission_input",
    "mission_output",
    "mission_wire",
    "formation_list",
    "formation_inspect",
    "formation_create",
    "formation_rename",
    "formation_place",
    "formation_set_type",
    "formation_set_slots",
    "formation_assign",
    "formation_unassign",
    "formation_set_brief",
    "formation_add_input",
    "formation_add_output",
    "formation_set_port",
    "formation_wire",
    "formation_unwire",
    "formation_delete",
    "gate_create",
    "gate_update",
    "gate_judge",
    "gate_delete",
    "end_create",
    "end_update",
    "end_delete",
    "limit_create",
    "limit_update",
    "limit_delete",
];

pub fn commands(missions: &Missions) -> Vec<Command> {
    NAMES
        .iter()
        .map(|&name| {
            let m = missions.clone();
            Command::new(
                name,
                description(name),
                move || schema(name),
                move |args| {
                    let m = m.clone();
                    async move { call(m, name, args).await }
                },
            )
        })
        .collect()
}
fn description(name: &str) -> &'static str {
    match name {
        "mission_list" => "List the missions on this node.",
        "mission_archives" => {
            "List archived mission definitions, layouts and notes, including partial archives."
        }
        "mission_purge_archive" => {
            "Permanently remove a listed mission archive's definition, layout and associated notes; unrelated notes and run history remain."
        }
        "mission_inspect" => "Read a mission and its graph.",
        "mission_validate" => "Report errors and warnings on a mission.",
        "mission_new" => "Create an empty mission.",
        "mission_edit" => "Apply one mission authoring operation.",
        "mission_notes" => "Read mission note threads.",
        "mission_note" => "Append, edit or delete a mission note.",
        "layout_show" => "Read a mission's canvas layout.",
        "layout_edit" => "Change or arrange a mission's canvas layout.",
        "mission_run" => "Start a mission at its Input card.",
        "formation_run" => "Start a mission at one formation.",
        "run_list" => "List runs, optionally by mission.",
        "run_status" => "Read a run and its current events.",
        "run_wait" => "Wait for a run to need its driver, finish, or change.",
        "run_evidence" => "Read a run’s problems, briefs and files.",
        "run_attempts" => "Read a run’s step attempts, verdicts and picks.",
        "run_gates" => "List a run’s gates and their state.",
        "gate_request" => "Read a waiting gate’s request and routes.",
        "run_seats" => "Read the seats belonging to a run.",
        "run_logs" => "Read the public events of a run.",
        "run_abort" => "Abort a run.",
        "run_resume" => "Resume a blocked run.",
        "run_land" => "Land a finished repository run.",
        "gate_approve" => "Approve a pending human gate.",
        "gate_reject" => "Reject a pending human gate.",
        "gate_pick" => "Pick an attempt at a pending human gate.",
        _ if crate::authoring::NAMES.contains(&name) => crate::authoring::description(name),
        _ => unreachable!(),
    }
}
fn schema(name: &str) -> Value {
    let s = json!({"type":"string"});
    let i = json!({"type":"integer"});
    let b = json!({"type":"boolean"});
    let obj = json!({"type":"object"});
    match name {
        "mission_list" | "mission_archives" => object(json!({}), &[]),
        "mission_purge_archive" => {
            object(json!({"slug":s,"archive_id":s}), &["slug", "archive_id"])
        }
        "mission_new" => object(json!({"slug":s,"title":s}), &["slug"]),
        "mission_edit" => object(
            json!({"mission":s,"change":obj,"rev":i}),
            &["mission", "change"],
        ),
        "mission_note" => object(
            json!({"mission":s,"target":s,"action":s,"entry":s,"text":s,"author":s,"etag":s}),
            &["mission"],
        ),
        "mission_run" | "formation_run" => {
            let properties = json!({"mission":s,"formation":s,"input":s,"inputs":obj,"cwd":s,"work":s,"repository":s,"base":s,"context_paths":{"type":"array","items":s},"actor":s,"request_id":s});
            if name == "formation_run" {
                object(properties, &["mission", "formation"])
            } else {
                object(properties, &["mission"])
            }
        }
        "layout_edit" => object(
            json!({"mission":s,"change":obj,"etag":s}),
            &["mission", "change"],
        ),
        "mission_inspect" | "mission_validate" | "mission_notes" | "layout_show" => {
            object(json!({"mission":s}), &["mission"])
        }
        "run_list" => object(json!({"mission":s}), &[]),
        "run_evidence" | "run_attempts" => object(json!({"run":s,"step":s}), &["run"]),
        "run_wait" => object(json!({"run":s,"until":s,"since":i,"timeout":i}), &["run"]),
        "run_abort" => object(json!({"run":s,"reason":s,"actor":s}), &["run"]),
        "run_resume" => object(
            json!({"run":s,"mode":s,"take_fail":b,"grant":b,"pick_best":b,"reason":s,"actor":s}),
            &["run"],
        ),
        "run_land" => object(json!({"run":s,"force":b}), &["run"]),
        "gate_request" => object(json!({"run":s,"gate":s}), &["run", "gate"]),
        "gate_approve" | "gate_reject" => object(
            json!({"run":s,"gate":s,"requested_seq":i,"response":s,"relayed_by":s,"actor":s}),
            &["run", "gate", "requested_seq"],
        ),
        "gate_pick" => object(
            json!({"run":s,"gate":s,"requested_seq":i,"attempt":i,"reason":s,"relayed_by":s,"actor":s}),
            &["run", "gate", "requested_seq", "attempt"],
        ),
        _ if crate::authoring::NAMES.contains(&name) => crate::authoring::schema(name),
        _ => object(json!({"run":s}), &["run"]),
    }
}
fn bad(e: impl std::fmt::Display) -> Error {
    Error::new("bad_request", e.to_string())
}
fn required<'a>(a: &'a Value, key: &str) -> Result<&'a str, Error> {
    text(a, key).map_err(bad).and_then(|v| {
        if v.is_empty() {
            Err(bad(format!("{key} is required")))
        } else {
            Ok(v)
        }
    })
}
fn optional<'a>(a: &'a Value, key: &str, default: &'a str) -> Result<&'a str, Error> {
    Ok(maybe_text(a, key).map_err(bad)?.unwrap_or(default))
}
fn command_actor(a: &Value) -> Result<String, Error> {
    let actor = optional(a, "actor", "agent:parvi")?.trim();
    Ok(if actor.is_empty() {
        "agent:parvi"
    } else {
        actor
    }
    .into())
}
fn boolean(a: &Value, key: &str) -> Result<bool, Error> {
    flag(a, key, false).map_err(bad)
}
fn positive(a: &Value, key: &str) -> Result<u64, Error> {
    a.get(key)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| bad(format!("{key} must be positive")))
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|e| Error::new("internal", e.to_string()))?
}
fn value<T: serde::Serialize>(v: T) -> Result<Value, Error> {
    serde_json::to_value(v).map_err(|e| Error::new("internal", e.to_string()))
}
pub(super) fn document(mission: wire::Mission, etag: String) -> Result<Value, Error> {
    value(wire::document::MissionDocument {
        mission,
        etag: Some(etag),
    })
}
fn notes(edit: wire::NotesEdit) -> Result<Value, Error> {
    value(wire::document::NotesDocument::from_notes(
        edit.notes, edit.etag,
    ))
}
pub(super) fn layout(edit: wire::LayoutEdit) -> Result<Value, Error> {
    value(wire::document::LayoutDocument {
        layout: edit.layout,
        etag: edit.etag,
    })
}
fn admission(e: AdmissionFailure) -> Error {
    match e {
        AdmissionFailure::NotFound(s) => Error::new("not_found", s),
        AdmissionFailure::Conflict => Error::new("conflict", "mission changed"),
        AdmissionFailure::Invalid(s) => bad(s),
        AdmissionFailure::Io(e) => Error::new("io", e.to_string()),
        AdmissionFailure::Findings(findings) => Error::new(
            "run_admission_failed",
            format!(
                "run admission failed: {}",
                serde_json::to_string(&findings).unwrap_or_default()
            ),
        ),
    }
}
async fn call(m: Missions, name: &str, a: Value) -> Result<Value, Error> {
    if !a.is_object() {
        return Err(bad("arguments must be an object"));
    }
    let node = &m.node;
    match name {
        "mission_list" => {
            if a.as_object().is_some_and(|v| !v.is_empty()) {
                return Err(bad("no arguments expected"));
            }
            Ok(json!({"node":node,"missions":m.store.list_missions()?}))
        }
        "mission_archives" => {
            if a.as_object().is_some_and(|v| !v.is_empty()) {
                return Err(bad("no arguments expected"));
            }
            Ok(json!({"node":node,"archives":m.store.mission_archives()?}))
        }
        "mission_purge_archive" => {
            let removed = m
                .store
                .remove_mission_archive(required(&a, "slug")?, required(&a, "archive_id")?)?;
            Ok(json!({"node":node,"removed":removed}))
        }
        "mission_new" => {
            let slug = required(&a, "slug")?;
            let edit = m
                .store
                .create_mission(slug, optional(&a, "title", slug)?, "agent:parvi")?;
            m.changed();
            Ok(json!({"node":node,"mission":document(edit.mission,edit.etag)?}))
        }
        "mission_inspect" => {
            let edit = m.store.mission_edit(required(&a, "mission")?)?;
            Ok(json!({"node":node,"mission":document(edit.mission,edit.etag)?}))
        }
        "mission_validate" => {
            let edit = m.store.mission_edit(required(&a, "mission")?)?;
            let report = validate(&edit.mission, &m.store.roles(), &Harnesses::from_host());
            Ok(
                json!({"node":node,"missionRev":edit.mission.rev,"missionEtag":edit.etag,"errors":report.errors,"warnings":report.warnings}),
            )
        }
        "mission_edit" => {
            let id = required(&a, "mission")?;
            let current = m.store.mission_edit(id)?;
            if let Some(rev) = a.get("rev").filter(|value| !value.is_null())
                && rev.as_i64() != Some(current.mission.rev)
            {
                return Err(Error::new("conflict", "mission changed"));
            }
            let change = a
                .get("change")
                .filter(|v| v.is_object())
                .ok_or_else(|| bad("change must be an object"))?;
            let patch = wire::patch::decode(change).map_err(bad)?;
            let before = document(current.mission.clone(), current.etag.clone())?;
            let edited = m.store.patch(
                id,
                Precondition {
                    rev: current.mission.rev,
                    etag: current.etag,
                },
                patch,
                "agent:parvi",
            )?;
            m.changed();
            let after = document(edited.mission, edited.etag)?;
            let mut answer = json!({"node":node,"mission":after});
            for (operation, list, key) in [
                ("createInputCard", "inputCards", "inputCard"),
                ("createFormation", "formations", "formation"),
                ("createGate", "gates", "gate"),
                ("createEnd", "ends", "end"),
                ("createLimit", "limits", "limit"),
            ] {
                if change.get(operation).is_some() {
                    let prior = before[list].as_array();
                    if let Some(created) = answer["mission"][list]
                        .as_array()
                        .and_then(|items| {
                            items.iter().find(|item| {
                                !prior.is_some_and(|p| p.iter().any(|old| old["id"] == item["id"]))
                            })
                        })
                        .cloned()
                    {
                        answer[key] = created;
                    }
                }
            }
            Ok(answer)
        }
        "mission_notes" => {
            let edit = m.store.notes_edit(required(&a, "mission")?)?;
            Ok(json!({"node":node,"notes":notes(edit)?}))
        }
        "mission_note" => {
            let id = required(&a, "mission")?;
            let current = m.store.notes_edit(id)?;
            let etag = optional(&a, "etag", &current.etag)?;
            let target = optional(&a, "target", "mission")?.into();
            let author = optional(&a, "author", "agent:parvi")?.into();
            let patch = match optional(&a, "action", "append")? {
                "append" => NotePatch::Append {
                    target,
                    author,
                    text: optional(&a, "text", "")?.into(),
                },
                "edit" => NotePatch::Edit {
                    target,
                    author,
                    id: required(&a, "entry")?.into(),
                    text: optional(&a, "text", "")?.into(),
                },
                "delete" => NotePatch::Delete {
                    target,
                    author,
                    id: required(&a, "entry")?.into(),
                },
                _ => return Err(bad("unknown note action")),
            };
            let edited = m.store.patch_notes(id, etag, patch)?;
            m.changed();
            Ok(json!({"node":node,"notes":notes(edited)?}))
        }
        "layout_show" => {
            let edit = m.store.layout_edit(required(&a, "mission")?)?;
            Ok(json!({"node":node,"layout":layout(edit)?}))
        }
        "layout_edit" => {
            let id = required(&a, "mission")?;
            let change = a
                .get("change")
                .filter(|v| v.is_object())
                .ok_or_else(|| bad("change must be an object"))?;
            let current = m.store.layout_edit(id)?;
            let etag = optional(&a, "etag", &current.etag)?;
            let patch = if change["arrange"] == true {
                let mission = m.store.read_mission(id)?;
                LayoutPatch::Nodes(parvi_mission_engine::arrange(&mission, &current.layout).nodes)
            } else if let Some(v) = change.get("nodes") {
                LayoutPatch::Nodes(serde_json::from_value(v.clone()).map_err(bad)?)
            } else if let Some(v) = change.get("edges") {
                LayoutPatch::Edges(serde_json::from_value(v.clone()).map_err(bad)?)
            } else {
                return Err(bad("layout needs nodes, edges or arrange"));
            };
            let edited = m.store.patch_layout(id, etag, patch)?;
            m.changed();
            Ok(json!({"node":node,"layout":layout(edited)?}))
        }
        "mission_run" | "formation_run" => start(m, name, &a).await,
        "run_list" => Ok(
            json!({"node":node,"runs":m.coordinator.list(maybe_text(&a,"mission").map_err(bad)?,false)?}),
        ),
        "run_status" => {
            let run = m.coordinator.run(required(&a, "run")?)?;
            let mut v = value(run)?;
            v["node"] = json!(node);
            Ok(v)
        }
        "run_wait" => wait(m, &a).await,
        "run_evidence" => Ok(
            json!({"evidence":m.coordinator.evidence(required(&a,"run")?, maybe_text(&a,"step").map_err(bad)?)?}),
        ),
        "run_attempts" => {
            let id = required(&a, "run")?;
            // Give both evidence commands the same unknown-run error.
            m.coordinator.run(id).map_err(|error| {
                if error.code == "not_found" || error.code == "invalid_input" {
                    Error::new("unknown_run", format!("run {id:?} not found"))
                } else {
                    error
                }
            })?;
            Ok(json!({"attempts":m.coordinator.attempts(id, maybe_text(&a,"step").map_err(bad)?)?}))
        }
        "run_gates" => Ok(json!({"gates":m.coordinator.gates(required(&a,"run")?)?})),
        "gate_request" => m
            .coordinator
            .gate_driver_request(required(&a, "run")?, required(&a, "gate")?),
        "run_seats" => Ok(json!({"node":node,"seats":m.coordinator.seats(required(&a,"run")?)?})),
        "run_logs" => Ok(json!({"node":node,"events":m.coordinator.events(required(&a,"run")?)?})),
        "run_abort" => {
            let id = required(&a, "run")?.to_owned();
            let actor = optional(&a, "actor", "agent:parvi")?.to_owned();
            let reason = optional(&a, "reason", "operator abort")?.to_owned();
            let coordinator = m.coordinator.clone();
            let run = blocking(move || coordinator.abort(&id, &actor, &reason)).await?;
            Ok(json!({"node":node,"run":run}))
        }
        "run_resume" => {
            let request = wire::ResumeRequest {
                mode: serde_json::from_value(json!(optional(&a, "mode", "reattach")?))
                    .map_err(bad)?,
                take_fail: boolean(&a, "take_fail")?,
                grant: boolean(&a, "grant")?,
                pick_best: boolean(&a, "pick_best")?,
                reason: optional(&a, "reason", "")?.into(),
                actor: optional(&a, "actor", "")?.into(),
            };
            let id = required(&a, "run")?.to_owned();
            let coordinator = m.coordinator.clone();
            let run = blocking(move || coordinator.resume(&id, request)).await?;
            Ok(json!({"node":node,"run":run}))
        }
        "run_land" => {
            let id = required(&a, "run")?.to_owned();
            let force = boolean(&a, "force")?;
            let coordinator = m.coordinator.clone();
            let notes = blocking(move || coordinator.land(&id, force)).await?;
            Ok(json!({"node":node,"landed":true,"notes":notes}))
        }
        "gate_approve" | "gate_reject" => {
            let id = required(&a, "run")?;
            let gate = required(&a, "gate")?;
            m.coordinator.record_verdict(
                id,
                gate,
                wire::HumanVerdict {
                    requested_seq: positive(&a, "requested_seq")?,
                    verdict: if name == "gate_approve" {
                        "pass"
                    } else {
                        "fail"
                    }
                    .into(),
                    response: optional(&a, "response", "")?.into(),
                    relayed_by: optional(&a, "relayed_by", "")?.into(),
                    actor: command_actor(&a)?,
                },
            )?;
            Ok(json!({"node":node,"runId":id}))
        }
        "gate_pick" => {
            let id = required(&a, "run")?;
            let gate = required(&a, "gate")?;
            let attempt = i64::try_from(positive(&a, "attempt")?).map_err(bad)?;
            m.coordinator.record_pick(
                id,
                gate,
                wire::HumanPick {
                    requested_seq: positive(&a, "requested_seq")?,
                    attempt,
                    reason: optional(&a, "reason", "")?.into(),
                    relayed_by: optional(&a, "relayed_by", "")?.into(),
                    actor: command_actor(&a)?,
                },
            )?;
            Ok(json!({"node":node,"runId":id}))
        }
        _ if crate::authoring::NAMES.contains(&name) => crate::authoring::call(&m, name, &a),
        _ => unreachable!(),
    }
}
async fn start(m: Missions, name: &str, a: &Value) -> Result<Value, Error> {
    let selector = required(a, "mission")?;
    let edit = m.store.mission_edit(selector)?;
    let mission = edit.mission;
    let pick = |items: Vec<(&str, &str)>, wanted: Option<&str>| -> Result<String, Error> {
        let found: Vec<_> = items
            .into_iter()
            .filter(|(id, title)| wanted.is_none_or(|w| w == *id || w == *title))
            .collect();
        match found.as_slice() {
            [(id, _)] => Ok((*id).into()),
            [] => Err(Error::new("not_found", "run target not found")),
            _ => Err(bad("run target is ambiguous; name its id")),
        }
    };
    let mut request = RunStart {
        mission: selector.into(),
        expected_rev: mission.rev,
        expected_etag: edit.etag,
        actor: optional(a, "actor", "agent:parvi")?.into(),
        cwd: optional(a, "cwd", "")?.into(),
        repository: optional(a, "repository", "")?.into(),
        base: optional(a, "base", "")?.into(),
        request_id: optional(a, "request_id", "")?.into(),
        known_devices: Some(vec![]),
        ..Default::default()
    };
    request.inputs =
        serde_json::from_value(a.get("inputs").cloned().unwrap_or(json!({}))).map_err(bad)?;
    request.context_paths =
        serde_json::from_value(a.get("context_paths").cloned().unwrap_or(json!([])))
            .map_err(bad)?;
    let work = optional(a, "work", "")?;
    if !work.is_empty() {
        request.work = m.resolver.resolve(work).await?.to_string();
    }
    if name == "mission_run" {
        request.input_card_id = pick(
            mission
                .input_cards
                .iter()
                .map(|c| (c.id.as_str(), c.title.as_str()))
                .collect(),
            maybe_text(a, "input").map_err(bad)?,
        )?;
    } else {
        request.formation_id = pick(
            mission
                .formations
                .iter()
                .map(|f| (f.id.as_str(), f.title.as_str()))
                .collect(),
            Some(required(a, "formation")?),
        )?;
    }
    let formation = name == "formation_run";
    let coordinator = m.coordinator.clone();
    let id = blocking(move || {
        if formation {
            coordinator.start_formation(request)
        } else {
            coordinator.start_mission(request)
        }
        .map_err(admission)
    })
    .await?;
    Ok(json!({"node":m.node,"runId":id}))
}
fn shell_word(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b))
        && !value.is_empty()
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
async fn wait(m: Missions, a: &Value) -> Result<Value, Error> {
    use parvi_mission_engine::WaitUntil;
    let id = required(a, "run")?;
    let until = match optional(a, "until", "needs-you")? {
        "needs-you" => WaitUntil::NeedsYou,
        "final" => WaitUntil::Final,
        "any-change" => WaitUntil::AnyChange,
        _ => return Err(bad("until must be needs-you, final or any-change")),
    };
    let number = |key: &str| -> Result<Option<u64>, Error> {
        match a.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| bad(format!("{key} must be a non-negative integer"))),
        }
    };
    let since = number("since")?;
    let timeout = number("timeout")?.unwrap_or(3600);
    let answer = m
        .coordinator
        .wait_run(id, until, since, Duration::from_secs(timeout))
        .await?;
    let run = &answer.run;
    let mut asks = Vec::new();
    let run_arg = shell_word(id);
    for gate in &run.waiting_gates {
        let gate_arg = shell_word(&gate.gate_id);
        let prefix = "parvi gate";
        let commands: Vec<String> = if let Some(pick) = &gate.pick {
            pick["candidates"].as_array().into_iter().flatten().filter_map(|candidate| {
                candidate.as_i64().or_else(|| candidate["attempt"].as_i64()).map(|attempt| format!("{prefix} pick {run_arg} {gate_arg} --requested-seq {} --attempt {attempt}", gate.requested_seq))
            }).collect()
        } else {
            ["approve", "reject"]
                .into_iter()
                .map(|verb| {
                    format!(
                        "{prefix} {verb} {run_arg} {gate_arg} --requested-seq {}",
                        gate.requested_seq
                    )
                })
                .collect()
        };
        asks.push(json!({"gate":gate.gate_id,"what":if gate.pick.is_some() {"Pick an attempt"} else {"Approve or reject"},"commands":commands}));
    }
    if run.status == "blocked"
        && let Some(event) = run
            .events
            .iter()
            .rev()
            .find(|event| event.kind == "run_blocked")
    {
        let mut commands = Vec::new();
        if run.resume_allowed {
            let extra = if run.resume_policy == "grant" || run.resume_policy == "grant_or_pick" {
                " --grant"
            } else {
                ""
            };
            commands.push(format!("parvi run resume {run_arg}{extra}"));
            if run.resume_policy == "grant_or_pick" {
                commands.push(format!("parvi run resume {run_arg} --pick-best"));
            }
        }
        asks.push(json!({"step":event.node_id,"what":run.block_code,"commands":commands}));
    }
    let next: Vec<_> = asks
        .iter()
        .flat_map(|ask| ask["commands"].as_array().into_iter().flatten().cloned())
        .collect();
    Ok(
        json!({"outcome":answer.outcome,"status":run.status,"cursor":answer.cursor,"asks":asks,"next":next}),
    )
}
