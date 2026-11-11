//! Admission-time snapshots and private workspaces.
use crate::{
    AdmissionFailure, BindingsSnapshot, Coordinator, Mission, RunEvent, RunStart,
    resolve_run_inputs,
};
const RUN_RECORD_MAX_BYTES: usize = 1 << 20;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("UTC time formats")
}
pub fn run_id() -> io::Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(format!(
        "run_{}",
        b.iter().map(|v| format!("{v:02x}")).collect::<String>()
    ))
}
pub fn validate_request(r: &RunStart) -> Result<(), AdmissionFailure> {
    for p in &r.context_paths {
        let path = Path::new(p);
        if !path.is_absolute() || !path.metadata().is_ok_and(|m| m.is_file() || m.is_dir()) {
            return Err(AdmissionFailure::Invalid(format!(
                "context path {p:?} must be an absolute existing file or directory"
            )));
        }
    }
    if !r.cwd.is_empty() && (!Path::new(&r.cwd).is_absolute() || !Path::new(&r.cwd).is_dir()) {
        return Err(AdmissionFailure::Invalid(
            "cwd must be an absolute existing directory".into(),
        ));
    }
    if !r.work.is_empty()
        && let Err(invalid) = parvi_work_ref::parse_resolved(&r.work)
    {
        return Err(AdmissionFailure::Invalid(format!("work: {invalid}")));
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}
pub fn create(
    c: &Coordinator,
    id: &str,
    slug: &str,
    mission: &Mission,
    bindings: &BindingsSnapshot,
    req: &RunStart,
) -> Result<PathBuf, AdmissionFailure> {
    let mission_data = crate::links::read_definition(
        &c.inner
            .root
            .join("missions")
            .join(format!("{slug}.mission.toml")),
    )
    .map_err(|e| AdmissionFailure::Invalid(e.to_string()))?;
    if Mission::parse(&mission_data).ok().as_ref() != Some(mission) {
        return Err(AdmissionFailure::Conflict);
    }
    let bindings_data = toml_edit::ser::to_string(bindings)
        .map_err(|e| AdmissionFailure::Invalid(e.to_string()))?;
    if mission_data.len() > RUN_RECORD_MAX_BYTES {
        return Err(AdmissionFailure::Invalid(
            "mission snapshot exceeds byte limit".into(),
        ));
    }
    if bindings_data.len() > RUN_RECORD_MAX_BYTES {
        return Err(AdmissionFailure::Invalid(
            "persona snapshot exceeds byte limit".into(),
        ));
    }
    let runs = c.inner.root.join("runs");
    let snapshot = runs.join(format!("{id}.snapshot.toml"));
    let binding_file = runs.join(format!("{id}.bindings.toml"));
    let automatic = req.cwd.is_empty() && req.repository.is_empty();
    let workspaces = crate::Workspaces::new(
        c.inner.root.join("worktrees"),
        c.inner.settings.repositories.clone(),
    );
    let mut tree = None;
    let mut workspace = if automatic {
        c.inner
            .settings
            .run_workspace_root
            .clone()
            .unwrap_or_else(|| c.inner.root.join("workspaces"))
            .join(id)
    } else {
        PathBuf::from(&req.cwd)
    };
    let (mut snapshot_created, mut binding_created, mut workspace_created) = (false, false, false);
    let result = (|| -> io::Result<()> {
        write_new(&snapshot, mission_data.as_bytes())?;
        snapshot_created = true;
        write_new(&binding_file, bindings_data.as_bytes())?;
        binding_created = true;
        if !req.repository.is_empty() {
            let prepared = workspaces
                .prepare(id, &req.repository, &req.base)
                .map_err(crate::error::to_io)?;
            workspace = prepared.path.clone();
            tree = Some(prepared);
        }
        if automatic {
            fs::create_dir_all(workspace.parent().unwrap())?;
            fs::create_dir(&workspace)?;
            workspace_created = true;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
            }
        }
        let actor = if req.actor.is_empty() {
            crate::DEFAULT_ACTOR
        } else {
            &req.actor
        };
        let event=RunEvent{kind:"run_started".into(),timestamp:now(),actor:actor.into(),mission_id:mission.id.clone(),mission_rev:mission.rev,input_card_id:req.input_card_id.clone(),work:req.work.clone(),data:json!({"missionSlug":mission.slug,"missionRev":mission.rev,"inputCardId":req.input_card_id,"formationId":req.formation_id,"work":req.work,"snapshot":snapshot,"bindingsSnapshot":binding_file,"cwd":workspace,"repository":req.repository,"requestId":req.request_id,"base":tree.as_ref().map(|t| t.base.as_str()).unwrap_or(""),"checkout":tree.as_ref().map(|t| t.checkout.as_path()),"contextPaths":req.context_paths,"inputs":resolve_run_inputs(mission,&req.inputs)}).as_object().unwrap().clone(),..Default::default()};
        c.ledger(id)?.create(event).map_err(crate::error::to_io)
    })();
    if let Err(e) = result {
        if let Some(tree) = &tree {
            let _ = workspaces.remove(tree, true, &Default::default());
        }
        if snapshot_created {
            let _ = fs::remove_file(snapshot);
        }
        if binding_created {
            let _ = fs::remove_file(binding_file);
        }
        if workspace_created {
            let _ = fs::remove_dir(&workspace);
        }
        return Err(e.into());
    }
    Ok(workspace)
}
