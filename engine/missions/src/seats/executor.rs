//! Formation executor over the dispatch-addressed seat interface.
use super::brief;
use crate::{
    AgentSeat, Dispatch, DispatchResult, ExecutionEvent, ExecutionSink, Executor,
    FormationExecution, FormationResult, JudgeCorrection, LeftSeat, OpenDispatch, SeatEvent,
    SeatFuture, SeatId, SeatSide, parse_outputs, parse_verdict,
};
use parvi_mission_model::Slot;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

#[derive(Clone)]
struct HeldJudge {
    request: FormationExecution,
    slot: Slot,
    original: String,
    seat: SeatId,
    prompt: String,
}

pub struct SeatExecutor {
    seats: Arc<dyn SeatSide>,
    runtime: tokio::runtime::Handle,
    judges: Mutex<HashMap<(String, String, i64), HeldJudge>>,
}
impl SeatExecutor {
    pub fn new(seats: Arc<dyn SeatSide>, runtime: tokio::runtime::Handle) -> Self {
        Self {
            seats,
            runtime,
            judges: Mutex::new(HashMap::new()),
        }
    }
    fn turn(
        &self,
        request: &FormationExecution,
        slot: &Slot,
        phase: &str,
        prompt: String,
        original: Option<&str>,
        sink: &ExecutionSink,
    ) -> Result<(String, String, SeatId), String> {
        let id = dispatch_id(request, slot, phase);
        let dispatch = Dispatch {
            id: id.clone(),
            run_id: request.run_id.clone(),
            node_id: request.formation.id.clone(),
            slot_id: slot.id.clone(),
            harness: slot.harness.clone(),
            model: slot.model.clone(),
            effort: slot.effort.clone(),
            cwd: request.cwd.clone(),
            brief: prompt.clone(),
            reuse_dispatch: original.map(str::to_owned),
            place: request.formation.device.clone(),
            turn: request.turn.clone(),
        };
        let (seat, result) = self.runtime.block_on(async {
            self.reach(request, &slot.id, sink, self.seats.dispatch(dispatch)).await?;
            let mut events = self.seats.events(&id, 0).map_err(|error| error.message)?;
            let mut created = None;
            let (seat, path) = loop {
                let event = events.recv().await.ok_or("seat event stream ended")?.map_err(|error| error.message)?;
                if event.kind == "seat_created" { created = Some(event); continue; }
                if event.kind == "seat_prepared" {
                    let seat: SeatId = serde_json::from_value(event.data.get("seat").cloned().ok_or("seat identity missing")?).map_err(|e| e.to_string())?;
                    let path = event.data.get("briefPath").and_then(Value::as_str).ok_or("brief path missing")?.to_owned();
                    break (seat, path);
                }
            };
            if let Some(event) = created { report_event(sink, &slot.id, event)?; }
            let data = dispatch_data(&id, &request.formation.id, slot, &seat, &path, &prompt, phase, true);
            if let Err(error) = sink.emit(ExecutionEvent { kind: "slot_dispatch".into(), slot_id: slot.id.clone(), data }).map_err(|e| format!("seat_ledger_unavailable: {e}")) {
                if request.stopping.load(Ordering::Acquire) { self.detach(original.unwrap_or(&id), &seat, &slot.id, sink).await?; }
                return Err(error);
            }
            self.seats.stage(&id, &prompt).await.map_err(|error| error.message)?;
            let mut terminal_event = false;
            let mut warned = vec![false; request.limits.warnings.len()];
            let mut counted = 0;
            // Reading a transcript costs, so live counts refresh every few seconds.
            let mut metered: Option<std::time::Instant> = None;
            let result = loop {
                if request.canceled.load(Ordering::Acquire) {
                    if request.stopping.load(Ordering::Acquire) {
                        self.detach(original.unwrap_or(&id), &seat, &slot.id, sink).await?;
                        return Err("coordinator shutdown".into());
                    }
                    // A seat whose node does not answer cannot be ended now;
                    // it stays as it is, as every session does.
                    if let Some(why) = self.seats.away(&request.formation.device) {
                        return Err(format!("canceled while {why}; its seat is left as it is"));
                    }
                    break Err("canceled".into());
                }
                if request.limits.counts_tokens()
                    && metered.is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
                    && let Some(tokens) = { metered = Some(std::time::Instant::now()); self.seats.tokens(&id).map_err(|error| error.message)? }
                {
                    counted = tokens;
                    request.limits.count_tokens(&id, tokens);
                }
                if request.limits.is_spent() {
                    break Err("the step's Limit card is spent".into());
                }
                self.warn(&request.limits, &mut warned, &id, &slot.id, sink).await;
                tokio::select! {
                    incoming = events.recv() => {
                        let event = incoming.ok_or("seat event stream ended")?.map_err(|error| error.message)?;
                        terminal_event |= matches!(event.kind.as_str(), "slot_result" | "seat_failed");
                        match event.kind.as_str() {
                            "adapter_send" | "seat_prompt_consumed" | "slot_result" | "seat_state" | "seat_idle_recovery" => report_event(sink, &slot.id, event)?,
                            "seat_failed" => {},
                            _ => {},
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(250)) => (),
                }
                if terminal_event && let Some(result) = self.seats.result(&id).map_err(|error| error.message)? { break Ok(result); }
            };
            // What the dispatch spent, so later attempts and the mission's
            // card read the ledger, never transcripts. A seat side that
            // cannot count tokens records none.
            if request.limits.counts_tokens()
                && let Some(tokens) = self.seats.tokens(&id).map_err(|error| error.message)?.or((counted > 0).then_some(counted))
            {
                sink.emit(ExecutionEvent { kind: "token_usage".into(), slot_id: slot.id.clone(), data: json!({"dispatchId":id,"tokens":tokens}).as_object().cloned().unwrap_or_default() })
                    .map_err(|e| format!("seat_ledger_unavailable: {e}"))?;
            }
            Ok::<_, String>((seat, result))
        })?;
        match result {
            Ok(result) if result.status == "ok" => Ok((id, result.text, seat)),
            Ok(result) => {
                let error = if result.text.is_empty() {
                    format!("seat returned status {:?}", result.status)
                } else {
                    result.text
                };
                Err(self.failed(original.unwrap_or(&id), &seat, &slot.id, sink, error))
            }
            Err(error) => Err(self.failed(original.unwrap_or(&id), &seat, &slot.id, sink, error)),
        }
    }
    /// Waits for a call to the step's place. While its node does not answer
    /// the step says so, as a placed seat's own stream does
    /// (`waiting_for_device`), and a stopped run stops waiting.
    async fn reach<T>(
        &self,
        request: &FormationExecution,
        slot: &str,
        sink: &ExecutionSink,
        mut call: SeatFuture<'_, T>,
    ) -> Result<T, String> {
        let place = &request.formation.device;
        let mut waiting = false;
        loop {
            tokio::select! {
                answer = &mut call => {
                    if waiting && answer.is_ok() {
                        place_state(sink, slot, "working", &format!("{place} answers again"))?;
                    }
                    return answer.map_err(|error| error.message);
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            if request.canceled.load(Ordering::Acquire) {
                return Err(if request.stopping.load(Ordering::Acquire) {
                    "coordinator shutdown".into()
                } else {
                    "canceled".into()
                });
            }
            if !waiting && let Some(why) = self.seats.away(place) {
                place_state(sink, slot, "waiting_for_device", &why)?;
                waiting = true;
            }
        }
    }
    /// Paste each time warning that is due into the seat, once the ledger
    /// accepts it: it refuses one this seat already had.
    async fn warn(
        &self,
        limits: &crate::AttemptLimits,
        warned: &mut [bool],
        id: &str,
        slot: &str,
        sink: &ExecutionSink,
    ) {
        for (warning, sent) in limits.warnings.iter().zip(warned.iter_mut()) {
            if *sent || warning.at > std::time::SystemTime::now() {
                continue;
            }
            *sent = true;
            let data = json!({"dispatchId":id,"limitId":warning.limit_id,"mission":warning.mission,"text":warning.text});
            let claimed = sink.emit(ExecutionEvent {
                kind: "limit_warning".into(),
                slot_id: slot.into(),
                data: data.as_object().cloned().unwrap_or_default(),
            });
            if claimed.is_ok()
                && let Err(error) = self.seats.notify(id, &warning.text).await
            {
                eprintln!("parvi: cannot paste a limit warning into dispatch {id}: {error}");
            }
        }
    }
    /// Reattach to a dispatch and wait for its result, metering its tokens
    /// as a live dispatch is metered until its step's Limit card is spent.
    async fn wait(&self, request: &OpenDispatch, id: &str) -> Result<DispatchResult, String> {
        self.seats
            .stage(id, "")
            .await
            .map_err(|error| error.message)?;
        let mut events = self.seats.events(id, 0).map_err(|error| error.message)?;
        let limits = &request.limits;
        let mut metered: Option<std::time::Instant> = None;
        loop {
            if request
                .canceled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                return Err("canceled".into());
            }
            if let Some(result) = self.seats.result(id).map_err(|error| error.message)? {
                if limits.counts_tokens()
                    && let Some(tokens) = self.seats.tokens(id).map_err(|error| error.message)?
                {
                    limits.settle_tokens(id, tokens);
                }
                return Ok(result);
            }
            if limits.counts_tokens()
                && metered.is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
                && let Some(tokens) = {
                    metered = Some(std::time::Instant::now());
                    self.seats.tokens(id).map_err(|error| error.message)?
                }
            {
                limits.count_tokens(id, tokens);
            }
            if limits.is_spent() {
                return Err("the step's Limit card is spent".into());
            }
            tokio::select! {
                event = events.recv() => { event.ok_or("seat event stream ended")?.map_err(|error| error.message)?; }
                _ = tokio::time::sleep(Duration::from_millis(250)) => (),
            }
        }
    }
    /// Reattach to a dispatch, and to the correction it owes, until its result.
    async fn reattach_seat(&self, request: &OpenDispatch) -> Result<FormationResult, String> {
        let result = self.wait(request, &request.dispatch_id).await?;
        let Some((id, error)) = owed_correction(request, &result) else {
            return recovered_result(request, result);
        };
        // The same one-turn correction the live run asks for, on the
        // same seat; an earlier attempt may already have started it.
        if !matches!(self.seats.result(&id), Ok(Some(_))) {
            let correction = match self.seats.dispatch_info(&id) {
                Ok(correction) => correction,
                Err(_) => {
                    let original = self
                        .seats
                        .dispatch_info(&request.dispatch_id)
                        .map_err(|error| error.message)?;
                    Dispatch {
                        id: id.clone(),
                        brief: correction_prompt(&original.brief, &error),
                        reuse_dispatch: Some(request.dispatch_id.clone()),
                        ..original
                    }
                }
            };
            self.seats
                .dispatch(correction)
                .await
                .map_err(|error| error.message)?;
        }
        let corrected = self.wait(request, &id).await?;
        let result = recovered_result(request, corrected);
        if result.is_err()
            && let (_, Some((seat, _))) = self.replay(request, &request.dispatch_id).await?
        {
            self.seats
                .end_seat(&seat, "run_final")
                .await
                .map_err(|error| error.message)?;
        }
        result
    }
    /// The seat a dispatch was prepared on, from the seat's own events.
    async fn seat_of(&self, id: &str) -> Result<Option<SeatId>, String> {
        let mut events = self.seats.events(id, 0).map_err(|error| error.message)?;
        while let Some(event) = events.recv().await {
            let event = event.map_err(|error| error.message)?;
            if event.kind == "seat_prepared" {
                let seat = event
                    .data
                    .get("seat")
                    .cloned()
                    .ok_or("seat identity missing")?;
                return serde_json::from_value(seat)
                    .map(Some)
                    .map_err(|e| e.to_string());
            }
            if matches!(event.kind.as_str(), "slot_result" | "seat_failed") {
                break;
            }
        }
        Ok(None)
    }
    /// The public events of a finished dispatch, and its seat and brief path.
    async fn replay(
        &self,
        request: &OpenDispatch,
        id: &str,
    ) -> Result<(Vec<ExecutionEvent>, Option<(SeatId, String)>), String> {
        let mut events = self.seats.events(id, 0).map_err(|error| error.message)?;
        let mut replay = Vec::new();
        let mut prepared = None;
        loop {
            let event = events
                .recv()
                .await
                .ok_or("seat event stream ended")?
                .map_err(|error| error.message)?;
            if event.kind == "seat_prepared" {
                let seat = serde_json::from_value::<SeatId>(
                    event
                        .data
                        .get("seat")
                        .cloned()
                        .ok_or("seat identity missing")?,
                )
                .map_err(|e| e.to_string())?;
                let path = event
                    .data
                    .get("briefPath")
                    .and_then(Value::as_str)
                    .ok_or("brief path missing")?
                    .to_owned();
                prepared = Some((seat, path));
            }
            let terminal = matches!(event.kind.as_str(), "slot_result" | "seat_failed");
            if matches!(
                event.kind.as_str(),
                "seat_created"
                    | "adapter_send"
                    | "seat_prompt_consumed"
                    | "slot_result"
                    | "seat_state"
                    | "seat_idle_recovery"
            ) {
                let mut data = event.data;
                data.insert("seatEventSeq".into(), json!(event.seq));
                replay.push(ExecutionEvent {
                    kind: event.kind,
                    slot_id: request.slot_id.clone(),
                    data,
                });
            }
            if terminal {
                return Ok((replay, prepared));
            }
        }
    }
    async fn detach(
        &self,
        id: &str,
        seat: &SeatId,
        slot: &str,
        sink: &ExecutionSink,
    ) -> Result<(), String> {
        self.seats
            .end_seat(seat, "shutdown")
            .await
            .map_err(|error| error.message)?;
        let mut events = self.seats.events(id, 0).map_err(|error| error.message)?;
        while let Some(event) = events.recv().await {
            let event = event.map_err(|error| error.message)?;
            if event.kind == "seat_cleanup"
                && event.data.get("outcome").and_then(Value::as_str) == Some("left_shutdown")
            {
                report_event(sink, slot, event)?;
                return Ok(());
            }
        }
        Err("seat detach event unavailable".into())
    }
    fn cleanup(
        &self,
        id: &str,
        seat: &SeatId,
        slot: &str,
        sink: &ExecutionSink,
    ) -> Result<(), String> {
        self.runtime.block_on(async {
            let outcome = self
                .seats
                .end_seat(seat, "run_final")
                .await
                .map_err(|error| error.message)?;
            let mut events = self.seats.events(id, 0).map_err(|error| error.message)?;
            loop {
                let event = events
                    .recv()
                    .await
                    .ok_or("seat event stream ended")?
                    .map_err(|error| error.message)?;
                if event.kind == "seat_cleanup" {
                    report_event(sink, slot, event)?;
                    break;
                }
            }
            if outcome != "ended" {
                return Err(format!("seat_cleanup: {outcome}"));
            }
            Ok(())
        })
    }
    /// Publishes a placed turn's work once its seat has ended, since a
    /// worktree cannot be removed under a running process on Windows.
    fn publish(
        &self,
        dispatch: &str,
        request: &FormationExecution,
        slot: &Slot,
        sink: &ExecutionSink,
    ) -> Result<(), String> {
        let Some(turn) = &request.turn else {
            return Ok(());
        };
        let event = self.runtime.block_on(published_turn(
            &*self.seats,
            dispatch,
            &request.formation.device,
            turn,
            &slot.id,
        ))?;
        sink.emit(event)
            .map_err(|e| format!("seat_ledger_unavailable: {e}"))
    }
    fn failed(
        &self,
        id: &str,
        seat: &SeatId,
        slot: &str,
        sink: &ExecutionSink,
        error: String,
    ) -> String {
        if error.contains("seat_ledger_unavailable") || error.contains("seat stream closed") {
            return error;
        }
        if let Err(cleanup) = self.cleanup(id, seat, slot, sink) {
            return format!("{error}; cleanup: {cleanup}");
        }
        error
    }
}
impl Executor for SeatExecutor {
    fn seats(&self, run_id: &str) -> Result<Vec<AgentSeat>, crate::Error> {
        self.seats.seats(run_id)
    }
    fn recover_completed(&self, request: OpenDispatch) -> Option<FormationResult> {
        let result = self.seats.result(&request.dispatch_id).ok()??;
        match owed_correction(&request, &result) {
            Some((id, _)) => recovered_result(&request, self.seats.result(&id).ok()??).ok(),
            None => recovered_result(&request, result).ok(),
        }
    }
    fn reattach(&self, request: OpenDispatch) -> Result<FormationResult, crate::Error> {
        (|| -> Result<FormationResult, String> {
            let result = self.runtime.block_on(self.reattach_seat(&request));
            if request.limits.is_spent() {
                // A live attempt the limit cut off ends its seat the same way.
                self.runtime.block_on(async {
                    if let Some(seat) = self.seat_of(&request.dispatch_id).await? {
                        self.seats
                            .end_seat(&seat, "run_final")
                            .await
                            .map_err(|error| error.message)?;
                    }
                    Ok::<_, String>(())
                })?;
            }
            result
        })()
        .map_err(|message: String| crate::Error::new("executor_error", message))
    }
    fn completed_events(
        &self,
        request: &OpenDispatch,
    ) -> Result<Vec<ExecutionEvent>, crate::Error> {
        self.runtime.block_on(async {
            let (mut replay, prepared) = self.replay(request, &request.dispatch_id).await?;
            let mut answer = request.dispatch_id.clone();
            if let Some(result) = self.seats.result(&request.dispatch_id).map_err(|error| error.message)?
                && let Some((id, _)) = owed_correction(request, &result)
                && matches!(self.seats.result(&id), Ok(Some(_)))
            {
                let (events, corrected) = self.replay(request, &id).await?;
                let (seat, path) = corrected.ok_or("correction seat missing")?;
                let slot = request.formation.slots.iter().find(|slot| slot.id == request.slot_id).ok_or("slot missing")?;
                let brief = self.seats.dispatch_info(&id).map_err(|error| error.message)?.brief;
                let data = dispatch_data(&id, &request.node_id, slot, &seat, &path, &brief, "output-correction", false);
                replay.push(ExecutionEvent { kind: "slot_dispatch".into(), slot_id: request.slot_id.clone(), data });
                replay.extend(events);
                answer = id;
            }
            if let Some((seat, _)) = prepared {
                let persisted = self.seats.dispatch_info(&request.dispatch_id).map_err(|error| error.message)?;
                let invalid_judge = request.final_judge && self.seats.result(&answer).map_err(|error| error.message)?.is_some_and(|result| parse_verdict(&result.text).is_err());
                if invalid_judge {
                    let slot = request.formation.slots.iter().find(|slot| slot.id == request.slot_id).cloned().ok_or("slot missing")?;
                    let execution = FormationExecution {
                        run_id: request.run_id.clone(), formation: request.formation.clone(),
                        bindings: crate::BindingsSnapshot { schema: 1, run_id: request.run_id.clone(), mission_id: String::new(), mission_slug: String::new(), mission_rev: 0, input_card_id: String::new(), bindings: Vec::new(), gate_bindings: Vec::new() },
                        cwd: persisted.cwd, turn: persisted.turn.clone(), artifact_dir: request.artifact_dir.clone(), context_paths: Vec::new(), work: String::new(), inputs: Vec::new(), input_text: String::new(), attempt: request.attempt, judge: true, pick_attempts: Vec::new(),
                        canceled: request.canceled.clone().unwrap_or_default(), stopping: request.stopping.clone().unwrap_or_default(), limits: Default::default(),
                    };
                    self.judges.lock().map_err(|e| e.to_string())?.insert((request.run_id.clone(), request.node_id.clone(), request.attempt), HeldJudge { request: execution, slot, original: persisted.reuse_dispatch.unwrap_or_else(|| request.dispatch_id.clone()), seat, prompt: persisted.brief });
                    return Ok(replay);
                }
                let outcome = self.seats.end_seat(&seat, "run_final").await.map_err(|error| error.message)?;
                if outcome != "ended" && outcome != "gone" {
                    return Err(format!("seat_cleanup: {outcome}"));
                }
                let data = json!({"dispatchId":request.dispatch_id,"sessionName":seat.session_name,"outcome":outcome,"cause":"run_final"});
                replay.push(ExecutionEvent { kind: "seat_cleanup".into(), slot_id: request.slot_id.clone(), data: data.as_object().cloned().unwrap_or_default() });
            }
            // A placed turn is published once its seat has ended, also when
            // the coordinator restarted in between; publishing is idempotent.
            let original = self.seats.dispatch_info(&request.dispatch_id).map_err(|error| error.message)?;
            if let Some(turn) = &original.turn {
                replay.push(published_turn(&*self.seats, &request.dispatch_id, &original.place, turn, &request.slot_id).await?);
            }
            Ok(replay)
        }).map_err(|message: String| crate::Error::new("executor_error", message))
    }
    fn end_left_seat(&self, left: LeftSeat) -> Result<String, crate::Error> {
        (|| -> Result<String, String> {
            let data = &left.identity;
            let get = |key: &str| {
                data.get(key)
                    .cloned()
                    .ok_or_else(|| format!("missing {key}"))
            };
            let seat = SeatId {
                node: serde_json::from_value(get("node")?).map_err(|e| e.to_string())?,
                session_name: serde_json::from_value(get("sessionName")?)
                    .map_err(|e| e.to_string())?,
                session: parvi_term::SessionRef {
                    session_id: serde_json::from_value(get("sessionId")?)
                        .map_err(|e| e.to_string())?,
                    pane_id: serde_json::from_value(get("paneId")?).map_err(|e| e.to_string())?,
                    generation: serde_json::from_value(get("generation")?)
                        .map_err(|e| e.to_string())?,
                },
            };
            self.runtime
                .block_on(self.seats.end_seat(&seat, &left.cause))
                .map_err(|error| error.message)
        })()
        .map_err(|message: String| crate::Error::new("executor_error", message))
    }
    fn execute_formation(
        &self,
        request: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, crate::Error> {
        (|| -> Result<FormationResult, String> {
            if request.formation.kind != "solo" || request.formation.slots.len() != 1 {
                return Err("seat executor currently supports solo formations".into());
            }
            let slot = request.formation.slots[0].clone();
            let mut request = request;
            if let Some(turn) = &request.turn {
                let place = &request.formation.device;
                let cwd = self.runtime.block_on(self.reach(
                    &request,
                    &slot.id,
                    &sink,
                    self.seats.prepare_turn(place, turn),
                ))?;
                let data = json!({"place":place,"commit":turn.commit,"cwd":cwd});
                sink.emit(ExecutionEvent {
                    kind: "workspace_fetch".into(),
                    slot_id: slot.id.clone(),
                    data: data.as_object().cloned().unwrap_or_default(),
                })
                .map_err(|e| format!("seat_ledger_unavailable: {e}"))?;
                request.cwd = cwd;
            }
            let prompt = brief::render(&request, &slot);
            let (original, mut text, seat) =
                self.turn(&request, &slot, "initial", prompt.clone(), None, &sink)?;
            let outputs = match outputs(&request, &text) {
                Ok(outputs) => outputs,
                Err(error) => {
                    let prompt = correction_prompt(&prompt, &error);
                    match self.turn(
                        &request,
                        &slot,
                        "output-correction",
                        prompt,
                        Some(&original),
                        &sink,
                    ) {
                        Ok((_, corrected, _)) => {
                            text = corrected;
                            match outputs(&request, &text) {
                                Ok(outputs) => outputs,
                                Err(e) => {
                                    self.cleanup(&original, &seat, &slot.id, &sink)?;
                                    return Err(e);
                                }
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
            };
            if request.judge && request.pick_attempts.is_empty() && parse_verdict(&text).is_err() {
                self.judges.lock().map_err(|e| e.to_string())?.insert(
                    (
                        request.run_id.clone(),
                        request.formation.id.clone(),
                        request.attempt,
                    ),
                    HeldJudge {
                        request: request.clone(),
                        slot: slot.clone(),
                        original,
                        seat,
                        prompt: brief::render(&request, &slot),
                    },
                );
            } else {
                self.cleanup(&original, &seat, &slot.id, &sink)?;
                self.publish(&original, &request, &slot, &sink)?;
            }
            Ok(FormationResult {
                status: "done".into(),
                text,
                outputs,
            })
        })()
        .map_err(|message: String| crate::Error::new("executor_error", message))
    }
    fn correct_judge(
        &self,
        correction: JudgeCorrection,
        sink: ExecutionSink,
    ) -> Result<Option<String>, crate::Error> {
        (|| -> Result<Option<String>, String> {
        let held = self.judges.lock().map_err(|e| e.to_string())?.remove(&(
            correction.run_id,
            correction.node_id,
            correction.attempt,
        ));
        let Some(held) = held else {
            return Ok(None);
        };
        let prompt = format!(
            "{}\nYour previous verdict was rejected: {}. Correct it while preserving its verdict, reason and evidence. Return a fresh complete answer with the output contract, verdict and completion sentinel.\n",
            held.prompt, correction.error
        );
        let result = self.turn(
            &held.request,
            &held.slot,
            "judge-correction",
            prompt,
            Some(&held.original),
            &sink,
        );
        match result {
            Ok((_, text, _)) => {
                self.cleanup(&held.original, &held.seat, &held.slot.id, &sink)?;
                self.publish(&held.original, &held.request, &held.slot, &sink)?;
                Ok(Some(text))
            }
            Err(error) => Err(error),
        }
     })().map_err(|message: String| crate::Error::new("executor_error", message))
    }
}
/// A seat state the coordinator observes about the step's place.
fn place_state(sink: &ExecutionSink, slot: &str, state: &str, detail: &str) -> Result<(), String> {
    let data = json!({"state":state,"detail":detail,"since":crate::run_storage::now()});
    sink.emit(ExecutionEvent {
        kind: "seat_state".into(),
        slot_id: slot.into(),
        data: data.as_object().cloned().unwrap_or_default(),
    })
    .map_err(|e| format!("seat_ledger_unavailable: {e}"))
}
fn report_event(sink: &ExecutionSink, slot: &str, event: SeatEvent) -> Result<(), String> {
    let mut data = event.data;
    data.insert("seatEventSeq".into(), json!(event.seq));
    sink.emit(ExecutionEvent {
        kind: event.kind,
        slot_id: slot.into(),
        data,
    })
    .map_err(|e| format!("seat_ledger_unavailable: {e}"))
}
fn outputs(request: &FormationExecution, text: &str) -> Result<Map<String, Value>, String> {
    formation_outputs(&request.formation, text)
}
fn recovered_result(
    request: &OpenDispatch,
    result: crate::DispatchResult,
) -> Result<FormationResult, String> {
    if result.status != "ok" {
        return Err(result.text);
    }
    Ok(FormationResult {
        status: "done".into(),
        outputs: formation_outputs(&request.formation, &result.text)?,
        text: result.text,
    })
}
fn formation_outputs(
    formation: &parvi_mission_model::Formation,
    text: &str,
) -> Result<Map<String, Value>, String> {
    let (_, payloads) =
        parse_outputs(text).map_err(|e| format!("invalid_output_payloads: {}", e.message))?;
    if payloads.len() != formation.outputs.len()
        || formation
            .outputs
            .iter()
            .any(|port| !payloads.contains_key(&port.id))
    {
        return Err("invalid_output_payloads: expected all and only declared output ports".into());
    }
    let mut result = serde_json::to_value(payloads)
        .map_err(|e| e.to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| "invalid output payloads".to_string())?;
    crate::output_refs::materialize(formation, &mut result).map_err(|e| e.to_string())?;
    Ok(result)
}
fn correction_prompt(prompt: &str, error: &str) -> String {
    format!(
        "{prompt}\nYour previous output was rejected: {error}. Correct the output contract, preserving your work. A text ref must be an absolute path to a UTF-8 file of at most 8192 bytes. To route a path as text, put it in text and omit ref. Return a fresh complete answer and the completion sentinel.\n"
    )
}
/// The output correction owed by an initial turn that completed with a
/// rejected output contract: its dispatch id and the rejection.
fn owed_correction(request: &OpenDispatch, result: &DispatchResult) -> Option<(String, String)> {
    let key = |phase| {
        dispatch_key(
            &request.run_id,
            &request.node_id,
            &request.slot_id,
            request.attempt,
            phase,
        )
    };
    if result.status != "ok" || request.dispatch_id != key("initial") {
        return None;
    }
    formation_outputs(&request.formation, &result.text)
        .err()
        .map(|error| (key("output-correction"), error))
}
#[allow(clippy::too_many_arguments)]
fn dispatch_data(
    id: &str,
    node: &str,
    slot: &Slot,
    seat: &SeatId,
    path: &str,
    prompt: &str,
    phase: &str,
    recorded_before_send: bool,
) -> Map<String, Value> {
    let prompt_sha = hex::encode(Sha256::digest(prompt.as_bytes()));
    let data = json!({"dispatchId":id,"nodeId":node,"slotId":slot.id,"agentId":slot.agent_id,"harness":slot.harness,"sessionStem":slot.id,"sessionRef":format!("terminal:{}",seat.session_name),"briefPath":path,"promptSha256":prompt_sha,"promptRef":"","phase":phase,"recordedBeforeSend":recorded_before_send,"nativeAck":false});
    data.as_object().cloned().unwrap_or_default()
}
fn dispatch_id(request: &FormationExecution, slot: &Slot, phase: &str) -> String {
    dispatch_key(
        &request.run_id,
        &request.formation.id,
        &slot.id,
        request.attempt,
        phase,
    )
}
fn dispatch_key(run: &str, node: &str, slot: &str, attempt: i64, phase: &str) -> String {
    let digest = Sha256::digest(format!("{run}\0{node}\0{slot}\0{attempt}\0{phase}").as_bytes());
    format!("dsp_{}", hex::encode(digest))
}

/// Publishes a placed turn on its node and describes it for the run's ledger.
async fn published_turn(
    seats: &dyn SeatSide,
    dispatch: &str,
    place: &str,
    turn: &crate::TurnWorkspace,
    slot: &str,
) -> Result<ExecutionEvent, String> {
    let published = seats
        .publish_turn(place, turn)
        .await
        .map_err(|error| error.message)?;
    let data = json!({"dispatchId":dispatch,"place":place,"commit":published.commit,"reference":published.reference});
    Ok(ExecutionEvent {
        kind: "workspace_turn".into(),
        slot_id: slot.into(),
        data: data.as_object().cloned().unwrap_or_default(),
    })
}
