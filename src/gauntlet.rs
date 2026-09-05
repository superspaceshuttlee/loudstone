//! A scripted playtest. `--gauntlet` runs the real game loop with a robot at the
//! controls: it walks, jumps, chips, smashes, places, gets ambushed, watches
//! night fall, and saves and reloads. Invariants are checked every frame, and
//! each step states what it expects to have happened by the time it ends.
//!
//! This exists because unit tests prove systems in isolation but say nothing
//! about the game. Falling through the floor, terrain that never loads, a frame
//! rate that collapses after two minutes, mobs that ignore you -- none of those
//! show up in a test of a pure function. They show up here.

use crate::camera::MoveInput;
use glam::Vec3;

/// What the harness can see about the running game, sampled once per frame.
#[derive(Clone, Copy)]
pub struct Probe {
    pub pos: Vec3,
    pub on_ground: bool,
    pub health: f32,
    pub fps: f32,
    pub chunks: usize,
    pub mobs: usize,
    /// The player's own box overlaps solid terrain right now.
    pub inside_solid: bool,
    pub daylight: f32,
    pub nearest_hostile: Option<f32>,
    // Monotonic counters the game increments as things happen.
    pub carved: u64,
    pub broken: u64,
    pub placed: u64,
}

/// What a step claims will be true by the time it finishes.
#[derive(Clone, Copy, PartialEq)]
pub enum Expect {
    Nothing,
    /// Horizontal distance covered, in blocks.
    MovedAtLeast(f32),
    /// Standing on something by the end.
    OnGround,
    CarvedSubVoxels,
    BrokeABlock,
    PlacedABlock,
    /// Sky went dark.
    NightFell,
    /// Something hostile got closer than it was.
    HostilesClosedIn,
}

pub struct Step {
    pub name: &'static str,
    pub seconds: f32,
    pub input: MoveInput,
    pub mine: bool,
    pub smash: bool,
    pub place: bool,
    /// Force this many hostile mobs into the world when the step begins.
    pub spawn_hostiles: usize,
    /// Multiplier on the day/night clock, for reaching nightfall quickly.
    pub time_scale: f32,
    /// Radians per second of yaw. A robot that holds W into a cliff face learns
    /// nothing; turning slowly as it walks keeps it exploring real ground.
    pub yaw_rate: f32,
    /// Force the camera pitch, so mining steps actually aim at the ground.
    pub look_pitch: Option<f32>,
    /// Jump the day clock to this fraction of a full cycle.
    pub set_time_frac: Option<f32>,
    /// Capture a frame at the end of the step, under this name.
    pub shot: Option<&'static str>,
    pub expect: Expect,
}

impl Step {
    fn new(name: &'static str, seconds: f32) -> Self {
        Self {
            name,
            seconds,
            input: MoveInput::default(),
            mine: false,
            smash: false,
            place: false,
            spawn_hostiles: 0,
            time_scale: 1.0,
            yaw_rate: 0.0,
            look_pitch: None,
            set_time_frac: None,
            shot: None,
            expect: Expect::Nothing,
        }
    }
    fn walking(mut self) -> Self {
        self.input.fwd = true;
        self
    }
    fn sprinting(mut self) -> Self {
        self.input.fwd = true;
        self.input.fast = true;
        self
    }
    fn jumping(mut self) -> Self {
        self.input.up = true;
        self
    }
    fn mining(mut self) -> Self {
        self.mine = true;
        self
    }
    fn smashing(mut self) -> Self {
        self.mine = true;
        self.smash = true;
        self
    }
    fn placing(mut self) -> Self {
        self.place = true;
        self
    }
    fn hostiles(mut self, n: usize) -> Self {
        self.spawn_hostiles = n;
        self
    }
    fn fast_clock(mut self, k: f32) -> Self {
        self.time_scale = k;
        self
    }
    fn turning(mut self, rate: f32) -> Self {
        self.yaw_rate = rate;
        self
    }
    fn looking_down(mut self) -> Self {
        self.look_pitch = Some(-0.9);
        self
    }
    /// A shallower angle, aimed at the ground a stride ahead. Straight down is
    /// wrong for smashing (four seconds of chipping digs the floor out from
    /// under you and you fall) and impossible for placing (the block above the
    /// one you are looking at is where you are standing).
    fn looking_ahead(mut self) -> Self {
        self.look_pitch = Some(-0.35);
        self
    }
    /// Steep enough to always meet the floor a stride away: shallower angles can
    /// find nothing at all inside a cavern, and straight down targets the block
    /// the player is standing in.
    fn looking_at_feet(mut self) -> Self {
        self.look_pitch = Some(-0.7);
        self
    }
    fn at_time(mut self, frac: f32) -> Self {
        self.set_time_frac = Some(frac);
        self
    }
    fn shot(mut self, name: &'static str) -> Self {
        self.shot = Some(name);
        self
    }
    fn expect(mut self, e: Expect) -> Self {
        self.expect = e;
        self
    }
}

/// Something that went wrong. Collected rather than panicked on, so one bad
/// step does not hide the rest of the run.
pub struct Finding {
    pub step: &'static str,
    pub what: String,
}

pub struct Gauntlet {
    steps: Vec<Step>,
    idx: usize,
    elapsed: f32,
    start: Probe,
    /// Consecutive seconds spent embedded in terrain.
    stuck_for: f32,
    /// Closest a hostile has been during this step.
    closest_seen: f32,
    warmup: f32,
    pub findings: Vec<Finding>,
    pub done: bool,
}

/// What the game should do this frame on the harness's behalf.
pub struct Frame {
    pub input: MoveInput,
    pub mine: bool,
    pub smash: bool,
    pub place: bool,
    pub time_scale: f32,
    pub yaw_rate: f32,
    pub look_pitch: Option<f32>,
    /// Only set on the first frame of a step that asks for it.
    pub set_time_frac: Option<f32>,
    pub spawn_hostiles: usize,
    pub shot: Option<&'static str>,
}

impl Gauntlet {
    pub fn new() -> Self {
        Self {
            steps: script(),
            idx: 0,
            elapsed: 0.0,
            start: blank_probe(),
            stuck_for: 0.0,
            closest_seen: f32::INFINITY,
            warmup: 2.0,
            findings: Vec::new(),
            done: false,
        }
    }

    fn fail(&mut self, what: String) {
        let step = self.steps.get(self.idx).map(|s| s.name).unwrap_or("(end)");
        println!("  FAIL  {step}: {what}");
        self.findings.push(Finding { step, what });
    }

    /// Invariants that must hold at every moment of the run, in every step.
    fn check_always(&mut self, dt: f32, p: &Probe) {
        if !p.pos.is_finite() {
            self.fail(format!("player position is not finite: {:?}", p.pos));
            self.done = true;
            return;
        }
        if p.pos.y < -8.0 || p.pos.y > 320.0 {
            self.fail(format!("player left the world at y={:.1}", p.pos.y));
            self.done = true;
            return;
        }
        if !(0.0..=20.0).contains(&p.health) {
            self.fail(format!("health out of range: {:.1}", p.health));
        }
        // Brief overlap is normal when standing flush on a surface; being
        // embedded for a whole second is not.
        if p.inside_solid {
            self.stuck_for += dt;
            if self.stuck_for > 1.0 {
                self.fail(format!(
                    "player embedded in terrain for {:.1}s at {:?}",
                    self.stuck_for, p.pos
                ));
                self.stuck_for = -1.0e9; // report once
            }
        } else {
            self.stuck_for = 0.0;
        }

        self.warmup -= dt;
        if self.warmup <= 0.0 && p.fps > 0.0 && p.fps < 20.0 {
            self.fail(format!("frame rate collapsed to {:.0} fps", p.fps));
            self.warmup = 5.0; // do not spam
        }
        if p.chunks > 12_000 {
            self.fail(format!("chunk count runaway: {}", p.chunks));
            self.warmup = 5.0;
        }
        if let Some(d) = p.nearest_hostile {
            self.closest_seen = self.closest_seen.min(d);
        }
    }

    /// Check what the finished step promised.
    fn check_step(&mut self, p: &Probe) {
        let Some(step) = self.steps.get(self.idx) else {
            return;
        };
        let (expect, name) = (step.expect, step.name);
        let s = self.start;
        let moved = (Vec3::new(p.pos.x, 0.0, p.pos.z) - Vec3::new(s.pos.x, 0.0, s.pos.z)).length();
        match expect {
            Expect::Nothing => {}
            Expect::MovedAtLeast(d) => {
                if moved < d {
                    self.fail(format!("expected to cover {d:.1} blocks, managed {moved:.2}"));
                }
            }
            Expect::OnGround => {
                if !p.on_ground {
                    self.fail("expected to be standing on something, still airborne".into());
                }
            }
            Expect::CarvedSubVoxels => {
                if p.carved <= s.carved {
                    self.fail("mining chipped nothing at all".into());
                }
            }
            Expect::BrokeABlock => {
                if p.broken <= s.broken {
                    self.fail("smashing removed no block".into());
                }
            }
            Expect::PlacedABlock => {
                if p.placed <= s.placed {
                    self.fail("placing put down no block".into());
                }
            }
            Expect::NightFell => {
                if p.daylight > 0.1 {
                    self.fail(format!("still daylight {:.2} after waiting", p.daylight));
                }
            }
            Expect::HostilesClosedIn => match (s.nearest_hostile, self.closest_seen) {
                (Some(before), closest) if closest.is_finite() => {
                    if closest > before - 1.0 {
                        self.fail(format!(
                            "hostiles never closed in: {before:.1} blocks at the start, \
                             closest seen {closest:.1}"
                        ));
                    }
                }
                _ => self.fail("no hostile was ever present to close in".into()),
            },
        }
        let _ = name;
    }

    /// Advance one frame. Returns what the game should do, or `None` when done.
    pub fn tick(&mut self, dt: f32, p: &Probe) -> Option<Frame> {
        if self.done {
            return None;
        }
        self.check_always(dt, p);
        if self.done {
            return None;
        }

        // Entering a step.
        if self.elapsed == 0.0 {
            let Some(step) = self.steps.get(self.idx) else {
                self.done = true;
                return None;
            };
            println!("[gauntlet] {} ({:.0}s)", step.name, step.seconds);
            self.start = *p;
            self.closest_seen = f32::INFINITY;
        }

        let Some(step) = self.steps.get(self.idx) else {
            self.done = true;
            return None;
        };
        let first_frame = self.elapsed == 0.0;
        let seconds = step.seconds;
        let frame = Frame {
            input: step.input,
            mine: step.mine,
            smash: step.smash,
            place: step.place,
            time_scale: step.time_scale,
            yaw_rate: step.yaw_rate,
            look_pitch: step.look_pitch,
            set_time_frac: if first_frame { step.set_time_frac } else { None },
            spawn_hostiles: if first_frame { step.spawn_hostiles } else { 0 },
            shot: None,
        };

        self.elapsed += dt;
        if self.elapsed >= seconds {
            let shot = self.steps[self.idx].shot;
            self.check_step(p);
            self.elapsed = 0.0;
            self.idx += 1;
            if self.idx >= self.steps.len() {
                self.done = true;
            }
            return Some(Frame { shot, ..frame });
        }
        Some(frame)
    }

    pub fn total_steps(&self) -> usize {
        self.steps.len()
    }
}

impl Default for Gauntlet {
    fn default() -> Self {
        Self::new()
    }
}

fn blank_probe() -> Probe {
    Probe {
        pos: Vec3::ZERO,
        on_ground: false,
        health: 20.0,
        fps: 0.0,
        chunks: 0,
        mobs: 0,
        inside_solid: false,
        daylight: 1.0,
        nearest_hostile: None,
        carved: 0,
        broken: 0,
        placed: 0,
    }
}

/// The run itself. Ordered so that a failure early is also the most diagnostic:
/// if the player cannot stand up, nothing after it means anything.
fn script() -> Vec<Step> {
    vec![
        Step::new("settle on spawn", 2.0)
            .expect(Expect::OnGround)
            .shot("01_spawn"),
        // Turning while walking matters: spawn can face a cliff, and a robot
        // that only holds W would report a movement failure that a player would
        // never experience.
        Step::new("walk forward", 5.0)
            .walking()
            .turning(0.35)
            .expect(Expect::MovedAtLeast(6.0)),
        Step::new("sprint across terrain", 8.0)
            .sprinting()
            .turning(0.25)
            .expect(Expect::MovedAtLeast(18.0))
            .shot("02_sprint"),
        Step::new("jump repeatedly", 3.0).jumping().walking(),
        Step::new("land after jumping", 2.0).expect(Expect::OnGround),
        Step::new("chip quietly at the ground", 4.0)
            .mining()
            .looking_down()
            .expect(Expect::CarvedSubVoxels)
            .shot("03_chipped"),
        Step::new("smash a whole block", 3.0)
            .smashing()
            .looking_ahead()
            .expect(Expect::BrokeABlock),
        Step::new("place a block", 3.0)
            .placing()
            .looking_at_feet()
            .expect(Expect::PlacedABlock),
        Step::new("stand still and stay quiet", 4.0)
            .looking_down()
            .shot("04_quiet"),
        Step::new("ambush: hostiles spawn nearby", 1.0).hostiles(4),
        Step::new("smash loudly while they listen", 8.0)
            .smashing()
            .looking_ahead()
            .expect(Expect::HostilesClosedIn)
            .shot("05_mobs_converge"),
        Step::new("survive the fight", 6.0).mining(),
        // Setting the clock beats fast-forwarding it: multiplying dt can
        // overshoot a whole cycle and land back in daylight.
        Step::new("nightfall", 3.0)
            .at_time(0.85)
            .expect(Expect::NightFell)
            .shot("06_night"),
        Step::new("walk at night", 5.0).walking().turning(0.3),
        Step::new("final look around", 2.0).shot("07_end"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_at(y: f32) -> Probe {
        Probe {
            pos: Vec3::new(0.0, y, 0.0),
            on_ground: true,
            ..blank_probe()
        }
    }

    #[test]
    fn the_script_is_not_empty_and_every_step_has_a_duration() {
        let g = Gauntlet::new();
        assert!(g.total_steps() >= 10);
        for s in &g.steps {
            assert!(s.seconds > 0.0, "step {} has no duration", s.name);
        }
    }

    #[test]
    fn leaving_the_world_ends_the_run_immediately() {
        let mut g = Gauntlet::new();
        let p = probe_at(-500.0);
        g.tick(0.016, &p);
        assert!(g.done, "falling out of the world must stop the run");
        assert!(!g.findings.is_empty());
    }

    #[test]
    fn a_non_finite_position_is_caught() {
        let mut g = Gauntlet::new();
        let mut p = probe_at(60.0);
        p.pos.x = f32::NAN;
        g.tick(0.016, &p);
        assert!(g.done);
        assert!(g.findings[0].what.contains("not finite"));
    }

    #[test]
    fn being_briefly_flush_with_the_ground_is_not_a_failure() {
        let mut g = Gauntlet::new();
        let mut p = probe_at(60.0);
        p.inside_solid = true;
        // Half a second of overlap is what standing on a surface looks like.
        for _ in 0..30 {
            g.tick(1.0 / 60.0, &p);
        }
        assert!(g.findings.is_empty(), "{:?}", g.findings[0].what);
    }

    #[test]
    fn being_embedded_for_a_full_second_is_a_failure() {
        let mut g = Gauntlet::new();
        let mut p = probe_at(60.0);
        p.inside_solid = true;
        for _ in 0..120 {
            g.tick(1.0 / 60.0, &p);
        }
        assert!(g
            .findings
            .iter()
            .any(|f| f.what.contains("embedded in terrain")));
    }

    #[test]
    fn a_step_that_does_not_move_far_enough_is_reported() {
        let mut g = Gauntlet::new();
        let p = probe_at(60.0);
        // Step 0 (settle, 2s) passes; step 1 (walk, 4s) wants 8 blocks of
        // travel, so the run has to get past t=6s for it to be judged at all.
        for _ in 0..(60 * 8) {
            g.tick(1.0 / 60.0, &p);
        }
        assert!(g
            .findings
            .iter()
            .any(|f| f.what.contains("expected to cover")));
    }

    #[test]
    fn the_run_finishes_and_reports_nothing_when_everything_behaves() {
        let mut g = Gauntlet::new();
        let mut t = 0.0f32;
        let mut p = probe_at(60.0);
        p.fps = 60.0;
        p.nearest_hostile = Some(20.0);
        let mut guard = 0;
        while !g.done && guard < 100_000 {
            // Satisfy every expectation: keep moving, keep mining, get dark,
            // and let the hostiles close the distance.
            t += 1.0 / 60.0;
            p.pos.x = t * 8.0;
            p.carved += 4;
            p.broken += 1;
            p.placed += 1;
            p.daylight = 0.0;
            p.nearest_hostile = Some((20.0 - t * 0.4).max(1.0));
            g.tick(1.0 / 60.0, &p);
            guard += 1;
        }
        assert!(g.done);
        assert!(
            g.findings.is_empty(),
            "clean run still reported: {}",
            g.findings[0].what
        );
    }
}

/// A phase breakdown of chunk loading, used to find where the streaming budget
/// actually goes. Not a correctness test -- it prints and never fails.
#[cfg(test)]
mod bench {
    use crate::chunk::{Chunk, ChunkPos};
    use crate::config::CHUNK_SIZE_I;
    use crate::mesh::{mesh_chunk, Neighborhood, NEIGHBOR_COUNT};
    use crate::worldgen::TerrainGen;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn where_does_chunk_loading_time_go() {
        let g = TerrainGen::new(1337);
        let mut positions = Vec::new();
        for cz in -5..5 {
            for cx in -5..5 {
                let b = g.column_bounds(cx, cz);
                let lo = (b.lo.div_euclid(CHUNK_SIZE_I) - 1).max(0);
                let hi = b.hi.div_euclid(CHUNK_SIZE_I).min(15);
                for cy in lo..=hi {
                    positions.push(ChunkPos::new(cx, cy, cz));
                }
            }
        }

        let t0 = Instant::now();
        let mut chunks: Vec<Chunk> = positions.iter().map(|p| g.generate(*p)).collect();
        let generated = t0.elapsed();

        let t1 = Instant::now();
        for c in chunks.iter_mut() {
            crate::light::seed_chunk(c, &g);
        }
        let lit = t1.elapsed();

        let arcs: Vec<Arc<Chunk>> = chunks.into_iter().map(Arc::new).collect();
        let t2 = Instant::now();
        let mut tris = 0usize;
        for (i, p) in positions.iter().enumerate() {
            let mut n: [Option<Arc<Chunk>>; NEIGHBOR_COUNT] = std::array::from_fn(|_| None);
            n[crate::mesh::neighbor_index(0, 0, 0)] = Some(arcs[i].clone());
            let nb = Neighborhood::build(*p, &n, &g, 0);
            tris += mesh_chunk(&nb).1.len();
        }
        let meshed = t2.elapsed();

        let n = positions.len() as f64;
        println!(
            "PHASES over {} chunks:\n  generate {:7.3}s  ({:6.1} us/chunk)\n  \
             light    {:7.3}s  ({:6.1} us/chunk)\n  mesh     {:7.3}s  ({:6.1} us/chunk)\n  \
             indices  {tris}",
            positions.len(),
            generated.as_secs_f64(),
            generated.as_secs_f64() / n * 1e6,
            lit.as_secs_f64(),
            lit.as_secs_f64() / n * 1e6,
            meshed.as_secs_f64(),
            meshed.as_secs_f64() / n * 1e6,
        );
    }
}
