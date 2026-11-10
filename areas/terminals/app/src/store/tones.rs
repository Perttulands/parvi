//! The tone an agent report plays, as CHROTE's `tones.ts` makes it: made on
//! the spot, so no audio file is shipped. Finished is a two-note fall,
//! needs-input a single higher note, 120 ms a note. Here it is written out
//! as a WAV the system plays (`crate::sound`), so a test reads exactly what
//! would be heard.

use parvi_terminals_contract::EventKind;

pub const NOTE_SECONDS: f32 = 0.12;
const LEVEL: f32 = 0.2;
/// A 10 ms rise and a 20 ms fall keep a note from clicking at its edges.
const RISE: f32 = 0.01;
const FALL: f32 = 0.02;
pub const RATE: u32 = 22_050;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub frequency: f32,
    /// Seconds after the tone starts.
    pub at: f32,
}

pub fn schedule(kind: EventKind) -> &'static [Note] {
    match kind {
        // E5 falling to A4.
        EventKind::Finished => &[
            Note {
                frequency: 659.25,
                at: 0.,
            },
            Note {
                frequency: 440.,
                at: NOTE_SECONDS,
            },
        ],
        // A5, above both.
        EventKind::NeedsInput => &[Note {
            frequency: 880.,
            at: 0.,
        }],
    }
}

/// The tone as mono samples at [`RATE`]: each note a sine under its
/// envelope.
pub fn samples(kind: EventKind) -> Vec<i16> {
    let notes = schedule(kind);
    let end = notes.iter().map(|n| n.at + NOTE_SECONDS).fold(0., f32::max);
    let mut out = vec![0f32; (end * RATE as f32).round() as usize];
    for note in notes {
        let first = (note.at * RATE as f32).round() as usize;
        let count = (NOTE_SECONDS * RATE as f32).round() as usize;
        for (i, slot) in out[first..first + count].iter_mut().enumerate() {
            let t = i as f32 / RATE as f32;
            *slot += envelope(t) * (std::f32::consts::TAU * note.frequency * t).sin();
        }
    }
    out.iter()
        .map(|v| (v * f32::from(i16::MAX)).round() as i16)
        .collect()
}

/// The note's loudness `t` seconds into it.
fn envelope(t: f32) -> f32 {
    let rise = (t / RISE).min(1.);
    let fall = ((NOTE_SECONDS - t) / FALL).min(1.);
    LEVEL * rise.min(fall).max(0.)
}

/// The tone as a 16-bit mono PCM WAV file.
pub fn wav(kind: EventKind) -> Vec<u8> {
    let pcm: Vec<u8> = samples(kind).iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes a second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes a frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How often the samples cross zero going up, a second: a note's pitch.
    fn pitch(samples: &[i16]) -> f32 {
        let ups = samples.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        ups as f32 / (samples.len() as f32 / RATE as f32)
    }

    fn note_samples(all: &[i16], index: usize) -> &[i16] {
        let count = (NOTE_SECONDS * RATE as f32).round() as usize;
        &all[index * count..(index + 1) * count]
    }

    #[test]
    fn finished_is_two_notes_falling_120_ms_each_the_second_as_the_first_stops() {
        let notes = schedule(EventKind::Finished);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].at, 0.);
        assert_eq!(notes[1].at, notes[0].at + NOTE_SECONDS);
        assert!(notes[0].frequency > notes[1].frequency);
        assert_eq!(NOTE_SECONDS, 0.12);

        let played = samples(EventKind::Finished);
        assert_eq!(played.len(), (0.24 * RATE as f32).round() as usize);
        let (first, second) = (note_samples(&played, 0), note_samples(&played, 1));
        assert!((pitch(first) - 659.25).abs() < 20., "{}", pitch(first));
        assert!((pitch(second) - 440.).abs() < 20., "{}", pitch(second));
    }

    #[test]
    fn needs_input_is_one_note_higher_than_either_note_of_finished() {
        let notes = schedule(EventKind::NeedsInput);
        assert_eq!(notes.len(), 1);
        let finished = schedule(EventKind::Finished);
        assert!(notes[0].frequency > finished.iter().map(|n| n.frequency).fold(0., f32::max));

        let played = samples(EventKind::NeedsInput);
        assert_eq!(played.len(), (NOTE_SECONDS * RATE as f32).round() as usize);
        assert!((pitch(&played) - 880.).abs() < 20., "{}", pitch(&played));
    }

    #[test]
    fn each_note_rises_from_silence_and_falls_back_to_it_never_above_its_level() {
        let played = samples(EventKind::Finished);
        let peak = (LEVEL * f32::from(i16::MAX)).round() as i32;
        for index in 0..2 {
            let note = note_samples(&played, index);
            assert_eq!(note[0], 0, "no click on");
            assert!(
                i32::from(note[note.len() - 1]).abs() < peak / 20,
                "no click off"
            );
            let loudest = note.iter().map(|s| i32::from(*s).abs()).max().unwrap();
            assert!(loudest <= peak && loudest > peak * 9 / 10, "{loudest}");
            // 5 ms in, the note is still rising.
            let early = note[..RATE as usize / 200]
                .iter()
                .map(|s| i32::from(*s).abs())
                .max();
            assert!(early.unwrap() < peak * 6 / 10);
        }
    }

    #[test]
    fn the_wav_says_16_bit_mono_at_the_rate_and_carries_every_sample() {
        let file = wav(EventKind::NeedsInput);
        let pcm = samples(EventKind::NeedsInput);
        assert_eq!(&file[0..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        let u32_at = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap());
        let u16_at = |i: usize| u16::from_le_bytes(file[i..i + 2].try_into().unwrap());
        assert_eq!(u32_at(4) as usize, file.len() - 8);
        assert_eq!((u16_at(20), u16_at(22)), (1, 1), "PCM, mono");
        assert_eq!(u32_at(24), RATE);
        assert_eq!(u16_at(34), 16);
        assert_eq!(&file[36..40], b"data");
        assert_eq!(u32_at(40) as usize, pcm.len() * 2);
        assert_eq!(file.len(), 44 + pcm.len() * 2);
        assert_eq!(
            i16::from_le_bytes([file[44 + 200], file[45 + 200]]),
            pcm[100]
        );
    }
}
