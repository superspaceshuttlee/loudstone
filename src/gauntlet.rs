//! A randomised, property-based playtest harness.
//!
//! The first version of this was a fixed 15-step script, and a fixed script is
//! close to worthless: it finds exactly the bugs its author already imagined,
//! passes trivially once tuned, and visits one hillside on one seed. Everything
//! else is a blind spot.
//!
//! This is built the other way round. A weighted random policy drives the
//! player through long sessions, teleporting between distant columns to sample
//! many biomes; **oracles** run every frame and assert properties that must hold
//! no matter what the robot did; and a **coverage tracker** records what was
//! actually exercised, so a session that wandered an empty field and touched
//! nothing is reported as a failure rather than a pass.
//!
//! Every run prints its seed, so any failure replays with `--gauntlet --seed N`.
//!
//! The oracles are the real content. They are properties, not examples:
//!
//! * the player is never non-finite, never outside the world, never sealed
//!   inside terrain for more than a moment;
//! * items never appear from nowhere -- the inventory total may only rise in a
//!   frame that mined or crafted something;
//! * stacks never exceed their limit and never sit at zero;
//! * streaming always finishes: standing still must eventually reach an idle
//!   world, or chunk loading has hung;
//! * light stays inside 0..15, mobs stay finite and inside their cap, health
//!   stays in range, and the frame rate never collapses;
//! * a save round-trip reproduces the world exactly.

use crate::camera::MoveInput;
use glam::Vec3;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

/// Simulated seconds per session unless `--secs` says otherwise.
pub const DEFAULT_SECONDS: f32 = 150.0;
/// A burst of one behaviour lasts between these, so the robot commits to an
/// action long enough for it to matter instead of twitching every frame.
const BURST_MIN: f32 = 0.4;
const BURST_MAX: f32 = 3.0;
/// Seconds of standing still before an unfinished world counts as a hang.
const STREAM_HANG_SECONDS: f32 = 20.0;
/// Below this the frame rate counts as collapsed.
const FPS_FLOOR: f32 = 15.0;
const MAX_CHUNKS: usize = 12_000;
const MAX_MOBS: usize = 300;

/// What a session must have exercised to count as a real playtest.
const MIN_BLOCK_KINDS_MINED: usize = 3;
const MIN_BIOMES_VISITED: usize = 2;

// ---------------------------------------------------------------------------

/// Everything the harness can see about the running game, sampled each frame.
#[derive(Clone, Default)]
pub struct Probe {
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub health: f32,
    pub fps: f32,
    pub chunks: usize,
    pub mobs: usize,
    pub mob_pos_bad: bool,
    pub inside_solid: bool,
    pub daylight: f32,
    pub nearest_hostile: Option<f32>,
    pub biome: u8,
    pub light_here: u8,
    /// Total items held, for the conservation oracle.
    pub inventory_total: u32,
    /// A stack over its limit, or present with a count of zero.
    pub inventory_bad: bool,
    /// Streaming has nothing left to do.
    pub world_idle: bool,
    /// 0 playing, 1 inventory, 2 table, 3 furnace.
    pub panel: u8,
    // Monotonic counters.
    pub carved: u64,
    pub broken: u64,
    pub placed: u64,
    pub crafted: u64,
    /// Block ids mined and placed since the last probe.
    pub mined_kinds: Vec<u8>,
    pub placed_kinds: Vec<u8>,
    /// Set by the game when a requested save round-trip finished.
    pub save_roundtrip: Option<bool>,
}

/// One behaviour the robot commits to for a burst.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Act {
    Idle,
    Walk,
    Sprint,
    Strafe,
    Back,
    Jump,
    Mine,
    Smash,
    Place,
    Attack,
    Hotbar,
    OpenInventory,
    CloseUi,
    Teleport,
    Night,
    Day,
    SpawnMobs,
    Craft,
    SaveCheck,
}

/// The weight table. Movement and mining dominate because that is what playing
/// the game mostly is. The rare directed entries -- teleport, night, mobs, save
/// -- are what stop pure randomness from never reaching a second biome, never
/// seeing darkness, and never meeting anything that fights back.
const WEIGHTS: &[(Act, u32)] = &[
    (Act::Walk, 22),
    (Act::Sprint, 12),
    (Act::Strafe, 6),
    (Act::Back, 4),
    (Act::Jump, 6),
    (Act::Mine, 20),
    (Act::Smash, 10),
    (Act::Place, 8),
    (Act::Attack, 5),
    (Act::Idle, 4),
    (Act::Hotbar, 3),
    (Act::OpenInventory, 3),
    (Act::CloseUi, 3),
    (Act::Craft, 2),
    (Act::Teleport, 5),
    (Act::Night, 2),
    (Act::Day, 2),
    (Act::SpawnMobs, 4),
    (Act::SaveCheck, 2),
];

/// What the game should do this frame on the harness's behalf.
#[derive(Default)]
pub struct Frame {
    pub input: MoveInput,
    pub mine: bool,
    pub smash: bool,
    pub place: bool,
    pub attack: bool,
    pub yaw_rate: f32,
    pub look_pitch: Option<f32>,
    pub set_time_frac: Option<f32>,
    pub spawn_hostiles: usize,
    /// Jump to a random distant column, to sample other biomes and re-stream.
    pub teleport: Option<(i32, i32)>,
    /// Some(true) opens the inventory, Some(false) closes whatever is open.
    pub set_inventory: Option<bool>,
    pub hotbar: Option<usize>,
    pub craft: bool,
    pub save_check: bool,
    pub shot: Option<&'static str>,
}

pub struct Finding {
    pub at: f32,
    pub during: Act,
    pub what: String,
}

#[derive(Default)]
pub struct Coverage {
    pub blocks_mined: HashSet<u8>,
    pub blocks_placed: HashSet<u8>,
    pub biomes: HashSet<u8>,
    pub panels: HashSet<u8>,
    pub acts: HashSet<Act>,
    pub min_y: f32,
    pub max_y: f32,
    pub distance: f32,
    pub saw_night: bool,
    pub saw_day: bool,
    pub saw_dark_place: bool,
    pub met_a_hostile: bool,
    pub took_damage: bool,
    pub crafted: u64,
    pub save_checks: u32,
}

pub struct Harness {
    rng: StdRng,
    pub seed: u64,
    pub total_seconds: f32,
    elapsed: f32,
    act: Act,
    burst_left: f32,
    burst_age: f32,
    yaw_rate: f32,
    pitch: f32,
    // Oracle state.
    stuck_for: f32,
    still_for: f32,
    warmup: f32,
    last: Probe,
    started: bool,
    next_shot: usize,
    pub findings: Vec<Finding>,
    pub coverage: Coverage,
    pub done: bool,
}

const SHOTS: [&str; 6] = ["a_early", "b_quarter", "c_half", "d_mid", "e_late", "f_end"];

impl Harness {
    pub fn new(seed: u64, seconds: f32) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            seed,
            total_seconds: seconds,
            elapsed: 0.0,
            act: Act::Walk,
            burst_left: 0.0,
            burst_age: 0.0,
            yaw_rate: 0.0,
            pitch: -0.2,
            stuck_for: 0.0,
            still_for: 0.0,
            warmup: 3.0,
            last: Probe::default(),
            started: false,
            next_shot: 0,
            findings: Vec::new(),
            coverage: Coverage {
                min_y: f32::INFINITY,
                max_y: f32::NEG_INFINITY,
                ..Default::default()
            },
            done: false,
        }
    }

    fn fail(&mut self, what: String) {
        println!("  FAIL [{:6.1}s during {:?}] {what}", self.elapsed, self.act);
        self.findings.push(Finding {
            at: self.elapsed,
            during: self.act,
            what,
        });
    }

    // -- oracles ------------------------------------------------------------

    fn check(&mut self, dt: f32, p: &Probe) {
        // Fatal: stop the run, because everything after it is noise.
        if !p.pos.is_finite() || !p.vel.is_finite() {
            self.fail(format!(
                "player state not finite: pos {:?} vel {:?}",
                p.pos, p.vel
            ));
            self.done = true;
            return;
        }
        if p.pos.y < -8.0 || p.pos.y > 320.0 {
            self.fail(format!("player left the world at y={:.1}", p.pos.y));
            self.done = true;
            return;
        }

        if !(0.0..=20.0).contains(&p.health) {
            self.fail(format!("health out of range: {:.2}", p.health));
        }
        if p.inventory_bad {
            self.fail("a stack is over its limit or present with a count of zero".into());
        }
        if p.mob_pos_bad {
            self.fail("a mob has a non-finite position".into());
        }
        if p.mobs > MAX_MOBS {
            self.fail(format!("mob population ran away: {}", p.mobs));
        }
        if p.chunks > MAX_CHUNKS {
            self.fail(format!("chunk count ran away: {}", p.chunks));
        }
        if p.light_here > 15 {
            self.fail(format!("light level {} is outside 0..15", p.light_here));
        }

        // Items may only appear in a frame that mined or crafted something.
        if self.started && p.inventory_total > self.last.inventory_total {
            let mined = p.broken > self.last.broken || p.carved > self.last.carved;
            let crafted = p.crafted > self.last.crafted;
            if !mined && !crafted {
                self.fail(format!(
                    "inventory grew from {} to {} with nothing mined or crafted",
                    self.last.inventory_total, p.inventory_total
                ));
            }
        }

        // Sealed inside terrain. A moment of overlap is just standing on a
        // surface; a full second means the player is buried.
        if p.inside_solid {
            self.stuck_for += dt;
            if self.stuck_for > 1.0 {
                self.fail(format!("player embedded in terrain at {:?}", p.pos));
                self.stuck_for = -1.0e9;
            }
        } else {
            self.stuck_for = 0.0;
        }

        // Streaming liveness: standing still must eventually settle.
        if p.vel.length() < 0.05 {
            self.still_for += dt;
        } else {
            self.still_for = 0.0;
        }
        if self.still_for > STREAM_HANG_SECONDS && !p.world_idle {
            self.fail(format!(
                "world still streaming after {STREAM_HANG_SECONDS:.0}s of standing still"
            ));
            self.still_for = 0.0;
        }

        self.warmup -= dt;
        if self.warmup <= 0.0 && p.fps > 0.0 && p.fps < FPS_FLOOR {
            self.fail(format!("frame rate collapsed to {:.0} fps", p.fps));
            self.warmup = 8.0;
        }

        if p.save_roundtrip == Some(false) {
            self.fail("a save round-trip did not reproduce the world".into());
        }
    }

    fn record(&mut self, p: &Probe) {
        let c = &mut self.coverage;
        c.min_y = c.min_y.min(p.pos.y);
        c.max_y = c.max_y.max(p.pos.y);
        if self.started {
            // Clamped so a teleport does not count as a marathon.
            c.distance += (p.pos - self.last.pos).length().min(50.0);
            if p.health < self.last.health {
                c.took_damage = true;
            }
        }
        c.biomes.insert(p.biome);
        c.panels.insert(p.panel);
        for b in &p.mined_kinds {
            c.blocks_mined.insert(*b);
        }
        for b in &p.placed_kinds {
            c.blocks_placed.insert(*b);
        }
        if p.daylight < 0.2 {
            c.saw_night = true;
        }
        if p.daylight > 0.8 {
            c.saw_day = true;
        }
        if p.light_here < 4 {
            c.saw_dark_place = true;
        }
        if p.nearest_hostile.map(|d| d < 12.0).unwrap_or(false) {
            c.met_a_hostile = true;
        }
        c.crafted = p.crafted;
        if p.save_roundtrip.is_some() {
            c.save_checks += 1;
        }
    }

    // -- policy -------------------------------------------------------------

    fn pick_act(&mut self) -> Act {
        let total: u32 = WEIGHTS.iter().map(|(_, w)| w).sum();
        let mut r = self.rng.random_range(0..total);
        for (a, w) in WEIGHTS {
            if r < *w {
                return *a;
            }
            r -= *w;
        }
        Act::Walk
    }

    /// Advance one frame. `None` once the session is over.
    pub fn tick(&mut self, dt: f32, p: &Probe) -> Option<Frame> {
        if self.done {
            return None;
        }
        self.check(dt, p);
        self.record(p);
        if self.done {
            return None;
        }
        self.last = p.clone();
        self.started = true;

        self.elapsed += dt;
        if self.elapsed >= self.total_seconds {
            self.done = true;
            return None;
        }

        // Screenshots spread evenly across the session.
        let mut shot = None;
        let stride = self.total_seconds / SHOTS.len() as f32;
        if self.next_shot < SHOTS.len() && self.elapsed >= stride * (self.next_shot as f32 + 0.5) {
            shot = Some(SHOTS[self.next_shot]);
            self.next_shot += 1;
        }

        self.burst_left -= dt;
        self.burst_age += dt;
        let mut first = false;
        if self.burst_left <= 0.0 {
            self.act = self.pick_act();
            self.burst_left = self.rng.random_range(BURST_MIN..BURST_MAX);
            self.yaw_rate = self.rng.random_range(-0.9f32..0.9);
            self.pitch = self.rng.random_range(-1.1f32..0.3);
            self.burst_age = 0.0;
            self.coverage.acts.insert(self.act);
            first = true;
        }

        let mut f = Frame {
            yaw_rate: self.yaw_rate,
            look_pitch: Some(self.pitch),
            shot,
            ..Default::default()
        };

        match self.act {
            Act::Idle => {}
            Act::Walk => f.input.fwd = true,
            Act::Sprint => {
                f.input.fwd = true;
                f.input.fast = true;
            }
            Act::Back => f.input.back = true,
            Act::Strafe => {
                if self.rng.random::<bool>() {
                    f.input.left = true;
                } else {
                    f.input.right = true;
                }
                f.input.fwd = true;
            }
            Act::Jump => {
                f.input.up = true;
                f.input.fwd = true;
            }
            Act::Mine => {
                f.mine = true;
                f.look_pitch = Some(self.pitch.min(-0.3));
            }
            Act::Smash => {
                f.mine = true;
                f.smash = true;
                f.look_pitch = Some(self.pitch.min(-0.3));
            }
            Act::Place => {
                f.place = true;
                f.look_pitch = Some(self.pitch.clamp(-1.1, -0.4));
            }
            Act::Attack => {
                f.attack = true;
                f.look_pitch = Some(-0.1);
            }
            Act::Hotbar => f.hotbar = Some(self.rng.random_range(0..9)),
            Act::OpenInventory => f.set_inventory = Some(true),
            Act::CloseUi => f.set_inventory = Some(false),
            Act::Craft => f.craft = true,
            Act::Teleport => {
                if first {
                    // Far enough that the whole radius must re-stream, and far
                    // enough to land in a different biome.
                    let x = self.rng.random_range(-4000i32..4000);
                    let z = self.rng.random_range(-4000i32..4000);
                    f.teleport = Some((x, z));
                }
            }
            Act::Night => f.set_time_frac = Some(0.85),
            Act::Day => f.set_time_frac = Some(0.2),
            Act::SpawnMobs => {
                if first {
                    f.spawn_hostiles = self.rng.random_range(1..5);
                }
            }
            Act::SaveCheck => f.save_check = first,
        }
        Some(f)
    }

    /// Coverage is itself an oracle: a session that exercised almost nothing
    /// proves almost nothing, and calling it a pass would be a lie.
    pub fn finish(&mut self) {
        let c = &self.coverage;
        let mut thin: Vec<String> = Vec::new();
        if c.blocks_mined.len() < MIN_BLOCK_KINDS_MINED {
            thin.push(format!(
                "only {} kind(s) of block were mined",
                c.blocks_mined.len()
            ));
        }
        if c.biomes.len() < MIN_BIOMES_VISITED {
            thin.push(format!("only {} biome(s) visited", c.biomes.len()));
        }
        if !c.saw_night || !c.saw_day {
            thin.push("did not see both day and night".into());
        }
        if !c.met_a_hostile {
            thin.push("never came near a hostile mob".into());
        }
        if c.distance < 50.0 {
            thin.push(format!("barely moved ({:.0} blocks)", c.distance));
        }
        if c.save_checks == 0 {
            thin.push("never checked a save round-trip".into());
        }
        for t in thin {
            self.fail(format!("coverage too thin: {t}"));
        }
    }

    pub fn report(&self) {
        let c = &self.coverage;
        println!("\n[gauntlet] ---- seed {} ----", self.seed);
        println!(
            "[gauntlet] {:.0}s simulated, {:.0} blocks travelled, y {:.0}..{:.0}",
            self.elapsed, c.distance, c.min_y, c.max_y
        );
        println!(
            "[gauntlet] coverage: {} block kinds mined, {} placed, {} biomes, {} panels, \
             {}/{} actions, {} crafts, {} save checks",
            c.blocks_mined.len(),
            c.blocks_placed.len(),
            c.biomes.len(),
            c.panels.len(),
            c.acts.len(),
            WEIGHTS.len(),
            c.crafted,
            c.save_checks
        );
        println!(
            "[gauntlet] day {} | night {} | darkness {} | hostiles near {} | took damage {}",
            c.saw_day, c.saw_night, c.saw_dark_place, c.met_a_hostile, c.took_damage
        );
        if self.findings.is_empty() {
            println!("[gauntlet] no findings");
        } else {
            println!("[gauntlet] {} findings:", self.findings.len());
            for f in &self.findings {
                println!("[gauntlet]   [{:6.1}s {:?}] {}", f.at, f.during, f.what);
            }
            println!("[gauntlet] reproduce with: --gauntlet --seed {}", self.seed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_probe() -> Probe {
        Probe {
            pos: Vec3::new(0.0, 64.0, 0.0),
            health: 20.0,
            fps: 60.0,
            world_idle: true,
            ..Default::default()
        }
    }

    fn run(h: &mut Harness, p: &Probe, seconds: f32) {
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        while t < seconds && !h.done {
            h.tick(dt, p);
            t += dt;
        }
    }

    #[test]
    fn a_non_finite_position_stops_the_run_at_once() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        p.pos.x = f32::NAN;
        h.tick(0.016, &p);
        assert!(h.done);
        assert!(h.findings[0].what.contains("not finite"));
    }

    #[test]
    fn falling_out_of_the_world_stops_the_run() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        p.pos.y = -500.0;
        h.tick(0.016, &p);
        assert!(h.done);
    }

    #[test]
    fn items_appearing_without_mining_or_crafting_is_caught() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        h.tick(0.016, &p);
        p.inventory_total = 64;
        h.tick(0.016, &p);
        assert!(
            h.findings
                .iter()
                .any(|f| f.what.contains("with nothing mined")),
            "item duplication must be reported"
        );
    }

    #[test]
    fn items_appearing_after_a_break_is_fine() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        h.tick(0.016, &p);
        p.inventory_total = 1;
        p.broken = 1;
        h.tick(0.016, &p);
        assert!(h.findings.is_empty(), "a mined drop is not duplication");
    }

    #[test]
    fn standing_still_in_a_world_that_never_settles_is_a_hang() {
        let mut h = Harness::new(1, 600.0);
        let mut p = ok_probe();
        p.world_idle = false;
        run(&mut h, &p, STREAM_HANG_SECONDS + 2.0);
        assert!(h.findings.iter().any(|f| f.what.contains("still streaming")));
    }

    #[test]
    fn a_settled_world_never_reports_a_hang() {
        let mut h = Harness::new(1, 600.0);
        let p = ok_probe();
        run(&mut h, &p, STREAM_HANG_SECONDS + 5.0);
        assert!(!h.findings.iter().any(|f| f.what.contains("still streaming")));
    }

    #[test]
    fn a_failed_save_roundtrip_is_reported() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        p.save_roundtrip = Some(false);
        h.tick(0.016, &p);
        assert!(h.findings.iter().any(|f| f.what.contains("save round-trip")));
    }

    #[test]
    fn briefly_flush_with_the_ground_is_not_being_buried() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        p.inside_solid = true;
        run(&mut h, &p, 0.5);
        assert!(h.findings.is_empty());
    }

    #[test]
    fn being_buried_for_a_full_second_is_reported() {
        let mut h = Harness::new(1, 60.0);
        let mut p = ok_probe();
        p.inside_solid = true;
        run(&mut h, &p, 2.0);
        assert!(h.findings.iter().any(|f| f.what.contains("embedded")));
    }

    #[test]
    fn a_thin_session_fails_on_coverage_alone() {
        // Nothing mined, one biome, no mobs, no movement. Passing that would be
        // a lie, so the harness calls it out.
        let mut h = Harness::new(1, 60.0);
        h.finish();
        assert!(h.findings.len() >= 3, "a session that did nothing must fail");
        assert!(h
            .findings
            .iter()
            .all(|f| f.what.contains("coverage too thin")));
    }

    #[test]
    fn a_broad_session_passes_coverage() {
        let mut h = Harness::new(1, 60.0);
        h.coverage.blocks_mined.extend([1u8, 2, 3, 4]);
        h.coverage.biomes.extend([0u8, 1, 2]);
        h.coverage.saw_day = true;
        h.coverage.saw_night = true;
        h.coverage.met_a_hostile = true;
        h.coverage.distance = 900.0;
        h.coverage.save_checks = 2;
        h.finish();
        assert!(h.findings.is_empty(), "{}", h.findings[0].what);
    }

    #[test]
    fn the_same_seed_produces_the_same_session() {
        let p = ok_probe();
        let acts = |seed: u64| {
            let mut h = Harness::new(seed, 30.0);
            let mut v = Vec::new();
            while !h.done {
                h.tick(1.0 / 60.0, &p);
                v.push(h.act);
            }
            v
        };
        assert_eq!(acts(7), acts(7), "a seed must replay exactly");
        assert_ne!(acts(7), acts(8), "different seeds must diverge");
    }

    #[test]
    fn every_action_is_reachable_from_the_weight_table() {
        let mut seen: HashSet<Act> = HashSet::new();
        let p = ok_probe();
        for seed in 0..40u64 {
            let mut h = Harness::new(seed, 120.0);
            while !h.done {
                h.tick(1.0 / 60.0, &p);
                seen.insert(h.act);
            }
        }
        for (a, w) in WEIGHTS {
            if *w > 0 {
                assert!(seen.contains(a), "{a:?} was never chosen in 40 sessions");
            }
        }
    }
}
/// How long a block actually takes to break, simulated through the real carve
/// path. "Mining feels bad" is usually a number, and this is the number.
#[cfg(test)]
mod mining_feel {
    use crate::block::BlockId;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::config::{CHIP_INTERVAL, CHIP_RADIUS};
    use crate::item::{self, ItemId};
    use crate::world::World;
    use glam::Vec3;
    use std::sync::Arc;

    fn slab_of(id: BlockId) -> World {
        let mut w = World::new(1);
        for cy in 0..3 {
            for cz in -1..2 {
                for cx in -1..2 {
                    let pos = ChunkPos::new(cx, cy, cz);
                    let mut c = Chunk::new(pos);
                    c.generated = true;
                    w.chunks.insert(pos, Arc::new(c));
                }
            }
        }
        for y in 0..8 {
            for z in 0..8 {
                for x in 0..8 {
                    w.set_block(x, y, z, id);
                }
            }
        }
        w
    }

    /// Seconds of held left-click to destroy one block, mirroring `App::interact`.
    fn seconds_to_break(id: BlockId, tool: Option<ItemId>) -> f32 {
        let mut w = slab_of(id);
        let eye = Vec3::new(4.5, 12.0, 4.5);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let target = (4, 7, 4);
        let speed = item::mining_speed_multiplier(tool, id).max(0.01);
        let step = CHIP_INTERVAL * id.hardness() / speed;

        let mut t = 0.0f32;
        for _ in 0..20_000 {
            if w.block_at(target.0, target.1, target.2).is_air() {
                return t;
            }
            let aim = w
                .raycast(eye, dir, 20.0)
                .map(|h| h.point)
                .unwrap_or(Vec3::new(4.5, 7.5, 4.5));
            w.chip_block(target, aim, CHIP_RADIUS);
            t += step;
        }
        f32::INFINITY
    }

    /// Mining one block must leave its neighbours untouched. Before this, every
    /// chip spilled into adjacent blocks, so holding the button gouged a bowl
    /// across a wall instead of removing the block being aimed at.
    #[test]
    fn mining_a_block_does_not_damage_its_neighbours() {
        let mut w = slab_of(BlockId::STONE);
        let eye = Vec3::new(4.5, 12.0, 4.5);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let target = (4, 7, 4);

        for _ in 0..2000 {
            if w.block_at(target.0, target.1, target.2).is_air() {
                break;
            }
            let aim = w
                .raycast(eye, dir, 20.0)
                .map(|h| h.point)
                .unwrap_or(Vec3::new(4.5, 7.5, 4.5));
            w.chip_block(target, aim, CHIP_RADIUS);
        }
        assert!(
            w.block_at(target.0, target.1, target.2).is_air(),
            "the aimed block should be gone"
        );
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = (target.0 + dx, target.1, target.2 + dz);
            let f = w.fill_ratio(n.0, n.1, n.2);
            assert!(
                (f - 1.0).abs() < 1.0e-6,
                "neighbour {n:?} was chipped to {f:.3} while mining next to it"
            );
        }
        // The block directly underneath must also survive.
        let below = w.fill_ratio(target.0, target.1 - 1, target.2);
        assert!((below - 1.0).abs() < 1.0e-6, "the block below was eaten too");
    }

    /// An explosion is the opposite: it is supposed to span blocks.
    #[test]
    fn an_explosion_still_carves_across_block_boundaries() {
        let mut w = slab_of(BlockId::STONE);
        let hit = w
            .raycast(Vec3::new(4.5, 12.0, 4.5), Vec3::new(0.0, -1.0, 0.0), 20.0)
            .expect("ray should hit the slab");
        w.chip_sphere(&hit, 6.0);
        let spread = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .filter(|(dx, dz)| w.fill_ratio(4 + dx, 7, 4 + dz) < 1.0)
            .count();
        assert!(spread > 0, "an unconfined carve should reach neighbouring blocks");
    }

    #[test]
    fn report_time_to_break() {
        let tools = [
            ("bare hand", None),
            ("wood pick", Some(ItemId::WOODEN_PICKAXE)),
            ("stone pick", Some(ItemId::STONE_PICKAXE)),
            ("iron pick", Some(ItemId::IRON_PICKAXE)),
            ("iron axe", Some(ItemId::IRON_AXE)),
        ];
        println!("seconds to break one block (held left click):");
        for (bname, b) in [
            ("dirt", BlockId::DIRT),
            ("grass", BlockId::GRASS),
            ("stone", BlockId::STONE),
            ("cobble", BlockId::COBBLESTONE),
            ("iron ore", BlockId::IRON_ORE),
            ("wood", BlockId::WOOD),
        ] {
            let mut line = format!("  {bname:<9}");
            for (tname, t) in tools {
                line.push_str(&format!("  {tname}: {:>6.2}s", seconds_to_break(b, t)));
            }
            println!("{line}");
        }
    }

    /// Mining has to stay inside the range that feels like a game. Minecraft
    /// sits around 0.75 s for dirt by hand and 1.15 s for stone with a matching
    /// pickaxe; several seconds of holding a button feels broken.
    #[test]
    fn breaking_a_block_does_not_take_forever() {
        let cases = [
            (BlockId::DIRT, None, 1.5),
            (BlockId::GRASS, None, 1.5),
            (BlockId::STONE, Some(ItemId::STONE_PICKAXE), 1.5),
            (BlockId::IRON_ORE, Some(ItemId::STONE_PICKAXE), 2.5),
            // Bare-handed wood is meant to be slow; an axe is the answer, and
            // that gap existing is the point of tool tiers.
            (BlockId::WOOD, None, 3.5),
            (BlockId::WOOD, Some(ItemId::IRON_AXE), 1.0),
        ];
        for (block, tool, limit) in cases {
            let t = seconds_to_break(block, tool);
            assert!(
                t <= limit,
                "{block:?} with {tool:?} takes {t:.2}s to break, over the {limit:.1}s limit"
            );
            assert!(t > 0.05, "{block:?} broke instantly ({t:.3}s), which is its own problem");
        }
    }
}
