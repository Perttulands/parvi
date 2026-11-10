//! The scheduled commands, each an MCP tool and a `parvi scheduled <verb>`:
//! list, show, create, edit, copy, pause, resume, run and delete, as the
//! Scheduled view lists, makes, edits, pauses, resumes, runs and deletes
//! them.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use parvi_core::{Error, NodeId};
use parvi_node_kit::Command;
use parvi_node_kit::args::{flag, maybe_text, object, text};
use parvi_scheduled_contract::{BAD_REQUEST, Schedule, Target};
use serde_json::{Value, json};

use crate::service::{Create, Patch, Scheduled};

/// Who the task's audit says made a change.
const ACTOR: &str = "parvi";

/// What the commands reach.
pub(crate) struct Ctx {
    pub node: NodeId,
    pub scheduled: Arc<Scheduled>,
}

fn id_schema() -> Value {
    object(json!({ "id": { "type": "string" } }), &["id"])
}

fn task_properties() -> Value {
    json!({
        "name": { "type": "string" },
        "prompt": { "type": "string", "description": "Pasted into each session, then Enter." },
        "sessions": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Session names; user/name for another user's session.",
        },
        "every": { "type": "integer", "minimum": 1, "description": "Fire every this many minutes." },
        "cron": { "type": "string", "description": "Five cron fields, instead of every." },
        "timezone": { "type": "string", "description": "IANA zone for cron; the node's if absent." },
    })
}

/// A command over `ctx`, whose failure is the error's own or a bad
/// argument.
fn command<F, Fut>(
    ctx: &Arc<Ctx>,
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: F,
) -> Command
where
    F: Fn(Arc<Ctx>, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Error>> + Send + 'static,
{
    let ctx = ctx.clone();
    Command::new(name, description, schema, move |args| {
        call(ctx.clone(), args)
    })
}

/// Every scheduled command.
pub(crate) fn commands(ctx: &Arc<Ctx>) -> Vec<Command> {
    vec![
        command(
            ctx,
            "scheduled_list",
            "List the scheduled prompts: id, name, sessions, schedule, next run, \
        last status, and whether paused or held.",
            || object(json!({}), &[]),
            |ctx, _| async move { list(&ctx) },
        ),
        command(
            ctx,
            "scheduled_show",
            "One scheduled prompt in full, with its recent runs and audit.",
            id_schema,
            |ctx, args| async move { show(&ctx, &args) },
        ),
        command(
            ctx,
            "scheduled_create",
            "Schedule a prompt into sessions, every N minutes or on a cron.",
            || {
                let mut properties = task_properties();
                properties["paused"] = json!({ "type": "boolean", "default": false });
                object(properties, &["name", "prompt", "sessions"])
            },
            |ctx, args| async move { create(&ctx, &args).await },
        ),
        command(
            ctx,
            "scheduled_edit",
            "Change a scheduled prompt; what is not given stays.",
            || {
                let mut properties = task_properties();
                properties["id"] = json!({ "type": "string" });
                object(properties, &["id"])
            },
            |ctx, args| async move { edit(&ctx, &args).await },
        ),
        command(
            ctx,
            "scheduled_copy",
            "Duplicate a scheduled prompt, paused, as the view's Duplicate does.",
            || {
                object(
                    json!({ "id": { "type": "string" }, "name": { "type": "string" } }),
                    &["id"],
                )
            },
            |ctx, args| async move { copy(&ctx, &args).await },
        ),
        command(
            ctx,
            "scheduled_pause",
            "Pause a scheduled prompt.",
            id_schema,
            |ctx, args| async move { act(&ctx, &args, "pause").await },
        ),
        command(
            ctx,
            "scheduled_resume",
            "Resume a paused or held scheduled prompt.",
            id_schema,
            |ctx, args| async move { act(&ctx, &args, "resume").await },
        ),
        command(
            ctx,
            "scheduled_run",
            "Send a scheduled prompt now, whatever its schedule.",
            id_schema,
            |ctx, args| async move { act(&ctx, &args, "run").await },
        ),
        command(
            ctx,
            "scheduled_delete",
            "Delete a scheduled prompt.",
            id_schema,
            |ctx, args| async move { act(&ctx, &args, "delete").await },
        ),
    ]
}

/// An argument that cannot be read.
fn bad(error: anyhow::Error) -> Error {
    Error::new(BAD_REQUEST, format!("{error:#}"))
}

fn list(ctx: &Ctx) -> Result<Value, Error> {
    let tasks: Vec<Value> = ctx
        .scheduled
        .list()?
        .into_iter()
        .map(|task| {
            json!({
                "id": task.id,
                "name": task.name,
                "sessions": task.targets.iter().map(Target::label).collect::<Vec<_>>(),
                "schedule": task.schedule,
                "paused": task.paused,
                "held": task.held,
                "next_run": task.next_run,
                "last_status": task.last_status,
            })
        })
        .collect();
    Ok(json!({ "node": ctx.node, "tasks": tasks }))
}

fn show(ctx: &Ctx, args: &Value) -> Result<Value, Error> {
    let task = ctx.scheduled.get(text(args, "id").map_err(bad)?)?;
    Ok(json!({ "node": ctx.node, "task": task }))
}

async fn create(ctx: &Ctx, args: &Value) -> Result<Value, Error> {
    let create = (|| {
        Ok::<_, anyhow::Error>(Create {
            name: text(args, "name")?.to_owned(),
            prompt: text(args, "prompt")?.to_owned(),
            targets: targets(args)?.context("sessions is required")?,
            schedule: schedule(args)?.context("every or cron is required")?,
            enabled: Some(true),
            paused: flag(args, "paused", false)?,
            created_by: ACTOR.into(),
            updated_by: ACTOR.into(),
        })
    })()
    .map_err(bad)?;
    let task = ctx.scheduled.create(create).await?;
    Ok(json!({ "node": ctx.node, "task": task }))
}

async fn edit(ctx: &Ctx, args: &Value) -> Result<Value, Error> {
    let (id, patch) = (|| {
        Ok::<_, anyhow::Error>((
            text(args, "id")?,
            Patch {
                name: maybe_text(args, "name")?.map(str::to_owned),
                prompt: maybe_text(args, "prompt")?.map(str::to_owned),
                targets: targets(args)?,
                schedule: schedule(args)?,
                updated_by: ACTOR.into(),
                ..Default::default()
            },
        ))
    })()
    .map_err(bad)?;
    let task = ctx.scheduled.patch(id, patch).await?;
    Ok(json!({ "node": ctx.node, "task": task }))
}

async fn copy(ctx: &Ctx, args: &Value) -> Result<Value, Error> {
    let from = ctx.scheduled.get(text(args, "id").map_err(bad)?)?;
    let create = Create {
        name: match maybe_text(args, "name").map_err(bad)? {
            Some(name) => name.to_owned(),
            None => format!("{} (copy)", from.name),
        },
        prompt: from.prompt,
        targets: from.targets,
        schedule: from.schedule,
        enabled: Some(true),
        paused: true,
        created_by: ACTOR.into(),
        updated_by: ACTOR.into(),
    };
    let task = ctx.scheduled.create(create).await?;
    Ok(json!({ "node": ctx.node, "task": task }))
}

async fn act(ctx: &Ctx, args: &Value, action: &str) -> Result<Value, Error> {
    let scheduled = &ctx.scheduled;
    let id = text(args, "id").map_err(bad)?;
    let mut answer = match action {
        "pause" => json!({ "task": scheduled.pause(id, ACTOR)? }),
        "resume" => json!({ "task": scheduled.resume(id, ACTOR).await? }),
        "run" => {
            let (task, run) = scheduled.run_now(id, ACTOR).await?;
            json!({ "task": task, "run": run })
        }
        "delete" => {
            scheduled.delete(id)?;
            json!({ "deleted": id })
        }
        _ => unreachable!("{action} is not a scheduled action"),
    };
    answer["node"] = json!(ctx.node);
    Ok(answer)
}

fn targets(args: &Value) -> Result<Option<Vec<Target>>> {
    let Some(sessions) = args.get("sessions").filter(|s| !s.is_null()) else {
        return Ok(None);
    };
    let sessions = sessions.as_array().context("sessions is a list")?;
    let targets = sessions
        .iter()
        .map(|s| {
            let s = s.as_str().context("each session is a name")?;
            let (unix_user, session_name) = s.split_once('/').unwrap_or(("", s));
            Ok(Target {
                session_name: session_name.to_owned(),
                unix_user: unix_user.to_owned(),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Some(targets))
}

fn schedule(args: &Value) -> Result<Option<Schedule>> {
    let every = args.get("every").filter(|v| !v.is_null());
    let cron = maybe_text(args, "cron")?;
    let timezone = maybe_text(args, "timezone")?.unwrap_or_default().to_owned();
    Ok(match (every, cron) {
        (Some(_), Some(_)) => bail!("give every or cron, not both"),
        (Some(every), None) => Some(Schedule {
            kind: "interval".into(),
            every_minutes: every.as_i64().context("every is a number of minutes")?,
            timezone,
            ..Default::default()
        }),
        (None, Some(cron)) => Some(Schedule {
            kind: "cron".into(),
            expression: cron.to_owned(),
            timezone,
            ..Default::default()
        }),
        (None, None) => None,
    })
}
