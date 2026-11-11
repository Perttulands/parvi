//! Drive one local dispatch and retain its native result.
use super::{local::adapter, transport::stage_guarded};
use crate::{
    DispatchResult, LocalSeats, Pointer, SeatEvent, SlotBinding, TurnWatch,
    parse_completion_sentinel,
};
use serde_json::json;
use std::sync::atomic::AtomicBool;

impl LocalSeats {
    /// Run or reattach to a dispatch. Events are saved before the callback,
    /// which must accept replayed events idempotently.
    pub async fn run(
        &self,
        id: &str,
        canceled: &AtomicBool,
        mut report: impl FnMut(&SeatEvent) -> Result<(), String> + Send,
    ) -> Result<DispatchResult, crate::Error> {
        (async {
        let records = self.records().clone();
        let lock_id = id.to_owned();
        let _guard = tokio::task::spawn_blocking(move || records.execution_guard(&lock_id))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let records = self.records();
        let dispatch = records.dispatch(id).map_err(|e| e.to_string())?;
        let events = records.events(id, 0).map_err(|e| e.to_string())?;
        let consumed_reported = events.iter().any(|event| event.key == "prompt-consumed");
        let result_reported = events.iter().any(|event| event.key == "slot-result");
        for event in &events {
            if public(event) {
                report(event)?;
            }
        }
        if let Some(result) = records.result(id).map_err(|e| e.to_string())? {
            return Ok(result);
        }
        let seat = records
            .seat(id)
            .map_err(|e| e.to_string())?
            .ok_or("dispatch has no seat")?;
        self.engine()
            .verify(&seat.session)
            .await
            .map_err(|e| e.to_string())?;
        let pane = seat.session.pane_id;
        let mut changes = self.engine().watch();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if self.engine().follow(pane).is_some() {
                    break Ok::<(), String>(());
                }
                changes.changed().await.map_err(|e| e.to_string())?;
            }
        })
        .await
        .map_err(|_| "seat did not appear in the terminal engine".to_string())??;
        let (kind, adapter) = adapter(&dispatch.harness)?;
        let path = records.brief_path(id).map_err(|e| e.to_string())?;
        let text = brief_pointer(&path);
        let pid = self.engine().pid(pane).await.ok();
        let mut pointer = Pointer {
            text: text.clone(),
            cwd: dispatch.cwd.clone(),
            pid,
            not_before: std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|time| time.checked_sub(std::time::Duration::from_secs(2))),
            binding: Some(SlotBinding {
                model: dispatch.model.clone(),
                effort: dispatch.effort.clone(),
            }),
            ..Default::default()
        };
        if let Some(event) = events.iter().find(|event| event.key == "prompt-consumed") {
            pointer.transcript = event
                .data
                .get("transcript")
                .and_then(|v| v.as_str())
                .map(Into::into);
            pointer.session_id = event
                .data
                .get("nativeSessionId")
                .and_then(|v| v.as_str())
                .map(Into::into);
            if let Some(value) = event.data.get("knownThreads") {
                pointer.known_threads =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            }
        }
        let started = events.iter().any(|event| event.key == "stage-started");
        let already_consumed = if started {
            adapter
                .turn(self.roots(), &pointer, &dispatch.run_id)
                .map_err(|e| e.to_string())?
                .consumed
        } else {
            false
        };
        let entered = events.iter().any(|event| event.key == "enter-intent");
        let staged = events.iter().any(|event| event.key == "stage-finished");
        if !already_consumed && !staged {
            records
                .append(id, "stage-started", "stage_started", Default::default())
                .map_err(|e| e.to_string())?;
            stage_guarded(
                self.engine().as_ref(),
                pane,
                kind,
                adapter.as_ref(),
                &text,
                canceled,
                started,
                entered,
                || {
                    records
                        .append(id, "enter-intent", "enter_intent", Default::default())
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
                |state, detail, since| {
                    let key = format!(
                        "state-{}",
                        records.events(id, 0).map_err(|e| e.to_string())?.len() + 1
                    );
                    let data = json!({"dispatchId":id,"state":state,"detail":detail,"since":since});
                    let event = records
                        .append(
                            id,
                            &key,
                            "seat_state",
                            data.as_object().cloned().unwrap_or_default(),
                        )
                        .map_err(|e| e.to_string())?;
                    report(&event)
                },
            )
            .await?;
            records
                .append(id, "stage-finished", "stage_finished", Default::default())
                .map_err(|e| e.to_string())?;
        }
        let adapter_sent = events.iter().any(|event| event.key == "adapter-send");
        let data = json!({"dispatchId":id,"nodeId":dispatch.node_id,"slotId":dispatch.slot_id,"sessionRef":format!("terminal:{}", seat.session_name),"adapter":"terminal","sent":true});
        let event = records
            .append(
                id,
                "adapter-send",
                "adapter_send",
                data.as_object().cloned().unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        if !adapter_sent {
            report(&event)?;
        }
        let watch = TurnWatch {
            engine: self.engine().as_ref(),
            pane,
            kind,
            adapter,
            roots: self.roots(),
            run_id: &dispatch.run_id,
            canceled,
            continued: events
                .iter()
                .any(|event| event.kind == "seat_idle_recovery"),
        };
        let report = std::sync::Mutex::new(&mut report);
        let answer = watch.wait(&mut pointer, |selected, turn| {
            let data = json!({"dispatchId":id,"sessionName":seat.session_name,"nativeSessionId":turn.session_id,"transcript":selected.transcript,"knownThreads":selected.known_threads});
            let event = records.append(id, "prompt-consumed", "seat_prompt_consumed", data.as_object().cloned().unwrap_or_default()).map_err(|e| e.to_string())?;
            if consumed_reported { Ok(()) } else { report.lock().map_err(|e| e.to_string())?(&event) }
        }, |state, detail, since| {
            let key = format!("state-{}", records.events(id, 0).map_err(|e| e.to_string())?.len() + 1);
            let data = json!({"dispatchId":id,"state":state,"detail":detail,"since":since});
            let kind = if state == "seat_idle_recovery" { "seat_idle_recovery" } else { "seat_state" };
            let event = records.append(id, &key, kind, data.as_object().cloned().unwrap_or_default()).map_err(|e| e.to_string())?;
            report.lock().map_err(|e| e.to_string())?(&event)
        }).await.map_err(|error| error.message)?;
        if answer.len() > 1024 * 1024 {
            return Err("oversized_output: native seat output exceeds 1 MiB".into());
        }
        let sentinel = parse_completion_sentinel(&answer, &dispatch.run_id)
            .ok_or("native completion lacks this run's sentinel")?;
        let result = DispatchResult {
            text: answer,
            status: sentinel.status.clone(),
        };
        let data = json!({"dispatchId":id,"nodeId":dispatch.node_id,"slotId":dispatch.slot_id,"status":sentinel.status,"sentinel":{"runId":sentinel.run_id,"status":sentinel.status,"artifact":sentinel.artifact}});
        let event = records
            .append(
                id,
                "slot-result",
                "slot_result",
                data.as_object().cloned().unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        records
            .finish(id, result.clone())
            .map_err(|e| e.to_string())?;
        if !result_reported {
            report.lock().map_err(|e| e.to_string())?(&event)?;
        }
        Ok(result)
     }).await.map_err(|message: String| crate::Error::new("seat_error", message))
    }
}
fn public(event: &SeatEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "seat_created"
            | "seat_prompt_consumed"
            | "adapter_send"
            | "slot_result"
            | "seat_cleanup"
            | "seat_state"
            | "seat_idle_recovery"
    )
}

/// The line pasted into a seat that points it at its brief; token counts
/// start after it.
pub(super) fn brief_pointer(path: &std::path::Path) -> String {
    format!(
        "Read the file {} and execute it exactly; it is your whole brief.",
        path.display()
    )
}
