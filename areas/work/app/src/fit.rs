//! How the Work tab fits its pane (#241), after CHROTE's `BeadsView.css`
//! panes and `BeadTable.tsx` compact table: the card stays beside the table
//! at its own width, gives way down to a narrowest card so the table keeps
//! its least, and goes beneath the table when the pane holds neither. The
//! table then leaves columns out, lowest first, until Title has its room.

use crate::store::work::{COLUMNS, Column};

use crate::tokens::*;

/// What goes first when Title runs short: Updated (Idle says it in days)
/// and Size as CHROTE's compact table, then what the card says again.
/// ID and Title always stay.
const DROPS: [Column; 7] = [
    Column::Updated,
    Column::Size,
    Column::Type,
    Column::Kids,
    Column::Idle,
    Column::Project,
    Column::Status,
];

/// A column's fixed width; Title takes what is left.
pub fn width(c: Column) -> Option<f32> {
    match c {
        Column::Id => Some(COLUMN_ID_W),
        Column::Type => Some(COLUMN_TYPE_W),
        Column::Title => None,
        Column::Project => Some(COLUMN_PROJECT_W),
        Column::Status => Some(COLUMN_STATUS_W),
        Column::Updated => Some(COLUMN_UPDATED_W),
        Column::Idle => Some(COLUMN_IDLE_W),
        Column::Size => Some(COLUMN_SIZE_W),
        Column::Kids => Some(COLUMN_KIDS_W),
    }
}

/// Where the card goes in a pane this wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    /// Beside the table, this wide.
    Beside(f32),
    /// Beneath the table, the pane's whole width, each with half its height.
    Beneath,
}

pub fn side(pane: f32) -> Side {
    if pane >= TABLE_MIN + CARD_MIN {
        Side::Beside(CARD_W.min(pane - TABLE_MIN))
    } else {
        Side::Beneath
    }
}

/// The table's columns at this width, in their order.
pub fn columns(table: f32) -> Vec<Column> {
    let mut kept = COLUMNS.to_vec();
    let need = |kept: &[Column]| {
        let fixed: f32 = kept.iter().filter_map(|&c| width(c)).sum();
        2. * ROW_PAD + ROW_EDGE + fixed + GAP * (kept.len() - 1) as f32 + TITLE_MIN
    };
    for drop in DROPS {
        if need(&kept) <= table {
            break;
        }
        kept.retain(|&c| c != drop);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use Column::*;

    #[test]
    fn the_card_gives_way_then_goes_beneath() {
        assert_eq!(side(1400.), Side::Beside(520.));
        assert_eq!(side(1140.), Side::Beside(520.));
        // 1600 px with the Beads column open: 1600 - 240 rail - 360 column.
        assert_eq!(side(1000.), Side::Beside(380.));
        assert_eq!(side(980.), Side::Beside(360.));
        assert_eq!(side(979.), Side::Beneath);
    }

    #[test]
    fn columns_go_lowest_first_until_title_has_its_room() {
        assert_eq!(columns(1000.), COLUMNS.to_vec());
        assert_eq!(
            columns(900.),
            vec![Id, Type, Title, Project, Status, Idle, Size, Kids]
        );
        assert_eq!(
            columns(850.),
            vec![Id, Type, Title, Project, Status, Idle, Kids]
        );
        assert_eq!(columns(750.), vec![Id, Title, Project, Status, Idle, Kids]);
        assert_eq!(columns(700.), vec![Id, Title, Project, Status, Idle]);
        // The table beside the card at 1600 px with the column open.
        assert_eq!(columns(620.), vec![Id, Title, Project, Status]);
        assert_eq!(columns(500.), vec![Id, Title, Status]);
        assert_eq!(columns(300.), vec![Id, Title]);
        assert_eq!(columns(0.), vec![Id, Title]);
    }

    #[test]
    fn title_keeps_its_least_wherever_a_column_could_go() {
        for w in (0..2000).step_by(10) {
            let kept = columns(w as f32);
            let fixed: f32 = kept.iter().filter_map(|&c| width(c)).sum();
            let title = w as f32 - 2. * ROW_PAD - ROW_EDGE - fixed - GAP * (kept.len() - 1) as f32;
            assert!(
                title >= TITLE_MIN || kept == vec![Id, Title],
                "{w} px leaves Title {title} px with {kept:?}"
            );
        }
    }
}
