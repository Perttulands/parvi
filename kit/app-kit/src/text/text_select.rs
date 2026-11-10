//! A selection across a column of texts, as a reader's drag makes it: an
//! anchor where the press landed and a head where the pointer is, each a
//! byte in one of the column's texts, numbered in reading order. What it
//! covers in each text, and the words it copies, follow from the two.

use std::ops::Range;

/// A place in the column: a text's number and a byte in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub text: usize,
    pub at: usize,
}

/// One of the column's texts as it is copied: its words and what stands
/// between it and the text before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub sep: &'static str,
}

#[derive(Debug, Default)]
pub struct Selection {
    anchor: Option<Spot>,
    head: Option<Spot>,
    /// The button is held since the press.
    pub dragging: bool,
}

impl Selection {
    /// A press: the selection collapses to `at` and follows the pointer.
    pub fn press(&mut self, at: Spot) {
        self.anchor = Some(at);
        self.head = Some(at);
        self.dragging = true;
    }

    /// A shift-press: the head moves to `at` and the anchor stays, if any.
    pub fn extend(&mut self, at: Spot) {
        if self.anchor.is_none() {
            return self.press(at);
        }
        self.head = Some(at);
        self.dragging = true;
    }

    /// The pointer moved to `at` with the button held; whether that changed
    /// the selection.
    pub fn drag(&mut self, at: Spot) -> bool {
        if !self.dragging || self.head == Some(at) {
            return false;
        }
        self.head = Some(at);
        true
    }

    /// The button is let go; whether that ends a drag that selected
    /// something.
    pub fn release(&mut self) -> bool {
        std::mem::take(&mut self.dragging) && !self.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.ends().is_none()
    }

    /// The selection's first and last spot, if it covers anything.
    fn ends(&self) -> Option<(Spot, Spot)> {
        let (a, h) = (self.anchor?, self.head?);
        match a.cmp(&h) {
            std::cmp::Ordering::Less => Some((a, h)),
            std::cmp::Ordering::Greater => Some((h, a)),
            std::cmp::Ordering::Equal => None,
        }
    }

    /// What it covers of the `i`th text, `text`: `None` outside it, a range
    /// on its character boundaries (perhaps empty) within.
    pub fn within(&self, i: usize, text: &str) -> Option<Range<usize>> {
        let (first, last) = self.ends()?;
        if i < first.text || i > last.text {
            return None;
        }
        // A spot from a text since redrawn may lie past it or inside a
        // character.
        let at = |s: usize| text.floor_char_boundary(s.min(text.len()));
        let start = if i == first.text { at(first.at) } else { 0 };
        let end = if i == last.text {
            at(last.at)
        } else {
            text.len()
        };
        Some(start..end.max(start))
    }

    /// The selected words of `pieces`, each text's part after its
    /// separator; `None` when nothing is selected.
    pub fn copied(&self, pieces: &[Piece]) -> Option<String> {
        let (first, _) = self.ends()?;
        let mut out = String::new();
        for (i, p) in pieces.iter().enumerate() {
            let Some(r) = self.within(i, &p.text) else {
                continue;
            };
            if i > first.text {
                out.push_str(p.sep);
            }
            out.push_str(&p.text[r]);
        }
        (!out.is_empty()).then_some(out)
    }
}

/// `styles` (sorted, apart) with `mark` laid over `range`: the styles
/// split at its ends, and the unstyled stretches inside it styled.
pub fn overlay<S: Clone + Default>(
    styles: &[(Range<usize>, S)],
    range: Range<usize>,
    mark: impl Fn(S) -> S,
) -> Vec<(Range<usize>, S)> {
    let mut out = vec![];
    // The unstyled stretch `from..to`, as far as the range covers it.
    let gap = |from: usize, to: usize, out: &mut Vec<(Range<usize>, S)>| {
        let part = from.max(range.start)..to.min(range.end);
        if part.start < part.end {
            out.push((part, mark(S::default())));
        }
    };
    let mut at = 0;
    for (r, s) in styles {
        gap(at, r.start, &mut out);
        // The style's parts before, inside and after the range.
        let cuts = [
            (r.start..r.end.min(range.start), false),
            (r.start.max(range.start)..r.end.min(range.end), true),
            (r.start.max(range.end)..r.end, false),
        ];
        for (part, inside) in cuts {
            if part.start < part.end {
                out.push((part, if inside { mark(s.clone()) } else { s.clone() }));
            }
        }
        at = r.end;
    }
    gap(at, range.end, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(text: usize, at: usize) -> Spot {
        Spot { text, at }
    }

    fn pieces() -> Vec<Piece> {
        vec![
            Piece {
                text: "Heading".into(),
                sep: "\n\n",
            },
            Piece {
                text: "First paragraph.".into(),
                sep: "\n\n",
            },
            Piece {
                text: "Second one.".into(),
                sep: "\n\n",
            },
        ]
    }

    #[test]
    fn a_drag_across_texts_copies_their_parts_with_separators() {
        let mut s = Selection::default();
        s.press(spot(0, 4));
        assert!(s.drag(spot(2, 6)));
        assert!(s.release());
        assert_eq!(
            s.copied(&pieces()).as_deref(),
            Some("ing\n\nFirst paragraph.\n\nSecond")
        );
    }

    #[test]
    fn a_drag_backwards_selects_the_same_words() {
        let mut s = Selection::default();
        s.press(spot(2, 6));
        s.drag(spot(1, 6));
        assert_eq!(s.copied(&pieces()).as_deref(), Some("paragraph.\n\nSecond"));
        assert_eq!(s.within(0, "Heading"), None);
        assert_eq!(s.within(1, "First paragraph."), Some(6..16));
        assert_eq!(s.within(2, "Second one."), Some(0..6));
    }

    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        let mut s = Selection::default();
        s.press(spot(1, 3));
        assert!(!s.drag(spot(1, 3)));
        assert!(!s.release());
        assert!(s.is_empty());
        assert_eq!(s.copied(&pieces()), None);
    }

    #[test]
    fn moves_after_the_release_do_not_drag() {
        let mut s = Selection::default();
        s.press(spot(1, 0));
        s.drag(spot(1, 5));
        s.release();
        assert!(!s.drag(spot(2, 3)));
        assert_eq!(s.copied(&pieces()).as_deref(), Some("First"));
    }

    #[test]
    fn shift_press_extends_from_the_anchor() {
        let mut s = Selection::default();
        s.press(spot(1, 0));
        s.release();
        s.extend(spot(1, 5));
        s.release();
        assert_eq!(s.copied(&pieces()).as_deref(), Some("First"));
    }

    #[test]
    fn a_stale_spot_lands_on_a_character_boundary() {
        let mut s = Selection::default();
        s.press(spot(0, 0));
        s.drag(spot(0, 2));
        // "é" is two bytes; byte 2 is inside it.
        assert_eq!(s.within(0, "hé"), Some(0..1));
        s.drag(spot(0, 99));
        assert_eq!(s.within(0, "hé"), Some(0..3));
    }

    #[test]
    fn overlay_splits_styles_at_the_selection() {
        // Bold over 2..6, plain elsewhere; the selection 4..9.
        let styles = vec![(2..6, 1u8)];
        let out = overlay(&styles, 4..9, |s| s + 10);
        assert_eq!(out, vec![(2..4, 1), (4..6, 11), (6..9, 10)]);
    }

    #[test]
    fn overlay_without_styles_marks_just_the_range() {
        let out = overlay::<u8>(&[], 3..5, |s| s + 10);
        assert_eq!(out, vec![(3..5, 10)]);
    }
}
