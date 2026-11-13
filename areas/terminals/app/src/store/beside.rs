//! How wide a view beside the windows is: as the operator last dragged it,
//! else as wide as its content asks, else half the room; never under its
//! least, and never so wide the windows keep less than one window's width.

use crate::theme::size;

/// The share of the room a view takes when nothing says otherwise, and the
/// most its content may ask for.
pub const HALF: f32 = 0.5;

pub fn width(remembered: Option<f32>, wish: Option<f32>, room: f32) -> f32 {
    let most = (room - size::WINDOW_MIN).max(size::BESIDE_MIN);
    let wanted = match remembered.filter(|w| w.is_finite()) {
        Some(w) => w,
        None => wish
            .filter(|w| w.is_finite())
            .map_or(room * HALF, |w| w.min(room * HALF)),
    };
    wanted.clamp(size::BESIDE_MIN, most)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_opens_at_its_contents_width_up_to_half_the_room() {
        assert_eq!(width(None, None, 1600.), 800.);
        assert_eq!(width(None, Some(500.), 1600.), 500.);
        assert_eq!(width(None, Some(1500.), 1600.), 800.);
        assert_eq!(width(None, Some(100.), 1600.), size::BESIDE_MIN);
    }

    #[test]
    fn a_dragged_width_is_kept_while_the_windows_keep_one_windows_room() {
        assert_eq!(width(Some(1000.), Some(300.), 1600.), 1000.);
        assert_eq!(width(Some(1500.), None, 1600.), 1600. - size::WINDOW_MIN);
        assert_eq!(width(Some(10.), None, 1600.), size::BESIDE_MIN);
        assert_eq!(
            width(Some(700.), None, 600.),
            size::BESIDE_MIN,
            "a narrow room"
        );
    }
}
