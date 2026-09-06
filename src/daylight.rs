//! The day/night cycle: how bright it is, what colour the sky is, and where
//! the sun is in it.
//!
//! Everything here is a pure function of one number, the time of day. That is
//! deliberate: the cycle has no state of its own to fall out of step, and any
//! caller -- the renderer, the mob spawner, the light baker, a test -- can ask
//! what noon looks like without a world existing.

use crate::config::SKY_COLOR;
use glam::Vec3;

/// Seconds for one full day/night cycle.
pub const DAY_LENGTH: f32 = 600.0;
/// Fraction of the cycle spent in full daylight.
pub const DAY_FRACTION: f32 = 0.55;
pub const SKY_NIGHT: [f32; 3] = [0.03, 0.04, 0.08];

/// 1.0 at noon, 0.0 at midnight, with dawn and dusk ramps between.
pub fn daylight_at(time_of_day: f32) -> f32 {
    let t = (time_of_day / DAY_LENGTH).fract();
    let twilight = (1.0 - DAY_FRACTION) * 0.5;
    if t < DAY_FRACTION {
        1.0
    } else if t < DAY_FRACTION + twilight {
        1.0 - (t - DAY_FRACTION) / twilight
    } else if t < DAY_FRACTION + twilight * 2.0 {
        0.0
    } else {
        (t - DAY_FRACTION - twilight * 2.0) / twilight.max(1.0e-4)
    }
    .clamp(0.0, 1.0)
}

/// Where the sun is, and how hard it is shining.
///
/// Returned as a direction *toward* the sun plus a strength, which is what the
/// shader wants. The arc is tilted rather than passing straight overhead: a sun
/// that crosses the exact zenith lights every upward face identically at noon
/// and flattens the whole landscape for the middle third of the day.
///
/// At night the direction is kept pointing at where the sun will rise instead of
/// being zeroed, so that nothing has to divide by a zero-length vector; the
/// strength is what actually turns the light off.
pub fn sun_for(time_of_day: f32) -> [f32; 4] {
    let t = (time_of_day / DAY_LENGTH).fract();
    // Sunrise at the start of the day arc, sunset at its end.
    let angle = std::f32::consts::PI * (t / DAY_FRACTION).clamp(0.0, 1.0);
    let tilt = 0.42;
    let dir = Vec3::new(
        angle.cos(),
        angle.sin() * (1.0 - tilt) + tilt * 0.35,
        -tilt * 1.15,
    )
    .normalize();
    let strength = daylight_at(time_of_day);
    [dir.x, dir.y, dir.z, strength]
}

pub fn sky_for(daylight: f32) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = SKY_NIGHT[i] + (SKY_COLOR[i] - SKY_NIGHT[i]) * daylight;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_runs_a_full_cycle_from_noon_to_midnight_and_back() {
        assert_eq!(daylight_at(0.0), 1.0);
        assert_eq!(
            daylight_at(DAY_LENGTH * 0.5),
            1.0,
            "still day at half a cycle"
        );
        // Deep night sits between the two twilight ramps.
        let night = DAY_LENGTH * (DAY_FRACTION + (1.0 - DAY_FRACTION) * 0.5 + 0.02);
        assert_eq!(daylight_at(night), 0.0);
        // And the sky follows it in both directions.
        assert_eq!(sky_for(1.0), SKY_COLOR);
        assert_eq!(sky_for(0.0), SKY_NIGHT);
    }
}
