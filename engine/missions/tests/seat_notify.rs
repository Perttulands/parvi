//! A message to a running seat goes to its pane only after the seat's
//! session is proven to be the one recorded: after a tmux server restart
//! the recorded `%N` can be another program's pane.
use parvi_mission_engine::{Dispatch, LocalSeats, SeatId, SeatSide, TranscriptRoots};
use parvi_term::{Generation, SessionRef, Sessions};
use parvi_testkit::{node::Node, sessions};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_not_pasted_into_a_pane_the_seat_no_longer_owns() {
    let node = Node::start(&[sessions::LISTING], "").await.unwrap();
    let fake = node.fake_sessions();
    // Someone else's session now holds the pane id the seat recorded. The
    // fake proves no seat's identity, as a restarted server would not.
    fake.add("someone-else");
    let pane = fake.list()[0].panes[0].0;
    let root = node.dir().join("seats");
    let engine: Arc<dyn Sessions> = fake.clone();
    let seats = LocalSeats::new(engine, &root, "local".into(), TranscriptRoots::from_host());
    let dispatch = Dispatch {
        id: "dsp_notify".into(),
        run_id: "run_1".into(),
        node_id: "step".into(),
        slot_id: "worker".into(),
        harness: "claude".into(),
        model: "model".into(),
        effort: "medium".into(),
        cwd: "/tmp".into(),
        brief: "brief".into(),
        reuse_dispatch: None,
        place: String::new(),
        turn: None,
    };
    seats.records().create(&dispatch).unwrap();
    let seat = SeatId {
        node: "local".into(),
        session_name: "parvi-run-worker".into(),
        session: SessionRef {
            session_id: "$9".into(),
            pane_id: pane,
            generation: Generation {
                pid: 1,
                start_time: 1,
            },
        },
    };
    seats.records().bind_seat(&dispatch.id, seat).unwrap();

    let sent = SeatSide::notify(&seats, &dispatch.id, "Worker gone.").await;
    assert!(
        sent.is_err(),
        "the message was pasted into an unproven pane"
    );
    assert_eq!(fake.delivered(), vec![]);
    node.stop().await;
}
