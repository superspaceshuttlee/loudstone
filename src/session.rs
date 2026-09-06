//! The two bundles of per-session state that were loose fields on `App`.
//!
//! `App` had grown to fifty-three fields, and the cost of that is not the line
//! count -- it is that nothing says which fields belong together. A timer that
//! must be cleared whenever another is set looks exactly like a timer that does
//! not, so the invariant lives only in whoever last edited the file. Grouping
//! them puts the rule and the data in the same place.

use crate::block::BlockId;

/// What the player is doing with their hands this frame, and the cooldowns
/// that pace it.
///
/// These belong together because they are not independent: `target` is only
/// meaningful while `mining`, and letting go of the button must clear it or the
/// next swing silently resumes on a block the player has walked away from.
/// [`Hands::stop`] is the one place that knows the whole rule.
#[derive(Clone, Debug, Default)]
pub struct Hands {
    /// Left button held.
    pub mining: bool,
    /// Right button held.
    pub placing: bool,
    /// Alt held: take the whole block loudly rather than chipping it quietly.
    pub smash: bool,
    /// Seconds until the next bite may land.
    pub chip_timer: f32,
    /// Seconds until the next block may be placed.
    pub place_timer: f32,
    /// Seconds until the next swing may connect.
    pub attack_timer: f32,
    /// The block this swing committed to.
    ///
    /// Mining sticks to the block it started on. Without that, the moment the
    /// ray drills through, mining jumps to whatever is behind and leaves a ring
    /// of the first block standing.
    pub target: Option<(i32, i32, i32)>,
}

impl Hands {
    /// Advance every cooldown by one frame.
    pub fn tick(&mut self, dt: f32) {
        self.chip_timer = (self.chip_timer - dt).max(0.0);
        self.place_timer = (self.place_timer - dt).max(0.0);
        self.attack_timer = (self.attack_timer - dt).max(0.0);
    }

    /// Let go of everything. Clears the committed target as well, which is the
    /// part that is easy to forget and produces a mining swing that resumes on
    /// a block the player has since walked away from.
    pub fn stop(&mut self) {
        self.mining = false;
        self.placing = false;
        self.target = None;
    }
}

/// Counters the gauntlet and the debug overlay read.
///
/// Purely observational: nothing here may be read back by the simulation. Kept
/// as its own value so that stays true by construction -- a counter mixed in
/// among live state is an invitation to branch on it, and then the measurement
/// changes the thing being measured.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub carved: u64,
    pub broken: u64,
    pub placed: u64,
    pub crafted: u64,
    /// Block kinds mined and placed since the harness last looked.
    pub mined_kinds: Vec<u8>,
    pub placed_kinds: Vec<u8>,
    /// Smoothed frames per second, and the accumulator behind it.
    pub fps: f32,
    fps_accum: f32,
    fps_frames: u32,
}

impl Stats {
    /// Fold one frame into the rate. Returns true on the frames where a new
    /// figure was published, so a caller with something to refresh -- the window
    /// title, say -- can do it exactly then rather than keeping its own timer
    /// that drifts out of step with this one.
    pub fn note_frame(&mut self, dt: f32) -> bool {
        self.fps_accum += dt;
        self.fps_frames += 1;
        if self.fps_accum < 0.4 {
            return false;
        }
        self.fps = self.fps_frames as f32 / self.fps_accum;
        self.fps_accum = 0.0;
        self.fps_frames = 0;
        true
    }

    pub fn note_mined(&mut self, id: BlockId) {
        self.broken += 1;
        self.mined_kinds.push(id.0);
    }

    pub fn note_placed(&mut self, id: BlockId) {
        self.placed += 1;
        self.placed_kinds.push(id.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this guards: releasing the button left `target` set, so the next
    /// swing carried on eating a block the player had walked away from.
    #[test]
    fn letting_go_forgets_the_block_being_mined() {
        let mut h = Hands {
            mining: true,
            target: Some((1, 2, 3)),
            ..Default::default()
        };
        h.stop();
        assert!(!h.mining);
        assert_eq!(h.target, None, "a released swing must not stay committed");
    }

    #[test]
    fn cooldowns_run_down_to_zero_and_stop() {
        let mut h = Hands {
            chip_timer: 0.05,
            ..Default::default()
        };
        h.tick(1.0);
        assert_eq!(h.chip_timer, 0.0, "a cooldown must not go negative");
    }

    #[test]
    fn the_frame_rate_is_reported_once_it_has_something_to_report() {
        let mut s = Stats::default();
        for _ in 0..60 {
            s.note_frame(1.0 / 60.0);
        }
        assert!((s.fps - 60.0).abs() < 1.0, "got {}", s.fps);
        assert!(
            !Stats::default().note_frame(1.0 / 60.0),
            "one frame is not enough to report a rate"
        );
    }
}
