//! A menu's rows. Each row carries what it does, so the shell that draws
//! the menu knows nothing of what its rows are about.

use crate::chords::Run;
use gpui::{App, Window};
use std::rc::Rc;

#[derive(Clone)]
pub struct MenuRow {
    pub label: String,
    pub chord: Option<String>,
    pub run: Option<Run>,
    /// Destroys something: the first click arms it, the second runs it.
    pub danger: bool,
    pub checked: bool,
    /// Offered and refused; its hint says why.
    pub refused: bool,
    /// A line under the label: what the row does, or why it is refused.
    pub hint: Option<String>,
    /// The rows it opens in its place.
    pub sub: Vec<MenuRow>,
}

impl MenuRow {
    pub fn new(label: impl Into<String>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        let mut row = Self::note(label);
        row.run = Some(Rc::new(run));
        row
    }
    /// A row that opens `rows` in the menu's place; `state` reads beside it.
    pub fn sub(label: impl Into<String>, state: Option<String>, rows: Vec<MenuRow>) -> Self {
        let mut row = Self::note(label);
        row.chord = Some(match state {
            Some(s) => format!("{s} ›"),
            None => "›".into(),
        });
        row.sub = rows;
        row
    }
    /// A row that does nothing now, saying why.
    pub fn refused(mut self, why: impl Into<String>) -> Self {
        self.refused = true;
        self.hint = Some(why.into());
        self
    }
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }
    /// A row that only says something.
    pub fn note(label: impl Into<String>) -> Self {
        let mut row = Self::rule();
        row.label = label.into();
        row
    }
    pub fn chord(mut self, c: &str) -> Self {
        self.chord = Some(c.into());
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn rule() -> Self {
        Self {
            label: String::new(),
            chord: None,
            run: None,
            danger: false,
            checked: false,
            refused: false,
            hint: None,
            sub: vec![],
        }
    }
    pub fn is_rule(&self) -> bool {
        self.run.is_none() && self.sub.is_empty() && self.label.is_empty()
    }
    /// A note, or a row refused now: it reads dim and takes no click.
    pub fn inert(&self) -> bool {
        self.refused || (self.run.is_none() && self.sub.is_empty())
    }
}

/// An open menu: its rows, where it stands, and the danger row armed.
#[derive(Clone)]
pub struct Menu {
    pub rows: Vec<MenuRow>,
    pub at: gpui::Point<gpui::Pixels>,
    pub armed: Option<usize>,
}

/// What a click on row `index` of a menu comes to.
pub enum Pick {
    /// Nothing: a note, a rule or a refused row.
    Nothing,
    /// The menu now shows the row's sub-rows, or its danger row is armed.
    Stays,
    /// The menu closes and the row runs.
    Run(Run),
}

impl Menu {
    pub fn pick(&mut self, index: usize) -> Pick {
        let Some(row) = self.rows.get(index).cloned() else {
            return Pick::Nothing;
        };
        if row.inert() {
            return Pick::Nothing;
        }
        if !row.sub.is_empty() {
            self.rows = row.sub;
            self.armed = None;
            return Pick::Stays;
        }
        if row.danger && self.armed != Some(index) {
            self.armed = Some(index);
            return Pick::Stays;
        }
        match row.run {
            Some(run) => Pick::Run(run),
            None => Pick::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_danger_row_runs_on_the_second_click_and_a_sub_row_opens_in_place() {
        let mut m = Menu {
            rows: vec![
                MenuRow::note("Says"),
                MenuRow::new("Kill", |_, _| {}).danger(),
                MenuRow::sub(
                    "Windows",
                    Some("2".into()),
                    vec![MenuRow::new("1", |_, _| {})],
                ),
                MenuRow::new("Claim", |_, _| {}).refused("Nothing to claim."),
            ],
            at: Default::default(),
            armed: None,
        };
        assert!(matches!(m.pick(0), Pick::Nothing));
        assert!(matches!(m.pick(3), Pick::Nothing), "refused");
        assert!(matches!(m.pick(1), Pick::Stays));
        assert_eq!(m.armed, Some(1));
        assert!(matches!(m.pick(1), Pick::Run(_)));
        assert!(matches!(m.pick(2), Pick::Stays));
        assert_eq!(m.rows.len(), 1);
        assert!(matches!(m.pick(0), Pick::Run(_)));
        assert!(MenuRow::rule().is_rule() && !MenuRow::note("x").is_rule());
    }
}
