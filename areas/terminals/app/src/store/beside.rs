//! How wide a view beside the windows is: as the operator last dragged it,
//! else half the room, whatever the view (a picture, a diagram, a panel
//! all open alike); never under its least, and never so wide the windows
//! keep less than one window's width.

use crate::theme::size;

/// The share of the room a view takes until the operator drags its edge.
pub const HALF: f32 = 0.5;

pub fn width(remembered: Option<f32>, room: f32) -> f32 {
    let most = (room - size::WINDOW_MIN).max(size::BESIDE_MIN);
    let wanted = remembered.filter(|w| w.is_finite()).unwrap_or(room * HALF);
    wanted.clamp(size::BESIDE_MIN, most)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_opens_at_half_the_room() {
        assert_eq!(width(None, 1600.), 800.);
        assert_eq!(width(None, 2. * size::BESIDE_MIN - 2.), size::BESIDE_MIN);
    }

    #[test]
    fn a_dragged_width_is_kept_while_the_windows_keep_one_windows_room() {
        assert_eq!(width(Some(1000.), 1600.), 1000.);
        assert_eq!(width(Some(1500.), 1600.), 1600. - size::WINDOW_MIN);
        assert_eq!(width(Some(10.), 1600.), size::BESIDE_MIN);
        assert_eq!(width(Some(700.), 600.), size::BESIDE_MIN, "a narrow room");
    }
}
