//! Dispatches whose completion has not been recorded, in dispatch order.
use crate::RunEvent;

pub(crate) fn open(events: &[RunEvent]) -> Vec<RunEvent> {
    let mut dispatches: Vec<RunEvent> = Vec::new();
    for event in events {
        let id = event
            .data
            .get("dispatchId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }
        match event.kind.as_str() {
            "slot_dispatch" => dispatches.push(event.clone()),
            "slot_result" => {
                dispatches.retain(|d| d.data.get("dispatchId").and_then(|v| v.as_str()) != Some(id))
            }
            _ => {}
        }
    }
    dispatches
}

/// Open dispatches and completed dispatches whose output was never retained.
pub(crate) fn unresolved(events: &[RunEvent]) -> Vec<RunEvent> {
    let mut dispatches = open(events);
    for dispatch in events.iter().filter(|event| event.kind == "slot_dispatch") {
        let id = dispatch.data.get("dispatchId").and_then(|v| v.as_str());
        let completed = events.iter().any(|event| {
            event.kind == "slot_result"
                && event.data.get("dispatchId").and_then(|v| v.as_str()) == id
                && event.data.get("status").and_then(|v| v.as_str()) == Some("ok")
        });
        let abandoned = events.iter().any(|event| {
            event.kind == "slot_result"
                && event.data.get("dispatchId").and_then(|v| v.as_str()) == id
                && event.data.get("status").and_then(|v| v.as_str()) == Some("abandoned")
        });
        let retained = events.iter().any(|event| {
            matches!(
                event.kind.as_str(),
                "node_output" | "node_output_pending" | "output_capture_pending"
            ) && event.node_id == dispatch.node_id
                && event.attempt == dispatch.attempt
        });
        if id.is_some()
            && completed
            && !abandoned
            && !retained
            && !dispatches.iter().any(|event| event.seq == dispatch.seq)
        {
            dispatches.push(dispatch.clone());
        }
    }
    dispatches.sort_by_key(|event| event.seq);
    dispatches
}

/// A completed slot followed by a recorded step error is not evidence of a
/// successful formation. Its dispatch must be abandoned before a retry.
pub(crate) fn failed_completion(events: &[RunEvent], dispatch: &RunEvent) -> bool {
    let id = dispatch.data.get("dispatchId").and_then(|v| v.as_str());
    events.iter().any(|event| {
        event.kind == "slot_result"
            && event.data.get("dispatchId").and_then(|v| v.as_str()) == id
            && event.data.get("status").and_then(|v| v.as_str()) == Some("ok")
    }) && events.iter().any(|event| {
        event.kind == "error"
            && event.node_id == dispatch.node_id
            && event.seq > dispatch.seq
            && !events.iter().any(|later| {
                later.kind == "node_started"
                    && later.node_id == dispatch.node_id
                    && later.seq > dispatch.seq
                    && later.seq < event.seq
            })
    })
}
