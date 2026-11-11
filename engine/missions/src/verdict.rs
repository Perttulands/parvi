//! Atomic decisions on pending human gate requests.
use crate::{Coordinator, RunEvent, RunProjection, run_storage::now};
use serde::Deserialize;
use serde_json::json;
use std::io;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct HumanVerdict {
    pub requested_seq: u64,
    pub verdict: String,
    #[serde(rename = "reason")]
    pub response: String,
    pub relayed_by: String,
    pub actor: String,
}

impl Coordinator {
    pub fn record_verdict(
        &self,
        run: &str,
        gate: &str,
        request: HumanVerdict,
    ) -> Result<RunProjection, crate::Error> {
        (|| -> io::Result<RunProjection> {
        let relay = request.relayed_by.as_bytes();
        if request.requested_seq == 0
            || !["pass", "fail"].contains(&request.verdict.as_str())
            || (!relay.is_empty()
                && (relay.len() > 64
                    || !relay[0].is_ascii_alphanumeric()
                    || !relay
                        .iter()
                        .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "requestedSeq, pass or fail verdict and valid relayedBy required",
            ));
        }
        let control = self.inner.controls.lock().unwrap().get(run).cloned();
        let guard = control.as_ref().map(|control| control.done.lock().unwrap());
        if control
            .as_ref()
            .is_some_and(|control| control.canceled.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "run is final or aborting",
            ));
        }
        let event = RunEvent {
            kind: "human_verdict_recorded".into(),
            gate_id: gate.into(),
            node_id: gate.into(),
            timestamp: now(),
            actor: if request.actor.is_empty() { "human:operator".into() } else { request.actor.clone() },
            data: json!({"requestedSeq":request.requested_seq,"verdict":request.verdict,"reason":request.response,"decidedBy":"human:operator","relayedBy":request.relayed_by}).as_object().unwrap().clone(),
            ..Default::default()
        };
        let appended = self.ledger(run)?.append_checked(event, |events| {
            crate::gate_replay::validate(events)?;
            crate::human_replay::validate(events)?;
            crate::code_binding::validate_run(self, run, events)?;
            let projection = RunProjection::from_events(events, run).map_err(crate::error::to_io)?;
            if projection.r#final
                || !projection.waiting_gates.iter().any(|pending| {
                    pending.gate_id == gate && pending.requested_seq == request.requested_seq
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "human gate request is no longer pending",
                ));
            }
            if events.iter().any(|event| {
                event.seq == request.requested_seq && crate::run_pick::is_pick(event)
            }) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "this gate waits for the operator to pick the best attempt, not for a verdict: answer it with gate pick",
                ));
            }
            Ok(())
        })?;
        drop(guard);
        self.changed(run, appended.seq);
        if let Some(control) = self.inner.controls.lock().unwrap().get(run)
            && let Some(wake) = control.wake.lock().unwrap().as_ref()
        {
            let _ = wake.send(());
        }
        self.run(run).map_err(crate::error::to_io)
     })().map_err(crate::error::io)
    }
}
