//! Owned terminal seats reconstructed from a run's public lifecycle events.
use parvi_missions_contract::RunProjection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    pub run: String,
    pub step: String,
    pub slot: String,
    pub seq: u64,
    pub node: String,
    pub session: String,
    pub name: String,
    pub state: State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Live,
    Ended,
    Unavailable,
}

/// A cleanup can only close the named seat in its own step and slot. Never
/// use the coordinator's node as a substitute for a missing seat identity.
pub fn seats(run: &RunProjection) -> Vec<Seat> {
    let mut events: Vec<_> = run.events.iter().collect();
    events.sort_by_key(|event| event.seq);
    let mut seats: Vec<Seat> = Vec::new();
    for event in events {
        match event.kind.as_str() {
            "seat_created"
                if !event.node_id.is_empty()
                    && !event.seat_node.is_empty()
                    && !event.session_id.is_empty()
                    && !event.session_name.is_empty() =>
            {
                if !seats.iter().any(|seat| {
                    seat.node == event.seat_node
                        && seat.session == event.session_id
                        && seat.name == event.session_name
                }) {
                    seats.push(Seat {
                        run: run.run_id.clone(),
                        step: event.node_id.clone(),
                        slot: event.slot_id.clone(),
                        seq: event.seq,
                        node: event.seat_node.clone(),
                        session: event.session_id.clone(),
                        name: event.session_name.clone(),
                        state: State::Live,
                    });
                }
            }
            "seat_cleanup" => {
                if let Some(seat) = seats.iter_mut().rev().find(|seat| {
                    seat.step == event.node_id
                        && seat.slot == event.slot_id
                        && !event.session_name.is_empty()
                        && seat.name == event.session_name
                }) {
                    seat.state = match event.outcome.as_str() {
                        "ended" | "gone" => State::Ended,
                        // A stopping coordinator leaves the seat running for
                        // the run to reattach, as the engine counts it.
                        "left_shutdown" => State::Live,
                        _ => State::Unavailable,
                    };
                }
            }
            _ => (),
        }
    }
    // A final run without a cleanup record cannot prove its seat still exists.
    if run.r#final {
        for seat in &mut seats {
            if seat.state == State::Live {
                seat.state = State::Unavailable;
            }
        }
    }
    seats
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn lifecycle_keeps_each_seat_identity_and_reports_cleanup() {
        let run: RunProjection = serde_json::from_value(json!({"runId":"run_1","status":"working","final":false,"resumeAllowed":false,"epoch":1,"missionSlug":"m","missionId":"m","missionRev":1,"inputCardId":"i","work":"","cwd":"","repository":"","base":"","contextPaths":[],"inputs":[],"outputs":[],"eventCount":3,"startedBy":"","startedAt":"","updatedAt":"","waitingGates":[],"events":[{"seq":1,"type":"seat_created","nodeId":"work","slotId":"a","seatNode":"n1","sessionId":"$1","sessionName":"same"},{"seq":2,"type":"seat_created","nodeId":"work","slotId":"b","seatNode":"n2","sessionId":"$2","sessionName":"same"},{"seq":3,"type":"seat_cleanup","nodeId":"work","slotId":"a","sessionName":"same","outcome":"ended"}]})).unwrap();
        let seats = seats(&run);
        assert_eq!(seats.len(), 2);
        assert_eq!(
            (seats[0].state, seats[1].state),
            (State::Ended, State::Live)
        );
        assert_eq!(
            (&seats[1].node, &seats[1].session),
            (&"n2".into(), &"$2".into())
        );
    }
    #[test]
    fn a_seat_left_running_by_a_coordinator_shutdown_can_still_be_opened() {
        let run: RunProjection = serde_json::from_value(json!({"runId":"run_1","status":"blocked","final":false,"resumeAllowed":true,"epoch":1,"missionSlug":"m","missionId":"m","missionRev":1,"inputCardId":"i","work":"","cwd":"","repository":"","base":"","contextPaths":[],"inputs":[],"outputs":[],"eventCount":2,"startedBy":"","startedAt":"","updatedAt":"","waitingGates":[],"events":[{"seq":1,"type":"seat_created","nodeId":"work","slotId":"a","seatNode":"n1","sessionId":"$1","sessionName":"seat"},{"seq":2,"type":"seat_cleanup","nodeId":"work","slotId":"a","sessionName":"seat","outcome":"left_shutdown"}]})).unwrap();
        assert_eq!(seats(&run)[0].state, State::Live);
    }
}
