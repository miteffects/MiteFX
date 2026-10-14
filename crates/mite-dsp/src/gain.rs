//! Decibel ↔ linear gain conversion.

/// Level treated as silence. `gain_to_db` never returns less than this.
pub const MINUS_INFINITY_DB: f32 = -150.0;

/// Converts decibels to a linear gain factor. Anything at or below [`MINUS_INFINITY_DB`] is 0.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= MINUS_INFINITY_DB {
        0.0
    } else {
        10.0_f32.powf(db * 0.05)
    }
}

/// Converts a linear gain factor (or absolute sample value) to decibels, floored at
/// [`MINUS_INFINITY_DB`].
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    let g = gain.abs();
    if g <= SILENCE_GAIN {
        MINUS_INFINITY_DB
    } else {
        20.0 * g.log10()
    }
}

/// 10^(MINUS_INFINITY_DB / 20) = 10^-7.5
const SILENCE_GAIN: f32 = 3.162_277_7e-8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_is_exact() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert_eq!(gain_to_db(1.0), 0.0);
    }

    #[test]
    fn round_trip_within_a_thousandth_of_a_db() {
        for db in [-120.0, -48.0, -6.0, -0.5, 0.0, 3.0, 24.0] {
            let back = gain_to_db(db_to_gain(db));
            assert!((back - db).abs() < 1e-3, "{db} -> {back}");
        }
    }

    #[test]
    fn silence_floors() {
        assert_eq!(db_to_gain(MINUS_INFINITY_DB), 0.0);
        assert_eq!(gain_to_db(0.0), MINUS_INFINITY_DB);
        assert_eq!(gain_to_db(-0.0), MINUS_INFINITY_DB);
    }
}
