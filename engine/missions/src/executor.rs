//! Formation-level execution boundary. Executors report events; they never
//! write a run ledger, so this boundary can later be placed on another node.
use crate::{BindingsSnapshot, Formation, RunInput};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
};

#[derive(Clone, Debug)]
pub struct FormationExecution {
    pub run_id: String,
    pub formation: Formation,
    pub bindings: BindingsSnapshot,
    pub cwd: PathBuf,
    /// For a repository run's step placed on another environment, the turn
    /// that environment prepares; `cwd` is then the coordinator's run
    /// worktree, and the seat works in the prepared one.
    pub turn: Option<crate::TurnWorkspace>,
    pub artifact_dir: PathBuf,
    pub context_paths: Vec<PathBuf>,
    /// The run's work item as a canonical reference, or empty.
    pub work: String,
    pub inputs: Vec<RunInput>,
    pub input_text: String,
    pub attempt: i64,
    pub judge: bool,
    /// For a judge picking the best attempt of a step whose Limit card is
    /// spent, the candidate attempts: the final judge picks with the
    /// `parvi-pick` contract, earlier judges recommend.
    pub pick_attempts: Vec<i64>,
    pub canceled: Arc<AtomicBool>,
    pub stopping: Arc<AtomicBool>,
    pub limits: crate::AttemptLimits,
}
#[derive(Clone, Debug)]
pub struct ExecutionEvent {
    pub kind: String,
    pub slot_id: String,
    pub data: Map<String, Value>,
}
pub(crate) struct PendingEvent {
    pub event: ExecutionEvent,
    pub ack: std::sync::mpsc::SyncSender<Result<(), String>>,
}
/// Sending waits for the coordinator to durably append the event. An
/// executor can therefore record its dispatch before it sends work to a seat.
#[derive(Clone)]
pub struct ExecutionSink {
    pub(crate) sender: Sender<PendingEvent>,
}
impl ExecutionSink {
    pub fn emit(&self, event: ExecutionEvent) -> Result<(), String> {
        let (ack, result) = std::sync::mpsc::sync_channel(1);
        self.sender
            .send(PendingEvent { event, ack })
            .map_err(|e| e.to_string())?;
        result.recv().map_err(|e| e.to_string())?
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FormationResult {
    pub status: String,
    pub text: String,
    pub outputs: Map<String, Value>,
}

/// A correction is sent to the same live judge seat that supplied the reply.
pub struct JudgeCorrection {
    pub run_id: String,
    pub node_id: String,
    pub attempt: i64,
    pub rejected: String,
    pub error: String,
}

/// Identity and formation of a durable dispatch to observe again without sending it.
#[derive(Clone, Debug)]
pub struct OpenDispatch {
    pub run_id: String,
    pub dispatch_id: String,
    pub node_id: String,
    pub slot_id: String,
    pub attempt: i64,
    pub formation: Formation,
    pub artifact_dir: PathBuf,
    pub dispatch_data: Map<String, Value>,
    pub canceled: Option<Arc<AtomicBool>>,
    pub stopping: Option<Arc<AtomicBool>>,
    pub final_judge: bool,
    /// What the step's Limit cards leave a reattached dispatch: once `spent`
    /// is raised the executor ends its seat and returns, as a live one does.
    pub limits: crate::AttemptLimits,
}

/// A seat retained by shutdown, including its immutable backend identity.
#[derive(Clone, Debug)]
pub struct LeftSeat {
    pub run_id: String,
    pub node_id: String,
    pub slot_id: String,
    pub identity: Map<String, Value>,
    pub cause: String,
}

pub trait Executor: Send + Sync + 'static {
    fn seats(&self, _: &str) -> std::io::Result<Vec<crate::AgentSeat>> {
        Ok(Vec::new())
    }
    fn execute_formation(
        &self,
        execution: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String>;
    fn reattach(&self, _: OpenDispatch) -> Result<FormationResult, String> {
        Err("executor cannot reattach this dispatch".into())
    }
    /// Non-blocking recovery of already completed evidence at startup.
    fn recover_completed(&self, _: OpenDispatch) -> Option<FormationResult> {
        None
    }
    /// Replay seat events saved while the coordinator was disconnected.
    fn completed_events(&self, _: &OpenDispatch) -> Result<Vec<ExecutionEvent>, String> {
        Ok(Vec::new())
    }
    /// End a previously detached seat by its recorded backend identity.
    fn end_left_seat(&self, _: LeftSeat) -> Result<String, String> {
        Err("executor cannot end a detached seat".into())
    }
    fn correct_judge(
        &self,
        _: JudgeCorrection,
        _: ExecutionSink,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// Until seats are available, a real execution cannot be dispatched.
pub struct UnavailableExecutor;
impl Executor for UnavailableExecutor {
    fn execute_formation(
        &self,
        _: FormationExecution,
        _: ExecutionSink,
    ) -> Result<FormationResult, String> {
        Err("seat executor is not available".into())
    }
}

/// Deterministic execution with the same dispatch and result events as Archon.
#[derive(Default)]
pub struct LabExecutor;
impl Executor for LabExecutor {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        if !request.cwd.is_dir() {
            return Err("run cwd is unavailable".into());
        }
        if let Some(goal) = request.formation.brief.as_ref().map(|b| b.goal.as_str())
            && let Some(duration) = goal
                .split("archon-lab-work:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
        {
            let millis = if let Some(n) = duration.strip_suffix("ms") {
                n.parse::<u64>().ok()
            } else if let Some(n) = duration.strip_suffix('s') {
                n.parse::<u64>().ok().and_then(|n| n.checked_mul(1000))
            } else {
                None
            }
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("invalid lab work duration {duration:?}"))?;
            // Time limits and their warnings can be rehearsed: each due
            // warning is recorded for every seat, as a real seat would get it
            // pasted, and the work stops when the step's time runs out.
            let end = std::time::Instant::now() + std::time::Duration::from_millis(millis);
            let mut warned = vec![false; request.limits.warnings.len()];
            while std::time::Instant::now() < end {
                if request.canceled.load(Ordering::Acquire) {
                    return Err("canceled".into());
                }
                if request.limits.is_spent() {
                    return Err("the step's Limit card ran out of time".into());
                }
                for (warning, sent) in request.limits.warnings.iter().zip(&mut warned) {
                    if *sent || warning.at > std::time::SystemTime::now() {
                        continue;
                    }
                    *sent = true;
                    for slot in &request.formation.slots {
                        // A refused warning is one this seat already had.
                        let _ = events.emit(ExecutionEvent {
                            kind: "limit_warning".into(),
                            slot_id: slot.id.clone(),
                            data: json!({"limitId":warning.limit_id,"mission":warning.mission,"text":warning.text})
                                .as_object()
                                .unwrap()
                                .clone(),
                        });
                    }
                }
                std::thread::sleep(
                    end.saturating_duration_since(std::time::Instant::now())
                        .min(std::time::Duration::from_millis(10)),
                );
            }
        }
        spend_lab_tokens(&request, &events)?;
        let result = lab_result(&request)?;
        for slot in &request.formation.slots {
            if request.canceled.load(Ordering::Acquire) {
                return Err("canceled".into());
            }
            let send = |kind: &str, data: Value| -> Result<(), String> {
                events.emit(ExecutionEvent {
                    kind: kind.into(),
                    slot_id: slot.id.clone(),
                    data: data.as_object().unwrap().clone(),
                })
            };
            let stem = if slot.agent_id.is_empty() {
                &slot.id
            } else {
                &slot.agent_id
            };
            let dispatch_id = format!("dsp_{}_{}_{}", request.run_id, slot.id, request.attempt);
            let mut brief = format!(
                "run: {}\nnode: {}\nslot: {}\nharness: {}\ncwd: {}\ngoal: {}\ninput: {}\ncontext paths: {}\n",
                request.run_id,
                request.formation.id,
                slot.id,
                slot.harness,
                request.cwd.display(),
                request
                    .formation
                    .brief
                    .as_ref()
                    .map(|b| b.goal.as_str())
                    .unwrap_or(""),
                request.input_text,
                request
                    .context_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            brief.push_str(&format!(
                "attempt artifact directory: {}\noutput capture budget: 32 MiB per run\n",
                request.artifact_dir.display()
            ));
            for port in &request.formation.outputs {
                brief.push_str(&format!("output port: {} kind: {}\n", port.id, port.kind()));
                if port.kind() != "text" {
                    brief.push_str(
                        "The ref must be the absolute path; text is a one-line summary.\n",
                    );
                }
            }
            if let Some(binding) = request.bindings.bindings.iter().find(|binding| {
                binding.node_id == request.formation.id && binding.slot_id == slot.id
            }) && !binding.card_toml.is_empty()
            {
                brief.push_str("role card:\n");
                brief.push_str(&binding.card_toml);
                brief.push('\n');
            }
            if let Some(details) = &request.formation.brief {
                for path in &details.files {
                    brief.push_str(&format!("reference file: {path}\n"));
                }
                for link in &details.links {
                    brief.push_str(&format!("reference link: {link}\n"));
                }
            }
            std::fs::create_dir_all(&request.artifact_dir).map_err(|e| e.to_string())?;
            let brief_path = request.artifact_dir.join(format!("brief-{}.md", slot.id));
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            let mut file = options.open(&brief_path).map_err(|e| e.to_string())?;
            file.write_all(brief.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            let digest = format!("{:x}", Sha256::digest(brief.as_bytes()));
            send(
                "slot_dispatch",
                json!({"dispatchId":dispatch_id,"nodeId":request.formation.id,"slotId":slot.id,"agentId":slot.agent_id,"harness":slot.harness,"sessionStem":stem,"sessionRef":format!("lab:{stem}"),"briefPath":brief_path,"promptSha256":digest,"promptRef":"","phase":"","recordedBeforeSend":true,"nativeAck":false}),
            )?;
            let evidence = serde_json::to_vec(&result).map_err(|e| e.to_string())?;
            let path = request.artifact_dir.join("lab-completed.json");
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path).map_err(|e| e.to_string())?;
            file.write_all(&evidence)
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            send(
                "slot_result",
                json!({"dispatchId":dispatch_id,"nodeId":request.formation.id,"slotId":slot.id,"status":"ok","sentinel":{"runId":request.run_id,"status":"ok","artifact":format!("lab-{}.md",slot.id)}}),
            )?;
        }
        Ok(result)
    }

    fn reattach(&self, dispatch: OpenDispatch) -> Result<FormationResult, String> {
        if dispatch.formation.slots.len() != 1 {
            return Err("lab evidence cannot recover a multi-slot formation".into());
        }
        let path = dispatch.artifact_dir.join("lab-completed.json");
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }
    fn recover_completed(&self, dispatch: OpenDispatch) -> Option<FormationResult> {
        self.reattach(dispatch).ok()
    }
}

/// How a lab brief makes a step spend tokens: "archon-lab-tokens: 40000" in
/// its goal. Where a Limit card counts the step's tokens they are recorded,
/// as a seat's transcript would count them, and the step stops when they
/// reach what its cards allow.
fn spend_lab_tokens(request: &FormationExecution, events: &ExecutionSink) -> Result<(), String> {
    let goal = request
        .formation
        .brief
        .as_ref()
        .map_or("", |b| b.goal.as_str());
    let Some(count) = goal
        .split("archon-lab-tokens:")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
    else {
        return Ok(());
    };
    if !request.limits.counts_tokens() {
        return Ok(());
    }
    let tokens = count
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("lab tokens {count:?} is not a positive whole number"))?;
    let slot = request
        .formation
        .slots
        .first()
        .map(|s| s.id.clone())
        .unwrap_or_default();
    let dispatch = format!("{}/{}/{slot}", request.formation.id, request.attempt);
    events.emit(ExecutionEvent {
        kind: "token_usage".into(),
        slot_id: slot,
        data: json!({"tokens":tokens,"usage":{"input":tokens,"cacheWrite":0,"cacheRead":0,"output":0}})
            .as_object()
            .unwrap()
            .clone(),
    })?;
    request.limits.count_tokens(&dispatch, tokens);
    if request.limits.is_spent() {
        return Err("the step spent the tokens its Limit card allows".into());
    }
    Ok(())
}

fn lab_result(request: &FormationExecution) -> Result<FormationResult, String> {
    let lines: Vec<_> = request
        .formation
        .slots
        .iter()
        .map(|slot| {
            format!(
                "lab-fake output from {} using {} for {}\nslot: {}\ninput:{}{}",
                if slot.agent_id.is_empty() {
                    "vanilla"
                } else {
                    &slot.agent_id
                },
                slot.harness,
                request.formation.title,
                slot.id,
                if request.input_text.starts_with("```") {
                    "\n"
                } else {
                    " "
                },
                request.input_text
            )
        })
        .collect();
    let text = collapse_lab_blocks(
        &collapse_lab_blocks(&lines.join("\n\n"), crate::contract_names::VERDICT),
        crate::contract_names::PICK,
    );
    let end = text.floor_char_boundary(text.len().min(8192));
    let text = text[..end].to_owned();
    let mut outputs = Map::new();
    for (index, port) in request.formation.outputs.iter().enumerate() {
        let output = if request.formation.outputs.len() > 1 {
            format!("{text}\noutput-port: {}", port.id)
        } else {
            text.clone()
        };
        let mut payload = json!({"text":output});
        if port.kind() == "file" || port.kind() == "folder" {
            std::fs::create_dir_all(&request.artifact_dir).map_err(|e| e.to_string())?;
            let path = request.artifact_dir.join(format!("lab-output-{index}"));
            let file = if port.kind() == "folder" {
                std::fs::create_dir(&path).map_err(|e| e.to_string())?;
                path.join("index.md")
            } else {
                path.clone()
            };
            std::fs::write(file, format!("{output}\n")).map_err(|e| e.to_string())?;
            payload["ref"] = json!(path);
        }
        outputs.insert(port.id.clone(), payload);
    }
    Ok(FormationResult {
        status: "done".into(),
        text,
        outputs,
    })
}

/// Keep one identical fenced fixture, while retaining conflicting fixtures.
fn collapse_lab_blocks(text: &str, lang: &str) -> String {
    let lines: Vec<_> = text.split('\n').collect();
    let marker = format!("```{lang}");
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim() == marker
            && let Some(end) = (index + 1..lines.len()).find(|&i| lines[i].trim() == "```")
        {
            blocks.push((index, end));
            index = end;
        }
        index += 1;
    }
    if blocks.len() < 2 {
        return text.into();
    }
    let body = |(start, end): (usize, usize)| {
        lines[start + 1..end]
            .iter()
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join("\n")
    };
    if blocks
        .iter()
        .skip(1)
        .any(|&block| body(block) != body(blocks[0]))
    {
        return text.into();
    }
    let mut kept = Vec::new();
    let mut i = 0;
    let mut next = 1;
    while i < lines.len() {
        if next < blocks.len() && i == blocks[next].0 {
            i = blocks[next].1 + 1;
            next += 1;
        } else {
            kept.push(lines[i]);
            i += 1;
        }
    }
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::collapse_lab_blocks;
    #[test]
    fn identical_lab_verdicts_collapse_but_conflicts_remain() {
        let a = "```parvi-verdict\n{\"verdict\":\"pass\"}\n```";
        let duplicate = format!("first\n{a}\nsecond\n{a}");
        assert_eq!(
            collapse_lab_blocks(&duplicate, "parvi-verdict")
                .matches("```parvi-verdict")
                .count(),
            1
        );
        let conflict = format!("{a}\n{}", a.replace("pass", "fail"));
        assert_eq!(collapse_lab_blocks(&conflict, "parvi-verdict"), conflict);
    }
}
