//! Playing an agent report's tone (`store::tones`). On Windows, a worker
//! plays reports in order so one report cannot cut off another. The Linux
//! build runs only on invisible test displays and logs instead.

use crate::store::tones;
use parvi_terminals_contract::EventKind;

#[cfg(any(windows, test))]
fn worker<T: Send + 'static>(
    mut play: impl FnMut(T) + Send + 'static,
) -> std::sync::mpsc::Sender<T> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for item in receive {
            play(item);
        }
    });
    send
}

#[cfg(windows)]
pub fn play(kind: EventKind) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
    static PLAYER: OnceLock<std::sync::mpsc::Sender<EventKind>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        worker(|kind| {
            let file = tones::wav(kind);
            // SAFETY: synchronous playback returns before the WAV is dropped.
            let played = unsafe {
                PlaySoundW(
                    file.as_ptr().cast(),
                    std::ptr::null_mut(),
                    SND_MEMORY | SND_SYNC | SND_NODEFAULT,
                )
            };
            if played == 0 {
                log::warn!("the {} tone could not be played", word(kind));
            }
        })
    });
    if player.send(kind).is_err() {
        log::warn!("the {} tone could not be queued", word(kind));
    }
}

#[cfg(not(windows))]
pub fn play(kind: EventKind) {
    let notes: Vec<String> = tones::schedule(kind)
        .iter()
        .map(|n| format!("{} Hz at {} ms", n.frequency, (n.at * 1000.).round()))
        .collect();
    log::warn!(
        "tone {} not played on this system: {} bytes of WAV, {}",
        word(kind),
        tones::wav(kind).len(),
        notes.join(", ")
    );
}

fn word(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Finished => "finished",
        EventKind::NeedsInput => "needs-input",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn reports_queue_without_waiting_and_play_in_order() {
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let player = worker(move |kind| {
            started.send(kind).unwrap();
            released.recv().unwrap();
        });
        player.send(EventKind::Finished).unwrap();
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            EventKind::Finished
        );
        // The first sound is still playing, but accepting the next does not wait.
        player.send(EventKind::NeedsInput).unwrap();
        assert!(matches!(starts.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            EventKind::NeedsInput
        );
        release.send(()).unwrap();
    }
}
