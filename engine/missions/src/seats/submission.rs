//! Decisions while submitting one pasted brief pointer.
use parvi_term::{
    Screen,
    waiting::harness::{self, Harness},
};
use std::time::{Duration, Instant};

/// An Enter is safe only while the complete input is still the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionAction {
    Wait,
    Enter,
    Consumed,
}

pub struct Submission {
    pointer: String,
    presses: u8,
    last_press: Option<Instant>,
}
impl Submission {
    pub fn new(pointer: &str) -> Self {
        Self {
            pointer: harness::rendered(pointer),
            presses: 0,
            last_press: None,
        }
    }
    pub fn observe(&self, harness: Harness, screen: &Screen, now: Instant) -> SubmissionAction {
        let Some(text) = harness::input(harness, screen) else {
            return SubmissionAction::Wait;
        };
        if self.presses > 0 && text.is_empty() {
            return SubmissionAction::Consumed;
        }
        if !self.pointer.is_empty()
            && text == self.pointer
            && self.presses < 5
            && self.last_press.is_none_or(|last| {
                now.saturating_duration_since(last) >= Duration::from_millis(1500)
            })
        {
            return SubmissionAction::Enter;
        }
        SubmissionAction::Wait
    }
    /// Record a successful Enter delivery before observing the next screen.
    pub fn pressed(&mut self, now: Instant) {
        self.presses += 1;
        self.last_press = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_term::{Cell, Color, Cursor};
    fn screen(text: &str) -> Screen {
        let cells = format!("› {text}")
            .chars()
            .map(|c| Cell {
                text: c.to_string(),
                width: 1,
                foreground: Color::Default,
                background: Color::Default,
                bold: false,
                dim: false,
                italic: false,
                inverse: false,
            })
            .collect();
        Screen {
            rows: vec![cells],
            cursor: Cursor {
                row: 0,
                column: (text.chars().count() + 2) as u16,
                visible: true,
            },
        }
    }
    #[test]
    fn submit_only_the_exact_pointer_and_limit_retries() {
        let mut submit = Submission::new("read /tmp/brief");
        let now = Instant::now();
        assert_eq!(
            submit.observe(Harness::Codex, &screen(""), now),
            SubmissionAction::Wait
        );
        assert_eq!(
            submit.observe(Harness::Codex, &screen("read /tmp/brief operator"), now),
            SubmissionAction::Wait
        );
        let pointer = screen("read /tmp/brief");
        for i in 0..5 {
            let time = now + Duration::from_secs(i * 2);
            assert_eq!(
                submit.observe(Harness::Codex, &pointer, time),
                SubmissionAction::Enter
            );
            submit.pressed(time);
            assert_eq!(
                submit.observe(Harness::Codex, &pointer, time),
                SubmissionAction::Wait
            );
        }
        assert_eq!(
            submit.observe(Harness::Codex, &pointer, now + Duration::from_secs(20)),
            SubmissionAction::Wait
        );
        assert_eq!(
            submit.observe(Harness::Codex, &screen(""), now + Duration::from_secs(20)),
            SubmissionAction::Consumed
        );
    }
}
