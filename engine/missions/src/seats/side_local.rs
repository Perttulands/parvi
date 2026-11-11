//! Local implementation of the dispatch-addressed seat interface.
use super::{
    local::LocalSeats,
    side::{DispatchId, SeatFuture, SeatSide},
};
use crate::{AgentSeat, Dispatch, DispatchResult, PublishedTurn, SeatEvent, SeatId, TurnWorkspace};
use serde_json::json;
use std::{io, path::PathBuf, sync::atomic::AtomicBool, time::Duration};
use tokio::sync::mpsc;

impl SeatSide for LocalSeats {
    fn dispatch(&self, dispatch: Dispatch) -> SeatFuture<'_, DispatchId> {
        Box::pin(async move {
            if let Some(original) = &dispatch.reuse_dispatch {
                self.reuse(&dispatch, original).await?;
            } else {
                LocalSeats::dispatch(self, dispatch.clone()).await?;
            }
            let seat = self
                .records()
                .seat(&dispatch.id)
                .map_err(|e| e.to_string())?
                .ok_or("dispatch has no seat")?;
            let path = self
                .records()
                .brief_path(&dispatch.id)
                .map_err(|e| e.to_string())?;
            let data = json!({"seat":seat,"briefPath":path});
            self.records()
                .append(
                    &dispatch.id,
                    "seat-prepared",
                    "seat_prepared",
                    data.as_object().cloned().unwrap_or_default(),
                )
                .map_err(|e| e.to_string())?;
            Ok(dispatch.id)
        })
    }
    fn events(
        &self,
        id: &str,
        since: u64,
    ) -> Result<mpsc::Receiver<Result<SeatEvent, String>>, String> {
        self.records().dispatch(id).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel(64);
        let records = self.records().clone();
        let id = id.to_owned();
        self.runtime.spawn(async move {
            let mut cursor = since;
            loop {
                if tx.is_closed() {
                    break;
                }
                let read_records = records.clone();
                let read_id = id.clone();
                let batch =
                    tokio::task::spawn_blocking(move || read_records.events(&read_id, cursor))
                        .await;
                match batch {
                    Ok(Ok(events)) => {
                        for event in events {
                            cursor = event.seq;
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        let _ = tx.send(Err(error.to_string())).await;
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string())).await;
                        return;
                    }
                }
                tokio::select! { _ = tx.closed() => break, _ = tokio::time::sleep(Duration::from_millis(250)) => () }
            }
        });
        Ok(rx)
    }
    fn stage(&self, id: &str, text: &str) -> SeatFuture<'_, ()> {
        let id = id.to_owned();
        let text = text.to_owned();
        Box::pin(async move {
            let request = self.records().dispatch(&id).map_err(|e| e.to_string())?;
            if !text.is_empty() && request.brief != text {
                return Err("dispatch brief differs from the recorded brief".into());
            }
            if self
                .records()
                .result(&id)
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Ok(());
            }
            {
                let mut running = self.running.lock().map_err(|e| e.to_string())?;
                if !running.insert(id.clone()) {
                    return Ok(());
                }
            }
            let owner = self.clone();
            let worker_owner = owner.clone();
            let worker_id = id.clone();
            let worker = self.runtime.spawn(async move {
                worker_owner
                    .run(&worker_id, &AtomicBool::new(false), |_| Ok(()))
                    .await
            });
            self.runtime.spawn(async move {
                let result = worker
                    .await
                    .map_err(|error| format!("seat worker failed: {error}"))
                    .and_then(|result| result);
                if let Err(error) = result {
                    let data = json!({"dispatchId":id,"detail":error});
                    let _ = owner.records().append(
                        &id,
                        "seat-failed",
                        "seat_failed",
                        data.as_object().cloned().unwrap_or_default(),
                    );
                    let _ = owner.records().finish(
                        &id,
                        DispatchResult {
                            status: "error".into(),
                            text: error,
                        },
                    );
                }
                if let Ok(mut running) = owner.running.lock() {
                    running.remove(&id);
                }
            });
            Ok(())
        })
    }
    fn tokens(&self, id: &str) -> Result<Option<i64>, String> {
        let records = self.records();
        let dispatch = records.dispatch(id).map_err(|e| e.to_string())?;
        let events = records.events(id, 0).map_err(|e| e.to_string())?;
        // Until the seat took its brief there is no transcript to count.
        let Some(consumed) = events.iter().find(|event| event.key == "prompt-consumed") else {
            return Ok(None);
        };
        let Some(transcript) = consumed.data.get("transcript").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        let path = records.brief_path(id).map_err(|e| e.to_string())?;
        let pointer = crate::Pointer {
            text: super::run::brief_pointer(&path),
            cwd: dispatch.cwd.clone(),
            transcript: Some(transcript.into()),
            session_id: (consumed.data.get("nativeSessionId"))
                .and_then(|v| v.as_str())
                .map(Into::into),
            ..Default::default()
        };
        let (_, adapter) = super::local::adapter(&dispatch.harness)?;
        let usage = adapter.tokens_since(&pointer).map_err(|e| e.to_string())?;
        Ok(Some(usage.counted()))
    }
    fn notify(&self, id: &str, text: &str) -> SeatFuture<'_, ()> {
        let id = id.to_owned();
        let text = text.to_owned();
        Box::pin(async move {
            let seat = (self.records().seat(&id))
                .map_err(|e| e.to_string())?
                .ok_or("dispatch has no seat")?;
            self.engine()
                .deliver(seat.session.pane_id, text, true)
                .await
                .map_err(|e| e.to_string())?
                .await
                .map_err(|e| e.to_string())
        })
    }
    fn dispatch_info(&self, id: &str) -> Result<Dispatch, String> {
        self.records().dispatch(id).map_err(|e| e.to_string())
    }
    fn result(&self, id: &str) -> Result<Option<DispatchResult>, String> {
        self.records().result(id).map_err(|e| e.to_string())
    }
    fn end_seat(&self, seat: &SeatId, cause: &str) -> SeatFuture<'_, String> {
        let seat = seat.clone();
        let cause = cause.to_owned();
        Box::pin(async move {
            let id = self
                .records()
                .dispatch_for_seat(&seat)
                .map_err(|e| e.to_string())?;
            if cause == "shutdown" {
                return self.detach(&id);
            }
            LocalSeats::end_seat(self, &id, &cause).await
        })
    }
    fn prepare_turn(&self, _: &str, turn: &TurnWorkspace) -> SeatFuture<'_, PathBuf> {
        let turns = self.turns.clone();
        let turn = turn.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || turns.prepare(&turn))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
        })
    }
    fn publish_turn(&self, _: &str, turn: &TurnWorkspace) -> SeatFuture<'_, PublishedTurn> {
        let turns = self.turns.clone();
        let turn = turn.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || turns.publish(&turn))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
        })
    }
    fn seats(&self, run_id: &str) -> io::Result<Vec<AgentSeat>> {
        let mut seats = self.records().seats(run_id)?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    self.runtime.block_on(async {
                        for seat in &mut seats {
                            if seat.available {
                                let identity = self.records().seat(&seat.dispatch_id)?;
                                seat.available = match identity {
                                    Some(identity) => {
                                        self.engine().verify(&identity.session).await.is_ok()
                                    }
                                    None => false,
                                };
                            }
                            if !seat.available {
                                seat.session.clear();
                                seat.pane = None;
                            }
                        }
                        Ok::<(), io::Error>(())
                    })
                })
                .join()
                .map_err(|_| io::Error::other("seat inspection panicked"))?
        })?;
        Ok(seats)
    }
}
