//! Mob entities and AI.
//!
//! Three hostiles -- a melee zombie, a ranged skeleton, an explosive creeper -- and one
//! passive pig. Each has health, an AABB, gravity, world collision, and a state machine
//! that runs idle -> investigate -> chase -> attack and back down again.
//!
//! # The rule that makes the game work
//!
//! **Investigation targets the last heard position, never the player's live position.**
//! A mob that hears a smash walks to *where the smash was*. If the player then stands
//! still and stays quiet, the mob arrives at an empty hole, loses patience, and wanders
//! off. Chasing only ever happens on line of sight, and it decays the moment sight is
//! lost. Without that split the sound system would be decoration and the player could
//! never outsmart anything.

use glam::{IVec3, Vec3};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::pathfind::{self, Path};
use crate::sound::{NoiseEvent, SoundField, VoxelWorld};

// =============================================================================
// ============================  TUNING BLOCK  =================================
//   Every dialable mob number lives here.
// =============================================================================

// --- physics ---
pub const GRAVITY: f32 = -30.0;
pub const TERMINAL_VELOCITY: f32 = -60.0;
/// Upward launch speed. 8.4 clears a one-block lip with room to spare.
pub const JUMP_SPEED: f32 = 8.4;
/// Horizontal acceleration blend per second while grounded.
pub const GROUND_ACCEL: f32 = 14.0;
pub const AIR_ACCEL: f32 = 3.0;
/// Gap left between a mob's AABB and a surface it rests against.
pub const SKIN: f32 = 1.0e-3;
/// Fall speed above which landing hurts, and damage per extra block/second.
pub const FALL_DAMAGE_SPEED: f32 = 22.0;
pub const FALL_DAMAGE_PER_SPEED: f32 = 0.5;

// --- steering / pathing ---
/// Horizontal distance at which a waypoint counts as reached.
pub const WAYPOINT_RADIUS: f32 = 0.55;
/// Seconds between path recomputes. Paths are NOT recomputed per frame.
pub const REPATH_INTERVAL: f32 = 0.7;
/// A target that has moved further than this forces an early repath.
pub const REPATH_TARGET_DELTA: f32 = 2.5;
/// Node cap for a mob repath. Smaller than the pathfinder's own cap: mobs are many.
pub const REPATH_NODE_BUDGET: usize = 900;
/// Paths recomputed per tick across all mobs. Spreads cost over frames.
pub const REPATHS_PER_TICK: usize = 3;
/// Seconds of near-zero progress before a mob jumps to unstick itself.
pub const STUCK_TIME: f32 = 0.45;

// --- perception ---
/// Seconds a mob keeps chasing after losing sight of the player.
pub const CHASE_MEMORY: f32 = 3.5;
/// Seconds a mob will keep walking toward a remembered noise.
pub const INVESTIGATE_PATIENCE: f32 = 12.0;
/// Distance at which "I have arrived at the noise" counts as satisfied.
pub const INVESTIGATE_ARRIVE_RADIUS: f32 = 2.0;
/// Seconds spent looking around after arriving at a noise that turned out to be nothing.
pub const INVESTIGATE_LOOK_TIME: f32 = 2.5;
/// A new noise must be this much louder than the one being investigated to steal focus.
pub const RETARGET_MARGIN: f32 = 1.35;

// --- wandering ---
pub const WANDER_RADIUS: f32 = 9.0;
pub const WANDER_PAUSE_MIN: f32 = 1.5;
pub const WANDER_PAUSE_MAX: f32 = 5.0;
/// Fraction of walk speed used while ambling.
pub const WANDER_SPEED_SCALE: f32 = 0.45;

// --- spawning / despawning ---
pub const HOSTILE_CAP: usize = 24;
pub const PASSIVE_CAP: usize = 10;
pub const SPAWN_INTERVAL: f32 = 2.0;
pub const SPAWN_ATTEMPTS_PER_WAVE: usize = 12;
pub const SPAWN_MIN_DISTANCE: f32 = 24.0;
pub const SPAWN_MAX_DISTANCE: f32 = 52.0;
/// Vertical span, around the player's own level, that spawn candidates are drawn from.
pub const SPAWN_VERTICAL_SPREAD: i32 = 18;
/// Mobs beyond this distance from the player are removed.
pub const DESPAWN_DISTANCE: f32 = 128.0;
/// Daylight below this counts as night for surface spawning. 0 = midnight, 1 = noon.
pub const NIGHT_DAYLIGHT: f32 = 0.25;
/// How far up the sky check looks before deciding a spot is open to the sky.
pub const SKY_SCAN_LIMIT: i32 = 48;

// --- creeper ---
pub const FUSE_TIME: f32 = 1.5;
/// Distance at which a creeper stops and starts its fuse.
pub const FUSE_RANGE: f32 = 3.0;
/// Get further than this and the fuse is abandoned.
pub const FUSE_ABORT_RANGE: f32 = 7.0;
/// Radius of the sub-voxel crater, in blocks.
pub const EXPLOSION_RADIUS: f32 = 3.0;
/// Rim raggedness: each block's effective radius is scaled into
/// `[1 - this, 1]`, so the crater edge is chewed rather than a perfect sphere.
pub const EXPLOSION_RAGGED: f32 = 0.22;
/// Damage at the centre of the blast, falling off linearly to zero at the rim.
pub const EXPLOSION_DAMAGE: f32 = 22.0;
/// Blast radius for hurting entities, as a multiple of the carve radius.
pub const EXPLOSION_HURT_SCALE: f32 = 1.6;

// --- skeleton ---
pub const ARROW_SPEED: f32 = 26.0;
pub const ARROW_GRAVITY: f32 = -14.0;
pub const ARROW_LIFETIME: f32 = 4.0;
pub const ARROW_DAMAGE: f32 = 3.5;
/// A skeleton backs away when the player is closer than this.
pub const SKELETON_KEEP_DISTANCE: f32 = 6.0;
/// Radius around the player's AABB that an arrow counts as hitting.
pub const ARROW_HIT_PAD: f32 = 0.25;

// --- per-kind stats ---
// Ordered: health, width, height, walk speed, sight, hearing threshold,
// attack damage, attack range, attack cooldown.
const ZOMBIE_STATS: MobStats = MobStats {
    max_health: 20.0,
    width: 0.6,
    height: 1.95,
    walk_speed: 3.1,
    sight_range: 16.0,
    hearing_threshold: 0.07,
    attack_damage: 4.0,
    attack_range: 1.6,
    attack_cooldown: 1.0,
    hostile: true,
};
const SKELETON_STATS: MobStats = MobStats {
    max_health: 16.0,
    width: 0.6,
    height: 1.95,
    walk_speed: 3.4,
    sight_range: 20.0,
    hearing_threshold: 0.06,
    attack_damage: ARROW_DAMAGE,
    attack_range: 14.0,
    attack_cooldown: 1.6,
    hostile: true,
};
const CREEPER_STATS: MobStats = MobStats {
    max_health: 16.0,
    width: 0.6,
    height: 1.7,
    walk_speed: 2.8,
    sight_range: 16.0,
    hearing_threshold: 0.09,
    attack_damage: EXPLOSION_DAMAGE,
    attack_range: FUSE_RANGE,
    attack_cooldown: 0.0,
    hostile: true,
};
const PIG_STATS: MobStats = MobStats {
    max_health: 10.0,
    width: 0.9,
    height: 0.9,
    walk_speed: 1.8,
    sight_range: 10.0,
    hearing_threshold: f32::INFINITY, // passives do not investigate noise
    attack_damage: 0.0,
    attack_range: 0.0,
    attack_cooldown: 0.0,
    hostile: false,
};

// =============================================================================
// ==========================  END TUNING BLOCK  ===============================
// =============================================================================

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum MobKind {
    /// Melee. Walks at you and hits you.
    Zombie,
    /// Ranged. Keeps its distance and shoots arrows.
    Skeleton,
    /// Closes, fuses, and blows a real sub-voxel crater in the world.
    Creeper,
    /// Passive. Wanders, drops meat.
    Pig,
}

impl MobKind {
    pub const ALL: [MobKind; 4] = [
        MobKind::Zombie,
        MobKind::Skeleton,
        MobKind::Creeper,
        MobKind::Pig,
    ];
    pub const HOSTILES: [MobKind; 3] = [MobKind::Zombie, MobKind::Skeleton, MobKind::Creeper];

    pub fn stats(self) -> MobStats {
        match self {
            MobKind::Zombie => ZOMBIE_STATS,
            MobKind::Skeleton => SKELETON_STATS,
            MobKind::Creeper => CREEPER_STATS,
            MobKind::Pig => PIG_STATS,
        }
    }

    pub fn is_hostile(self) -> bool {
        self.stats().hostile
    }

    /// Full AABB extent, in blocks: (width, height, width).
    pub fn size(self) -> Vec3 {
        let s = self.stats();
        Vec3::new(s.width, s.height, s.width)
    }

    /// Flat body colour, matching the game's no-texture look. sRGB.
    pub fn color(self) -> [f32; 3] {
        match self {
            MobKind::Zombie => [0.30, 0.55, 0.32],
            MobKind::Skeleton => [0.82, 0.82, 0.78],
            MobKind::Creeper => [0.22, 0.70, 0.28],
            MobKind::Pig => [0.90, 0.60, 0.62],
        }
    }

    fn drop_kind(self) -> DropKind {
        match self {
            MobKind::Zombie => DropKind::RottenFlesh,
            MobKind::Skeleton => DropKind::Bone,
            MobKind::Creeper => DropKind::Gunpowder,
            MobKind::Pig => DropKind::Meat,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobStats {
    pub max_health: f32,
    pub width: f32,
    pub height: f32,
    pub walk_speed: f32,
    pub sight_range: f32,
    /// Audibility a sound must clear before this kind reacts to it.
    pub hearing_threshold: f32,
    pub attack_damage: f32,
    pub attack_range: f32,
    pub attack_cooldown: f32,
    pub hostile: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MobState {
    /// Standing around.
    Idle,
    /// Ambling to a nearby point for no reason.
    Wander,
    /// Walking to a **remembered sound position**. Never the player's live position.
    Investigate,
    /// Walking to where the player was last *seen*.
    Chase,
    /// In contact and swinging / shooting.
    Attack,
    /// Creeper only: stopped, primed, counting down.
    Fuse,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DropKind {
    Meat,
    RottenFlesh,
    Bone,
    Arrow,
    Gunpowder,
}

/// Everything that happened this tick that the rest of the game must react to.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MobEvent {
    PlayerDamaged {
        amount: f32,
        source: MobKind,
        from: Vec3,
    },
    MobDied {
        id: u32,
        kind: MobKind,
        pos: Vec3,
    },
    Drop {
        kind: DropKind,
        pos: Vec3,
        count: u32,
    },
    /// A creeper went off. The world has already been carved when you see this.
    Exploded {
        pos: Vec3,
        radius: f32,
    },
}

/// What mob AI needs to know about the player. Built fresh each tick.
#[derive(Copy, Clone, Debug)]
pub struct PlayerState {
    /// Feet position.
    pub pos: Vec3,
    /// Eye position, for line-of-sight tests.
    pub eye: Vec3,
    /// Dead players are not chased or attacked.
    pub alive: bool,
}

impl PlayerState {
    pub fn new(pos: Vec3, eye_height: f32) -> Self {
        Self {
            pos,
            eye: pos + Vec3::Y * eye_height,
            alive: true,
        }
    }
}

/// A skeleton's arrow.
#[derive(Copy, Clone, Debug)]
pub struct Projectile {
    pub pos: Vec3,
    pub vel: Vec3,
    pub ttl: f32,
    pub damage: f32,
    pub owner: u32,
}

/// One mob. Public fields are the renderer's contract: `pos`, `kind`, `yaw`, plus
/// `size()` for the AABB and `MobKind::color()` for the flat body colour.
#[derive(Clone, Debug)]
pub struct Mob {
    pub id: u32,
    pub kind: MobKind,
    /// Feet centre, in block units.
    pub pos: Vec3,
    pub vel: Vec3,
    /// Facing, radians, 0 along +X. Renderers orient the body with this.
    pub yaw: f32,
    pub health: f32,
    pub on_ground: bool,
    /// Horizontal distance actually travelled, in blocks. Renderers drive the
    /// walk cycle from this rather than from a clock, so a mob shoving against
    /// a wall stops striding instead of moonwalking on the spot.
    pub gait: f32,
    /// Held in place by the model review stand; skips physics.
    pub pinned: bool,
    pub state: MobState,
    /// Where the mob is currently heading. In `Investigate` this is a sound position.
    pub target: Vec3,
    /// Audibility of the noise being investigated, for retarget comparisons.
    pub heard_loudness: f32,

    patience: f32,
    look_timer: f32,
    chase_memory: f32,
    attack_cd: f32,
    fuse: f32,
    wander_pause: f32,
    path: Path,
    path_index: usize,
    repath_timer: f32,
    stuck_timer: f32,
    last_progress_pos: Vec3,
}

impl Mob {
    pub fn new(id: u32, kind: MobKind, pos: Vec3) -> Self {
        Self {
            id,
            kind,
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            health: kind.stats().max_health,
            on_ground: false,
            gait: 0.0,
            pinned: false,
            state: MobState::Idle,
            target: pos,
            heard_loudness: 0.0,
            patience: 0.0,
            look_timer: 0.0,
            chase_memory: 0.0,
            attack_cd: 0.0,
            fuse: 0.0,
            wander_pause: 0.0,
            path: Path::default(),
            path_index: 0,
            repath_timer: 0.0,
            stuck_timer: 0.0,
            last_progress_pos: pos,
        }
    }

    /// Full AABB extent (width, height, width).
    pub fn size(&self) -> Vec3 {
        self.kind.size()
    }

    /// Eye position, used for sight and for listening.
    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::Y * (self.kind.stats().height * 0.85)
    }

    /// Centre of the body, useful for billboards and hit tests.
    pub fn centre(&self) -> Vec3 {
        self.pos + Vec3::Y * (self.kind.stats().height * 0.5)
    }

    pub fn health_fraction(&self) -> f32 {
        (self.health / self.kind.stats().max_health).clamp(0.0, 1.0)
    }

    /// 0 when not fusing, ramping to 1 at detonation. Renderers flash on this.
    pub fn fuse_fraction(&self) -> f32 {
        if self.state == MobState::Fuse {
            (self.fuse / FUSE_TIME).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Cell the mob's feet are in.
    pub fn feet_cell(&self) -> IVec3 {
        self.pos.floor().as_ivec3()
    }

    fn is_alerted(&self) -> bool {
        matches!(
            self.state,
            MobState::Investigate | MobState::Chase | MobState::Attack | MobState::Fuse
        )
    }
}

// -----------------------------------------------------------------------------
// Physics
// -----------------------------------------------------------------------------

fn aabb(pos: Vec3, size: Vec3) -> (Vec3, Vec3) {
    let half = Vec3::new(size.x * 0.5, 0.0, size.z * 0.5);
    (pos - half, pos + half + Vec3::new(0.0, size.y, 0.0))
}

/// Move one axis and resolve into the world. Returns true if something was hit.
fn resolve_axis<W: VoxelWorld + ?Sized>(
    world: &W,
    pos: &mut Vec3,
    size: Vec3,
    delta: f32,
    axis: usize,
) -> bool {
    if delta == 0.0 {
        return false;
    }
    pos[axis] += delta;
    let (mn, mx) = aabb(*pos, size);
    let b0 = mn.floor().as_ivec3();
    let b1 = (mx - Vec3::splat(SKIN)).floor().as_ivec3();

    let mut hit = false;
    let mut limit = if delta > 0.0 {
        f32::INFINITY
    } else {
        f32::NEG_INFINITY
    };
    for x in b0.x..=b1.x {
        for y in b0.y..=b1.y {
            for z in b0.z..=b1.z {
                if !pathfind::cell_blocked(world, IVec3::new(x, y, z)) {
                    continue;
                }
                hit = true;
                let lo = [x, y, z][axis] as f32;
                if delta > 0.0 {
                    limit = limit.min(lo);
                } else {
                    limit = limit.max(lo + 1.0);
                }
            }
        }
    }
    if hit {
        if axis == 1 {
            pos.y = if delta > 0.0 {
                limit - size.y - SKIN
            } else {
                limit + SKIN
            };
        } else {
            let half = size[axis] * 0.5;
            pos[axis] = if delta > 0.0 {
                limit - half - SKIN
            } else {
                limit + half + SKIN
            };
        }
    }
    hit
}

/// Integrate one mob's velocity against the world. Returns `(hit_wall, landed_speed)`.
fn step_physics<W: VoxelWorld + ?Sized>(world: &W, mob: &mut Mob, dt: f32) -> (bool, f32) {
    if mob.pinned {
        return (false, 0.0);
    }
    let size = mob.size();
    let was = mob.pos;
    mob.vel.y = (mob.vel.y + GRAVITY * dt).max(TERMINAL_VELOCITY);

    let hit_x = resolve_axis(world, &mut mob.pos, size, mob.vel.x * dt, 0);
    let hit_z = resolve_axis(world, &mut mob.pos, size, mob.vel.z * dt, 2);
    mob.gait += ((mob.pos.x - was.x).powi(2) + (mob.pos.z - was.z).powi(2)).sqrt();
    if hit_x {
        mob.vel.x = 0.0;
    }
    if hit_z {
        mob.vel.z = 0.0;
    }

    let fall_speed = -mob.vel.y;
    let hit_y = resolve_axis(world, &mut mob.pos, size, mob.vel.y * dt, 1);
    let landed = hit_y && mob.vel.y < 0.0;
    mob.on_ground = landed;
    if hit_y {
        mob.vel.y = 0.0;
    }

    // Safety valve: if a mob ends up inside geometry (a block placed on it, a bad
    // spawn), push it up rather than letting it fall through the world.
    if pathfind::cell_blocked(world, mob.feet_cell()) {
        mob.pos.y += 4.0 * dt;
    }

    (hit_x || hit_z, if landed { fall_speed } else { 0.0 })
}

/// Exact voxel traversal test: is there an unbroken line of sight from `from` to `to`?
fn line_of_sight<W: VoxelWorld + ?Sized>(world: &W, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let dist = delta.length();
    if dist < 1.0e-4 {
        return true;
    }
    let dir = delta / dist;
    let mut cell = from.floor().as_ivec3();
    let end = to.floor().as_ivec3();

    let step = IVec3::new(
        if dir.x > 0.0 { 1 } else { -1 },
        if dir.y > 0.0 { 1 } else { -1 },
        if dir.z > 0.0 { 1 } else { -1 },
    );
    // Distance along the ray to the next grid plane on each axis, and per-cell spacing.
    let inv = Vec3::new(
        if dir.x.abs() < 1.0e-6 {
            f32::INFINITY
        } else {
            1.0 / dir.x.abs()
        },
        if dir.y.abs() < 1.0e-6 {
            f32::INFINITY
        } else {
            1.0 / dir.y.abs()
        },
        if dir.z.abs() < 1.0e-6 {
            f32::INFINITY
        } else {
            1.0 / dir.z.abs()
        },
    );
    let mut t_max = Vec3::new(
        next_boundary(from.x, dir.x) * inv.x,
        next_boundary(from.y, dir.y) * inv.y,
        next_boundary(from.z, dir.z) * inv.z,
    );

    // Bound the walk: a ray can cross at most this many cells.
    let guard = (dist.ceil() as i32 + 1) * 3;
    for _ in 0..guard {
        if cell == end {
            return true;
        }
        if t_max.x < t_max.y && t_max.x < t_max.z {
            cell.x += step.x;
            t_max.x += inv.x;
        } else if t_max.y < t_max.z {
            cell.y += step.y;
            t_max.y += inv.y;
        } else {
            cell.z += step.z;
            t_max.z += inv.z;
        }
        if t_max.min_element() > dist {
            return true;
        }
        if pathfind::cell_blocked(world, cell) {
            return false;
        }
    }
    true
}

/// Distance from `p` to the next grid line in direction `d`, in axis units.
fn next_boundary(p: f32, d: f32) -> f32 {
    if d.abs() < 1.0e-6 {
        return f32::INFINITY;
    }
    let f = p - p.floor();
    if d > 0.0 { 1.0 - f } else { f }
}

#[inline]
fn horiz(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

// -----------------------------------------------------------------------------
// Explosions -- where sub-voxel destruction and mob AI meet
// -----------------------------------------------------------------------------

/// Deterministic 0..1 hash of a block position. Gives each block its own crater rim,
/// so blasts look chewed rather than stamped, and stay reproducible across runs.
fn block_hash01(p: IVec3) -> f32 {
    let mut h = (p.x as u32).wrapping_mul(0x9E37_79B1)
        ^ (p.y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (p.z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// What one explosion did to the world.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BlastReport {
    /// Blocks emptied entirely.
    pub blocks_destroyed: u32,
    /// Blocks left standing but chewed.
    pub blocks_damaged: u32,
    /// Sub-voxel carve calls issued.
    pub subvoxels_carved: u64,
}

/// Blow a crater into the world at `centre`.
///
/// This carves **sub-voxels**, not blocks: the middle of the blast empties out and
/// vanishes, while the rim is left as real partial geometry. Blocks whose every
/// sub-voxel falls inside the blast are cleared with one `set_block` instead of 512
/// carves -- otherwise a single creeper would cost a quarter of a million hash
/// lookups and visibly hitch the frame.
pub fn explode<W: VoxelWorld + ?Sized>(world: &mut W, centre: Vec3, radius: f32) -> BlastReport {
    let mut report = BlastReport::default();
    if radius <= 0.0 {
        return report;
    }
    let r_max = radius;
    let r_min = radius * (1.0 - EXPLOSION_RAGGED);
    let reach = r_max.ceil() as i32 + 1;
    let base = centre.floor().as_ivec3();
    let sub = 1.0 / 8.0;

    for bx in base.x - reach..=base.x + reach {
        for by in base.y - reach..=base.y + reach {
            for bz in base.z - reach..=base.z + reach {
                let p = IVec3::new(bx, by, bz);
                if p.y < pathfind::WORLD_MIN_Y || p.y >= pathfind::WORLD_MAX_Y {
                    continue;
                }
                let block = world.block_at(p.x, p.y, p.z);
                if block.is_air() || !block.is_solid() {
                    continue;
                }
                // Bedrock and anything else unmineable shrugs it off.
                if block.hardness().is_infinite() {
                    continue;
                }

                // Nearest and furthest sub-voxel centres of this block.
                let lo = p.as_vec3() + Vec3::splat(sub * 0.5);
                let hi = p.as_vec3() + Vec3::splat(1.0 - sub * 0.5);
                let near = centre.clamp(lo, hi).distance(centre);
                let far = lo.distance(centre).max(hi.distance(centre)).max(
                    // the true far corner is whichever of the 8 is furthest
                    Vec3::new(lo.x, lo.y, hi.z)
                        .distance(centre)
                        .max(Vec3::new(hi.x, lo.y, lo.z).distance(centre))
                        .max(Vec3::new(lo.x, hi.y, lo.z).distance(centre))
                        .max(Vec3::new(hi.x, hi.y, lo.z).distance(centre))
                        .max(Vec3::new(hi.x, lo.y, hi.z).distance(centre))
                        .max(Vec3::new(lo.x, hi.y, hi.z).distance(centre)),
                );

                if near > r_max {
                    continue; // wholly outside the blast
                }
                let r_eff = r_min + (r_max - r_min) * block_hash01(p);
                if far <= r_eff {
                    // Wholly inside: clear it in one call.
                    world.set_block(p.x, p.y, p.z, crate::block::BlockId::AIR);
                    report.blocks_destroyed += 1;
                    continue;
                }
                if near > r_eff {
                    continue;
                }

                let mut carved = 0u64;
                let mut emptied = false;
                for sy in 0..8 {
                    for sz in 0..8 {
                        for sx in 0..8 {
                            let c = p.as_vec3()
                                + Vec3::new(sx as f32, sy as f32, sz as f32) * sub
                                + Vec3::splat(sub * 0.5);
                            if c.distance(centre) > r_eff {
                                continue;
                            }
                            if world.sub_solid(p.x, p.y, p.z, sx, sy, sz) {
                                emptied |= world.carve(p.x, p.y, p.z, sx, sy, sz);
                                carved += 1;
                            }
                        }
                    }
                }
                report.subvoxels_carved += carved;
                if emptied {
                    report.blocks_destroyed += 1;
                } else if carved > 0 {
                    report.blocks_damaged += 1;
                }
            }
        }
    }
    report
}

// -----------------------------------------------------------------------------
// Manager
// -----------------------------------------------------------------------------

/// Owns every mob and arrow, and runs the whole system for one tick.
pub struct MobManager {
    mobs: Vec<Mob>,
    projectiles: Vec<Projectile>,
    events: Vec<MobEvent>,
    rng: StdRng,
    next_id: u32,
    spawn_timer: f32,
    /// Round-robin cursor so only a few mobs repath per tick.
    repath_cursor: usize,
    /// Set false to stop hostile spawning (creative/peaceful, or tests).
    pub spawning_enabled: bool,
    scratch: Vec<(IVec3, f32)>,
}

impl MobManager {
    pub fn new(seed: u64) -> Self {
        Self {
            mobs: Vec::new(),
            projectiles: Vec::new(),
            events: Vec::new(),
            rng: StdRng::seed_from_u64(seed),
            next_id: 1,
            spawn_timer: SPAWN_INTERVAL,
            repath_cursor: 0,
            spawning_enabled: true,
            scratch: Vec::with_capacity(8),
        }
    }

    // --- renderer / integrator surface ---

    pub fn mobs(&self) -> &[Mob] {
        &self.mobs
    }
    pub fn projectiles(&self) -> &[Projectile] {
        &self.projectiles
    }
    pub fn len(&self) -> usize {
        self.mobs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.mobs.is_empty()
    }
    pub fn hostile_count(&self) -> usize {
        self.mobs.iter().filter(|m| m.kind.is_hostile()).count()
    }
    pub fn passive_count(&self) -> usize {
        self.mobs.iter().filter(|m| !m.kind.is_hostile()).count()
    }
    pub fn get(&self, id: u32) -> Option<&Mob> {
        self.mobs.iter().find(|m| m.id == id)
    }
    pub fn clear(&mut self) {
        self.mobs.clear();
        self.projectiles.clear();
    }

    /// Place a mob explicitly. Returns its id.
    /// Point a mob a given way and stop it thinking. Used by the model review
    /// stand so the rig is judged in its rest pose rather than mid-stride.
    pub fn face_and_freeze(&mut self, id: u32, yaw: f32) {
        if let Some(m) = self.mobs.iter_mut().find(|m| m.id == id) {
            m.yaw = yaw;
            m.vel = Vec3::ZERO;
            m.gait = 0.0;
            m.state = MobState::Idle;
        }
        self.spawning_enabled = false;
    }

    /// Force a mob to a position and keep it there. Review-stand only.
    pub fn pin(&mut self, id: u32, pos: Vec3) {
        if let Some(m) = self.mobs.iter_mut().find(|m| m.id == id) {
            m.pos = pos;
            m.vel = Vec3::ZERO;
            m.pinned = true;
        }
    }

    pub fn spawn(&mut self, kind: MobKind, pos: Vec3) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.mobs.push(Mob::new(id, kind, pos));
        id
    }

    /// Place a mob with an explicit velocity, facing and health -- how a mob comes
    /// back out of a save file.
    ///
    /// Identical to [`MobManager::spawn`] in every other respect: the mob starts
    /// idle, with no path and no AI timers running, and re-acquires the player the
    /// same way a freshly spawned one does. `health` is clamped to the kind's
    /// maximum; passing zero or less means the mob dies on the next `update`.
    pub fn spawn_restored(
        &mut self,
        kind: MobKind,
        pos: Vec3,
        vel: Vec3,
        yaw: f32,
        health: f32,
        on_ground: bool,
    ) -> u32 {
        let id = self.spawn(kind, pos);
        if let Some(m) = self.mobs.last_mut() {
            m.vel = vel;
            m.yaw = yaw;
            m.health = health.min(kind.stats().max_health);
            m.on_ground = on_ground;
        }
        id
    }

    /// Hurt a mob. Returns true if this killed it. Drops are pushed as events on the
    /// next `update`, or immediately if you drain events yourself.
    pub fn damage(&mut self, id: u32, amount: f32, knockback: Vec3) -> bool {
        let Some(m) = self.mobs.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        m.health -= amount;
        m.vel += knockback;
        // Being hit is as good as seeing the attacker: go and look.
        if m.kind.is_hostile() && !m.is_alerted() {
            m.state = MobState::Investigate;
            m.target = m.pos - horiz(knockback).normalize_or_zero() * 4.0;
            m.patience = INVESTIGATE_PATIENCE;
            m.repath_timer = 0.0;
        }
        if m.health <= 0.0 {
            let (kind, pos) = (m.kind, m.pos);
            self.kill(id, kind, pos);
            return true;
        }
        false
    }

    fn kill(&mut self, id: u32, kind: MobKind, pos: Vec3) {
        self.events.push(MobEvent::MobDied { id, kind, pos });
        let count = if kind == MobKind::Pig { 2 } else { 1 };
        self.events.push(MobEvent::Drop {
            kind: kind.drop_kind(),
            pos,
            count,
        });
        self.mobs.retain(|m| m.id != id);
        self.projectiles.retain(|p| p.owner != id);
    }

    /// First mob a ray hits, for the player's own attacks.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_distance: f32) -> Option<(u32, f32)> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let mut best: Option<(u32, f32)> = None;
        for m in &self.mobs {
            let (mn, mx) = aabb(m.pos, m.size());
            if let Some(t) = ray_aabb(origin, dir, mn, mx)
                && t <= max_distance
                && best.is_none_or(|(_, bt)| t < bt)
            {
                best = Some((m.id, t));
            }
        }
        best
    }

    /// Run every mob for one tick. Call once per frame, after `SoundField::update`.
    ///
    /// `daylight` is 0 at midnight and 1 at noon; it gates surface spawning only.
    /// Returns everything the rest of the game must react to this tick.
    pub fn update<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &mut W,
        sound: &mut SoundField,
        player: &PlayerState,
        daylight: f32,
        dt: f32,
    ) -> Vec<MobEvent> {
        let dt = dt.clamp(0.0, 0.1); // a long stall must not teleport mobs through walls

        self.despawn_far(player);
        self.spawn_wave(world, player, daylight, dt);

        // Only a few mobs may repath this tick.
        let mut repath_left = REPATHS_PER_TICK;
        let n = self.mobs.len();
        for i in 0..n {
            let idx = (self.repath_cursor + i) % n.max(1);
            if idx >= self.mobs.len() {
                continue;
            }
            self.tick_mob(world, sound, player, idx, dt, &mut repath_left);
        }
        self.repath_cursor = self.repath_cursor.wrapping_add(REPATHS_PER_TICK);

        self.detonate_ready(world, sound, player);
        self.tick_projectiles(world, player, dt);
        self.reap_dead();

        std::mem::take(&mut self.events)
    }

    fn despawn_far(&mut self, player: &PlayerState) {
        let p = player.pos;
        self.mobs
            .retain(|m| (m.pos - p).length() <= DESPAWN_DISTANCE);
    }

    fn reap_dead(&mut self) {
        let dead: Vec<(u32, MobKind, Vec3)> = self
            .mobs
            .iter()
            .filter(|m| m.health <= 0.0)
            .map(|m| (m.id, m.kind, m.pos))
            .collect();
        for (id, kind, pos) in dead {
            self.kill(id, kind, pos);
        }
    }

    // --- perception and state machine ---

    fn tick_mob<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &W,
        sound: &SoundField,
        player: &PlayerState,
        idx: usize,
        dt: f32,
        repath_left: &mut usize,
    ) {
        // A pinned mob is being photographed, not simulated. Skipping physics
        // alone was not enough: the state machine kept re-steering its yaw every
        // frame, so the review stand quietly turned every model back around.
        if self.mobs[idx].pinned {
            return;
        }
        let mut mob = self.mobs[idx].clone();
        let stats = mob.kind.stats();

        mob.attack_cd = (mob.attack_cd - dt).max(0.0);
        mob.chase_memory = (mob.chase_memory - dt).max(0.0);
        mob.patience = (mob.patience - dt).max(0.0);
        mob.look_timer = (mob.look_timer - dt).max(0.0);
        mob.repath_timer -= dt;

        let to_player = player.pos - mob.pos;
        let player_dist = to_player.length();

        // --- sight ---
        let visible = player.alive
            && stats.hostile
            && player_dist <= stats.sight_range
            && line_of_sight(world, mob.eye(), player.eye);

        // --- hearing: the loudest thing audible HERE, and where it came from ---
        let heard = if stats.hearing_threshold.is_finite() {
            sound.loudest_above(mob.eye(), stats.hearing_threshold)
        } else {
            None
        };

        // --- transitions ---
        // A creeper that has begun priming is COMMITTED: only its own abort check
        // (player out of range, or dead) may pull it out of Fuse. Without this
        // guard the generic `visible -> Chase` arm below overwrites the state every
        // tick, the creeper re-lights its fuse from zero each time, and it can
        // never reach FUSE_TIME -- it just follows the player around forever.
        let priming = mob.state == MobState::Fuse;
        if priming {
            // Held by the kind-specific block below.
        } else if visible {
            mob.state = MobState::Chase;
            mob.target = player.pos; // live position, but ONLY while actually seen
            mob.chase_memory = CHASE_MEMORY;
        } else if mob.state == MobState::Chase && mob.chase_memory > 0.0 {
            // Keep heading for where the player was last seen. Target is not updated.
        } else if let Some(h) = heard {
            let steal = mob.state != MobState::Investigate
                || h.loudness > mob.heard_loudness * RETARGET_MARGIN
                || horiz(mob.target - mob.pos).length() < INVESTIGATE_ARRIVE_RADIUS;
            if steal {
                // THE position we walk to is the SOUND's, never the player's.
                mob.state = MobState::Investigate;
                mob.target = h.pos;
                mob.heard_loudness = h.loudness;
                mob.patience = INVESTIGATE_PATIENCE;
                mob.look_timer = 0.0;
                mob.repath_timer = 0.0;
            }
        } else if mob.state == MobState::Chase {
            // Memory ran out: go poke around the last known spot, then give up.
            mob.state = MobState::Investigate;
            mob.patience = INVESTIGATE_LOOK_TIME * 2.0;
            mob.heard_loudness = 0.0;
        }

        if mob.state == MobState::Investigate {
            let arrived = horiz(mob.target - mob.pos).length() < INVESTIGATE_ARRIVE_RADIUS;
            if arrived && mob.look_timer <= 0.0 {
                mob.look_timer = INVESTIGATE_LOOK_TIME;
            }
            // Nothing here, and nothing left to hear: lose interest.
            if mob.patience <= 0.0 || (arrived && mob.look_timer <= 0.0 && heard.is_none()) {
                mob.state = MobState::Idle;
                mob.heard_loudness = 0.0;
                mob.wander_pause = 0.0;
            }
        }

        if matches!(mob.state, MobState::Idle | MobState::Wander) {
            self.tick_wander(world, &mut mob, dt);
        }

        // --- attacking ---
        match mob.kind {
            MobKind::Zombie => {
                if visible && player_dist <= stats.attack_range {
                    mob.state = MobState::Attack;
                    if mob.attack_cd <= 0.0 {
                        mob.attack_cd = stats.attack_cooldown;
                        self.events.push(MobEvent::PlayerDamaged {
                            amount: stats.attack_damage,
                            source: mob.kind,
                            from: mob.pos,
                        });
                    }
                }
            }
            MobKind::Skeleton => {
                if visible && player_dist <= stats.attack_range && mob.attack_cd <= 0.0 {
                    mob.attack_cd = stats.attack_cooldown;
                    let from = mob.eye();
                    let aim = aim_arrow(from, player.pos + Vec3::Y * 0.9);
                    self.projectiles.push(Projectile {
                        pos: from,
                        vel: aim,
                        ttl: ARROW_LIFETIME,
                        damage: ARROW_DAMAGE,
                        owner: mob.id,
                    });
                }
            }
            MobKind::Creeper => {
                let close = player_dist <= FUSE_RANGE;
                if mob.state == MobState::Fuse {
                    if player_dist > FUSE_ABORT_RANGE || !player.alive {
                        mob.state = MobState::Chase;
                        mob.fuse = 0.0;
                    } else {
                        mob.fuse += dt;
                    }
                } else if close && visible {
                    // Light the fuse and start it burning on this same tick --
                    // otherwise the first tick is dead time and the fuse reads as
                    // not-yet-started to anything drawing a priming indicator.
                    mob.state = MobState::Fuse;
                    mob.fuse = dt;
                }
            }
            MobKind::Pig => {}
        }

        // --- movement ---
        self.steer(world, &mut mob, player, visible, dt, repath_left);
        let (hit_wall, land_speed) = step_physics(world, &mut mob, dt);

        if land_speed > FALL_DAMAGE_SPEED {
            mob.health -= (land_speed - FALL_DAMAGE_SPEED) * FALL_DAMAGE_PER_SPEED;
        }

        // Unstick: no progress for a while while trying to move -> hop.
        if (mob.pos - mob.last_progress_pos).length() > 0.35 {
            mob.last_progress_pos = mob.pos;
            mob.stuck_timer = 0.0;
        } else if mob.is_alerted() || mob.state == MobState::Wander {
            mob.stuck_timer += dt;
        }
        if (hit_wall || mob.stuck_timer > STUCK_TIME) && mob.on_ground {
            mob.vel.y = JUMP_SPEED;
            mob.stuck_timer = 0.0;
        }

        self.mobs[idx] = mob;
    }

    fn tick_wander<W: VoxelWorld + ?Sized>(&mut self, world: &W, mob: &mut Mob, dt: f32) {
        mob.wander_pause -= dt;
        let arrived = horiz(mob.target - mob.pos).length() < 1.0;
        if mob.state == MobState::Idle || arrived {
            if mob.wander_pause > 0.0 {
                mob.state = MobState::Idle;
                return;
            }
            let a = self.rng.random_range(0.0f32..std::f32::consts::TAU);
            let r = self.rng.random_range(3.0..WANDER_RADIUS);
            let want = mob.pos + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
            let cell = want.floor().as_ivec3();
            mob.target = match pathfind::snap_to_ground(world, cell) {
                Some(c) => c.as_vec3() + Vec3::new(0.5, 0.0, 0.5),
                None => mob.pos,
            };
            mob.state = MobState::Wander;
            mob.wander_pause = self.rng.random_range(WANDER_PAUSE_MIN..WANDER_PAUSE_MAX);
            mob.repath_timer = 0.0;
        }
    }

    /// Turn the current target into a velocity, repathing on a timer (never per frame).
    fn steer<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &W,
        mob: &mut Mob,
        player: &PlayerState,
        visible: bool,
        dt: f32,
        repath_left: &mut usize,
    ) {
        let stats = mob.kind.stats();
        let mut speed = stats.walk_speed;
        let mut goal = mob.target;

        match mob.state {
            MobState::Idle => {
                decelerate(mob, dt);
                return;
            }
            MobState::Fuse => {
                decelerate(mob, dt);
                face(mob, player.pos - mob.pos);
                return;
            }
            MobState::Attack => {
                // Close the last stride by hand; pathing at contact range is noise.
                goal = player.pos;
            }
            MobState::Wander => speed *= WANDER_SPEED_SCALE,
            MobState::Chase | MobState::Investigate => {}
        }

        // A skeleton with a clear shot backs off instead of closing.
        if mob.kind == MobKind::Skeleton && visible {
            let d = (player.pos - mob.pos).length();
            if d < SKELETON_KEEP_DISTANCE {
                let away = horiz(mob.pos - player.pos).normalize_or_zero();
                accelerate(mob, away * speed, dt);
                face(mob, player.pos - mob.pos);
                return;
            }
        }

        let goal_cell = goal.floor().as_ivec3();
        let need_repath = mob.repath_timer <= 0.0
            || mob.path.waypoints.is_empty()
            || mob
                .path
                .end()
                .is_none_or(|e| (e.as_vec3() - goal_cell.as_vec3()).length() > REPATH_TARGET_DELTA);

        if need_repath && *repath_left > 0 {
            *repath_left -= 1;
            mob.repath_timer = REPATH_INTERVAL;
            mob.path = pathfind::find_path(world, mob.feet_cell(), goal_cell, REPATH_NODE_BUDGET);
            mob.path_index = 0;
        }

        // Advance along the path.
        let mut aim = goal;
        if !mob.path.waypoints.is_empty() {
            while mob.path_index < mob.path.waypoints.len() {
                let wp = waypoint_centre(mob.path.waypoints[mob.path_index]);
                if horiz(wp - mob.pos).length() < WAYPOINT_RADIUS && (wp.y - mob.pos.y).abs() < 1.2
                {
                    mob.path_index += 1;
                } else {
                    break;
                }
            }
            if mob.path_index < mob.path.waypoints.len() {
                aim = waypoint_centre(mob.path.waypoints[mob.path_index]);
                // A waypoint above us means climb: hop as we approach it.
                if aim.y > mob.pos.y + 0.5 && mob.on_ground && horiz(aim - mob.pos).length() < 1.4 {
                    mob.vel.y = JUMP_SPEED;
                }
            } else if mob.path.reached_goal {
                aim = goal;
            }
        }

        let dir = horiz(aim - mob.pos);
        if dir.length_squared() > 1.0e-4 {
            accelerate(mob, dir.normalize() * speed, dt);
            face(mob, dir);
        } else {
            decelerate(mob, dt);
        }
        // Scratch buffer kept warm for callers of `successors`; not needed here.
        self.scratch.clear();
    }

    // --- creeper detonation ---

    fn detonate_ready<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &mut W,
        sound: &mut SoundField,
        player: &PlayerState,
    ) {
        let ready: Vec<(u32, Vec3)> = self
            .mobs
            .iter()
            .filter(|m| {
                m.kind == MobKind::Creeper && m.state == MobState::Fuse && m.fuse >= FUSE_TIME
            })
            .map(|m| (m.id, m.centre()))
            .collect();

        for (id, centre) in ready {
            explode(world, centre, EXPLOSION_RADIUS);
            self.events.push(MobEvent::Exploded {
                pos: centre,
                radius: EXPLOSION_RADIUS,
            });
            // The blast is itself a very loud noise: every mob in earshot converges.
            sound.emit_immediate(world, NoiseEvent::explosion(centre));

            let hurt_r = EXPLOSION_RADIUS * EXPLOSION_HURT_SCALE;
            let d = (player.pos + Vec3::Y * 0.9 - centre).length();
            if player.alive && d < hurt_r {
                let amount = EXPLOSION_DAMAGE * (1.0 - d / hurt_r);
                self.events.push(MobEvent::PlayerDamaged {
                    amount,
                    source: MobKind::Creeper,
                    from: centre,
                });
            }
            // Other mobs caught in the blast take it too.
            let casualties: Vec<(u32, f32)> = self
                .mobs
                .iter()
                .filter(|m| m.id != id)
                .filter_map(|m| {
                    let dd = (m.centre() - centre).length();
                    (dd < hurt_r).then_some((m.id, EXPLOSION_DAMAGE * (1.0 - dd / hurt_r)))
                })
                .collect();
            for (vid, amount) in casualties {
                self.damage(vid, amount, Vec3::ZERO);
            }

            self.mobs.retain(|m| m.id != id);
            self.projectiles.retain(|p| p.owner != id);
        }
    }

    // --- projectiles ---

    fn tick_projectiles<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &W,
        player: &PlayerState,
        dt: f32,
    ) {
        let (pmin, pmax) = aabb(player.pos, Vec3::new(0.6, 1.8, 0.6));
        let pad = Vec3::splat(ARROW_HIT_PAD);
        let (pmin, pmax) = (pmin - pad, pmax + pad);

        let mut hits: Vec<(f32, Vec3)> = Vec::new();
        self.projectiles.retain_mut(|a| {
            a.ttl -= dt;
            if a.ttl <= 0.0 {
                return false;
            }
            a.vel.y += ARROW_GRAVITY * dt;
            let next = a.pos + a.vel * dt;
            if player.alive && next.cmpge(pmin).all() && next.cmple(pmax).all() {
                hits.push((a.damage, a.pos));
                return false;
            }
            if pathfind::cell_blocked(world, next.floor().as_ivec3()) {
                return false;
            }
            a.pos = next;
            true
        });
        for (amount, from) in hits {
            self.events.push(MobEvent::PlayerDamaged {
                amount,
                source: MobKind::Skeleton,
                from,
            });
        }
    }

    // --- spawning ---

    fn spawn_wave<W: VoxelWorld + ?Sized>(
        &mut self,
        world: &W,
        player: &PlayerState,
        daylight: f32,
        dt: f32,
    ) {
        if !self.spawning_enabled {
            return;
        }
        self.spawn_timer -= dt;
        if self.spawn_timer > 0.0 {
            return;
        }
        self.spawn_timer = SPAWN_INTERVAL;

        let want_hostile = self.hostile_count() < HOSTILE_CAP;
        let want_passive = self.passive_count() < PASSIVE_CAP;
        if !want_hostile && !want_passive {
            return;
        }

        for _ in 0..SPAWN_ATTEMPTS_PER_WAVE {
            let a = self.rng.random_range(0.0f32..std::f32::consts::TAU);
            let r = self
                .rng
                .random_range(SPAWN_MIN_DISTANCE..SPAWN_MAX_DISTANCE);
            let dy = self
                .rng
                .random_range(-SPAWN_VERTICAL_SPREAD..=SPAWN_VERTICAL_SPREAD);
            let probe = IVec3::new(
                (player.pos.x + a.cos() * r).floor() as i32,
                player.pos.y.floor() as i32 + dy,
                (player.pos.z + a.sin() * r).floor() as i32,
            );
            let Some(cell) = pathfind::snap_to_ground(world, probe) else {
                continue;
            };
            let pos = cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
            if (pos - player.pos).length() < SPAWN_MIN_DISTANCE {
                continue;
            }
            let dark = is_dark(world, cell, daylight);

            if dark && want_hostile && self.hostile_count() < HOSTILE_CAP {
                let kind = MobKind::HOSTILES[self.rng.random_range(0..MobKind::HOSTILES.len())];
                self.spawn(kind, pos);
                return;
            }
            if !dark && want_passive && self.passive_count() < PASSIVE_CAP {
                self.spawn(MobKind::Pig, pos);
                return;
            }
        }
    }
}

/// Dark enough for a hostile to spawn: buried under rock, or out under a night sky.
pub fn is_dark<W: VoxelWorld + ?Sized>(world: &W, feet: IVec3, daylight: f32) -> bool {
    if sky_covered(world, feet) {
        return true;
    }
    daylight < NIGHT_DAYLIGHT
}

/// Whether anything opaque stands between this cell and the sky, within a bounded scan.
pub fn sky_covered<W: VoxelWorld + ?Sized>(world: &W, feet: IVec3) -> bool {
    let top = (feet.y + SKY_SCAN_LIMIT).min(pathfind::WORLD_MAX_Y - 1);
    for y in feet.y + 1..=top {
        if world.block_at(feet.x, y, feet.z).is_opaque() {
            return true;
        }
    }
    false
}

fn waypoint_centre(c: IVec3) -> Vec3 {
    c.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
}

fn accelerate(mob: &mut Mob, want: Vec3, dt: f32) {
    let rate = if mob.on_ground {
        GROUND_ACCEL
    } else {
        AIR_ACCEL
    };
    let blend = (rate * dt).min(1.0);
    mob.vel.x += (want.x - mob.vel.x) * blend;
    mob.vel.z += (want.z - mob.vel.z) * blend;
}

fn decelerate(mob: &mut Mob, dt: f32) {
    accelerate(mob, Vec3::ZERO, dt);
}

fn face(mob: &mut Mob, dir: Vec3) {
    if dir.x.abs() + dir.z.abs() > 1.0e-4 {
        mob.yaw = dir.z.atan2(dir.x);
    }
}

/// Lead an arrow so it arcs onto the target rather than dropping short.
fn aim_arrow(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    let flat = horiz(d).length().max(0.001);
    let t = flat / ARROW_SPEED;
    // Compensate the drop the arrow will take over the flight time.
    let rise = d.y - 0.5 * ARROW_GRAVITY * t * t;
    Vec3::new(d.x, 0.0, d.z).normalize_or_zero() * ARROW_SPEED
        + Vec3::Y * (rise / t).clamp(-ARROW_SPEED, ARROW_SPEED)
}

/// Slab-method ray/AABB. Returns the entry distance if the ray hits going forward.
fn ray_aabb(origin: Vec3, dir: Vec3, mn: Vec3, mx: Vec3) -> Option<f32> {
    let inv = Vec3::ONE / dir;
    let t0 = (mn - origin) * inv;
    let t1 = (mx - origin) * inv;
    let near = t0.min(t1);
    let far = t0.max(t1);
    let t_near = near.max_element();
    let t_far = far.min_element();
    if t_far < t_near.max(0.0) {
        None
    } else {
        Some(t_near.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockId;
    use crate::sound::mock::MockWorld;
    use crate::sound::{self, LOUDNESS_CHIP, LOUDNESS_SMASH};

    const GROUND: i32 = 64;
    const DT: f32 = 1.0 / 60.0;

    fn flat() -> MockWorld {
        MockWorld::flat_ground(80, GROUND)
    }

    fn stand(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, (GROUND + 1) as f32, z)
    }

    fn quiet_manager() -> MobManager {
        let mut m = MobManager::new(7);
        m.spawning_enabled = false;
        m
    }

    /// Run `secs` of simulation.
    fn run<W: VoxelWorld + ?Sized>(
        mgr: &mut MobManager,
        world: &mut W,
        sound: &mut SoundField,
        player: &PlayerState,
        secs: f32,
    ) -> Vec<MobEvent> {
        let mut out = Vec::new();
        let steps = (secs / DT).round() as i32;
        for _ in 0..steps {
            sound.update(world, DT);
            out.extend(mgr.update(world, sound, player, 0.0, DT));
        }
        out
    }

    // ---- the headline behaviour ----

    #[test]
    fn a_mob_walks_to_the_sound_not_to_the_player() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();

        // Player far away in -X, well outside the zombie's 16-block sight.
        let player = PlayerState::new(stand(-40.0, 0.0), 1.62);
        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
        // Noise 12 blocks away in +X: the opposite direction from the player.
        let noise = stand(12.0, 0.0);
        sound.emit(NoiseEvent::smash(noise));

        run(&mut mgr, &mut w, &mut sound, &player, 5.0);

        let m = mgr.get(id).unwrap();
        assert_eq!(
            m.state,
            MobState::Investigate,
            "should be investigating: {:?}",
            m.state
        );
        assert!(
            m.pos.x > 4.0,
            "should have moved toward the noise, is at {:?}",
            m.pos
        );
        assert!(
            (m.target - noise).length() < 1.5,
            "the investigate target must be the SOUND position {noise:?}, got {:?}",
            m.target
        );
    }

    #[test]
    fn investigation_ignores_a_moving_player() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();

        let noise = stand(14.0, 0.0);
        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
        sound.emit(NoiseEvent::smash(noise));

        // The player runs away in -Z while the mob investigates.
        let mut z = -40.0f32;
        for _ in 0..300 {
            z -= 0.05;
            let player = PlayerState::new(stand(-40.0, z), 1.62);
            sound.update(&w, DT);
            mgr.update(&mut w, &mut sound, &player, 0.0, DT);
        }
        let m = mgr.get(id).unwrap();
        assert!(
            (m.target - noise).length() < 1.5,
            "target must still be the sound, not the player: {:?}",
            m.target
        );
        assert!(
            m.pos.z.abs() < 3.0,
            "must not have chased the player in -Z: {:?}",
            m.pos
        );
    }

    #[test]
    fn a_quiet_chip_does_not_alert_a_distant_mob_but_a_smash_does() {
        let dist = 16.0;
        let player = PlayerState::new(stand(-40.0, 0.0), 1.62);

        let alerted = |loudness: f32| {
            let mut w = flat();
            let mut sound = SoundField::new();
            let mut mgr = quiet_manager();
            let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
            sound.emit(NoiseEvent {
                pos: stand(dist, 0.0),
                loudness,
            });
            run(&mut mgr, &mut w, &mut sound, &player, 1.5);
            mgr.get(id).unwrap().state == MobState::Investigate
        };

        assert!(
            !alerted(LOUDNESS_CHIP),
            "a chip must not carry {dist} blocks"
        );
        assert!(alerted(LOUDNESS_SMASH), "a smash must carry {dist} blocks");
    }

    #[test]
    fn silence_makes_a_mob_lose_interest_and_wander_off() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(-40.0, 0.0), 1.62);

        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
        sound.emit(NoiseEvent::smash(stand(6.0, 0.0)));
        run(&mut mgr, &mut w, &mut sound, &player, 1.0);
        assert_eq!(mgr.get(id).unwrap().state, MobState::Investigate);

        // Now stay perfectly quiet.
        run(&mut mgr, &mut w, &mut sound, &player, 20.0);
        let m = mgr.get(id).unwrap();
        assert!(
            matches!(m.state, MobState::Idle | MobState::Wander),
            "must have given up, is {:?}",
            m.state
        );
    }

    #[test]
    fn a_mob_that_can_see_the_player_chases_the_live_position() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(8.0, 0.0), 1.62);
        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));

        run(&mut mgr, &mut w, &mut sound, &player, 2.0);
        let m = mgr.get(id).unwrap();
        assert!(
            matches!(m.state, MobState::Chase | MobState::Attack),
            "should have seen the player: {:?}",
            m.state
        );
        assert!(
            m.pos.x > 2.0,
            "should have closed the distance: {:?}",
            m.pos
        );
    }

    #[test]
    fn a_wall_blocks_sight_so_a_silent_player_is_not_found() {
        let mut w = flat();
        // Solid stone wall between mob and player.
        w.fill((4, GROUND + 1, -20), (5, GROUND + 6, 20), BlockId::STONE);
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(8.0, 0.0), 1.62);
        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));

        run(&mut mgr, &mut w, &mut sound, &player, 3.0);
        let m = mgr.get(id).unwrap();
        assert!(
            matches!(m.state, MobState::Idle | MobState::Wander),
            "cannot see through rock, should be idle: {:?}",
            m.state
        );
    }

    // ---- combat ----

    #[test]
    fn melee_deals_contact_damage_on_a_cooldown() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(1.0, 0.0), 1.62);
        mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));

        let events = run(&mut mgr, &mut w, &mut sound, &player, 5.0);
        let hits: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    MobEvent::PlayerDamaged {
                        source: MobKind::Zombie,
                        ..
                    }
                )
            })
            .collect();
        let expected = (5.0 / ZOMBIE_STATS.attack_cooldown) as usize;
        assert!(
            hits.len() >= expected - 1 && hits.len() <= expected + 1,
            "expected about {expected} hits in 5s, got {}",
            hits.len()
        );
    }

    #[test]
    fn a_skeleton_shoots_arrows_at_range() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(10.0, 0.0), 1.62);
        mgr.spawn(MobKind::Skeleton, stand(0.0, 0.0));

        let mut fired = false;
        for _ in 0..120 {
            sound.update(&w, DT);
            mgr.update(&mut w, &mut sound, &player, 0.0, DT);
            fired |= !mgr.projectiles().is_empty();
        }
        assert!(fired, "a skeleton with line of sight should shoot");
    }

    #[test]
    fn killing_a_pig_drops_meat() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(20.0, 20.0), 1.62);
        let id = mgr.spawn(MobKind::Pig, stand(0.0, 0.0));

        assert!(mgr.damage(id, 100.0, Vec3::ZERO), "should have died");
        let events = run(&mut mgr, &mut w, &mut sound, &player, DT);
        assert!(
            events.iter().any(|e| matches!(
                e,
                MobEvent::Drop {
                    kind: DropKind::Meat,
                    ..
                }
            )),
            "a pig must drop meat: {events:?}"
        );
        assert!(mgr.get(id).is_none());
    }

    #[test]
    fn the_player_can_shoot_a_mob_with_a_ray() {
        let mut mgr = quiet_manager();
        let id = mgr.spawn(MobKind::Zombie, stand(6.0, 0.0));
        let hit = mgr.raycast(stand(0.0, 0.0) + Vec3::Y, Vec3::X, 10.0);
        assert_eq!(hit.map(|(i, _)| i), Some(id));
        assert!(
            mgr.raycast(stand(0.0, 0.0) + Vec3::Y, -Vec3::X, 10.0)
                .is_none()
        );
    }

    // ---- the creeper: where both headline mechanics meet ----

    #[test]
    fn a_creeper_explosion_leaves_a_real_crater() {
        let mut w = MockWorld::solid_stone();
        let centre = Vec3::new(8.5, 64.5, 8.5);
        let report = explode(&mut w, centre, EXPLOSION_RADIUS);

        // The centre block is gone entirely.
        assert!(
            w.block_at(8, 64, 8).is_air(),
            "the centre block must be destroyed"
        );
        assert_eq!(w.fill_ratio(8, 64, 8), 0.0);

        // There is a rim of blocks that survived but are chewed.
        let mut damaged = 0;
        let mut destroyed = 0;
        let reach = EXPLOSION_RADIUS.ceil() as i32 + 1;
        for x in 8 - reach..=8 + reach {
            for y in 64 - reach..=64 + reach {
                for z in 8 - reach..=8 + reach {
                    let f = w.fill_ratio(x, y, z);
                    if f == 0.0 {
                        destroyed += 1;
                    } else if f < 1.0 {
                        damaged += 1;
                    }
                }
            }
        }
        assert!(
            destroyed > 20,
            "the blast should hollow out a core, got {destroyed}"
        );
        assert!(
            damaged > 20,
            "the rim must be damaged, not deleted, got {damaged}"
        );
        assert_eq!(
            destroyed, report.blocks_destroyed,
            "report should match the world"
        );
        assert_eq!(damaged, report.blocks_damaged);

        // Well outside the blast nothing is touched at all.
        assert_eq!(w.fill_ratio(8 + reach + 2, 64, 8), 1.0);
        assert_eq!(w.fill_ratio(8, 64 - reach - 2, 8), 1.0);
    }

    #[test]
    fn the_crater_rim_is_ragged_not_a_clean_sphere() {
        let mut w = MockWorld::solid_stone();
        explode(&mut w, Vec3::new(0.5, 64.5, 0.5), EXPLOSION_RADIUS);
        // Blocks at the same distance should not all be damaged identically.
        let ratios: Vec<f32> = [(3, 64, 0), (-3, 64, 0), (0, 64, 3), (0, 64, -3), (0, 67, 0)]
            .iter()
            .map(|&(x, y, z)| w.fill_ratio(x, y, z))
            .collect();
        let first = ratios[0];
        assert!(
            ratios.iter().any(|r| (r - first).abs() > 1.0e-3),
            "the rim should vary between blocks: {ratios:?}"
        );
    }

    #[test]
    fn a_creeper_fuses_then_detonates_and_hurts_the_player() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(1.5, 0.0), 1.62);
        let id = mgr.spawn(MobKind::Creeper, stand(0.0, 0.0));

        // One tick to spot the player and start fusing.
        run(&mut mgr, &mut w, &mut sound, &player, 0.2);
        assert_eq!(
            mgr.get(id).unwrap().state,
            MobState::Fuse,
            "should be priming"
        );
        assert!(mgr.get(id).unwrap().fuse_fraction() > 0.0);

        let events = run(&mut mgr, &mut w, &mut sound, &player, FUSE_TIME + 0.3);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MobEvent::Exploded { .. })),
            "the fuse must run out: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                MobEvent::PlayerDamaged {
                    source: MobKind::Creeper,
                    ..
                }
            )),
            "the blast must hurt a player standing next to it"
        );
        assert!(
            mgr.get(id).is_none(),
            "the creeper is consumed by its own blast"
        );
        // And it left a hole in the floor.
        assert!(
            w.fill_ratio(0, GROUND, 0) < 1.0,
            "the ground under it should be cratered"
        );
    }

    #[test]
    fn a_creeper_aborts_its_fuse_if_the_player_retreats() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let id = mgr.spawn(MobKind::Creeper, stand(0.0, 0.0));

        let near = PlayerState::new(stand(1.5, 0.0), 1.62);
        run(&mut mgr, &mut w, &mut sound, &near, 0.2);
        assert_eq!(mgr.get(id).unwrap().state, MobState::Fuse);

        let far = PlayerState::new(stand(30.0, 0.0), 1.62);
        run(&mut mgr, &mut w, &mut sound, &far, 0.2);
        assert_ne!(
            mgr.get(id).unwrap().state,
            MobState::Fuse,
            "should have stood down"
        );
    }

    #[test]
    fn an_explosion_is_heard_across_the_map() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(1.5, 0.0), 1.62);
        mgr.spawn(MobKind::Creeper, stand(0.0, 0.0));
        // A second zombie far away, out of sight, that should come running.
        let watcher = mgr.spawn(MobKind::Zombie, stand(14.0, 0.0));

        run(&mut mgr, &mut w, &mut sound, &player, FUSE_TIME + 0.5);
        let m = mgr.get(watcher).unwrap();
        assert!(
            m.state == MobState::Investigate || m.state == MobState::Chase,
            "a blast should draw mobs in: {:?}",
            m.state
        );
    }

    // ---- physics, spawning, housekeeping ----

    #[test]
    fn mobs_fall_and_land_on_the_ground() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(40.0, 40.0), 1.62);
        let id = mgr.spawn(MobKind::Pig, Vec3::new(0.0, (GROUND + 12) as f32, 0.0));

        run(&mut mgr, &mut w, &mut sound, &player, 3.0);
        let m = mgr.get(id).unwrap();
        assert!(m.on_ground, "should have landed");
        assert!(
            (m.pos.y - (GROUND + 1) as f32).abs() < 0.05,
            "should rest on the floor, is at {}",
            m.pos.y
        );
    }

    #[test]
    fn mobs_do_not_walk_through_walls() {
        let mut w = flat();
        w.fill((3, GROUND + 1, -20), (3, GROUND + 8, 20), BlockId::STONE);
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        let player = PlayerState::new(stand(-40.0, 0.0), 1.62);
        let id = mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
        sound.emit(NoiseEvent::smash(stand(8.0, 0.0)));

        run(&mut mgr, &mut w, &mut sound, &player, 6.0);
        assert!(
            mgr.get(id).unwrap().pos.x < 3.0,
            "walked through a solid wall"
        );
    }

    #[test]
    fn hostiles_respect_the_population_cap() {
        let mut w = MockWorld::solid_stone();
        // A big dark room to spawn into.
        w.fill((-60, GROUND, -60), (60, GROUND + 6, 60), BlockId::AIR);
        w.fill((-60, GROUND, -60), (60, GROUND, 60), BlockId::STONE);

        let mut sound = SoundField::new();
        let mut mgr = MobManager::new(11);
        let player = PlayerState::new(stand(0.0, 0.0), 1.62);
        run(&mut mgr, &mut w, &mut sound, &player, 200.0);
        assert!(
            mgr.hostile_count() <= HOSTILE_CAP,
            "cap broken: {} mobs",
            mgr.hostile_count()
        );
        assert!(
            mgr.hostile_count() > 0,
            "should have spawned something in the dark"
        );
    }

    #[test]
    fn hostiles_spawn_away_from_the_player() {
        let mut w = MockWorld::solid_stone();
        w.fill((-60, GROUND, -60), (60, GROUND + 6, 60), BlockId::AIR);
        w.fill((-60, GROUND, -60), (60, GROUND, 60), BlockId::STONE);
        let mut sound = SoundField::new();
        let mut mgr = MobManager::new(3);
        let player = PlayerState::new(stand(0.0, 0.0), 1.62);

        // One spawn wave only, then check where they appeared.
        run(&mut mgr, &mut w, &mut sound, &player, SPAWN_INTERVAL + DT);
        assert!(!mgr.is_empty(), "expected a spawn");
        for m in mgr.mobs() {
            assert!(
                (m.pos - player.pos).length() >= SPAWN_MIN_DISTANCE - 1.0,
                "spawned on top of the player at {:?}",
                m.pos
            );
        }
    }

    #[test]
    fn daylight_on_the_surface_suppresses_hostile_spawns() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = MobManager::new(5);
        let player = PlayerState::new(stand(0.0, 0.0), 1.62);
        for _ in 0..(60.0 / DT) as i32 {
            sound.update(&w, DT);
            mgr.update(&mut w, &mut sound, &player, 1.0, DT); // high noon
        }
        assert_eq!(mgr.hostile_count(), 0, "nothing hostile spawns in daylight");
        assert!(mgr.passive_count() > 0, "pigs should still appear");
    }

    #[test]
    fn distant_mobs_despawn() {
        let mut w = flat();
        let mut sound = SoundField::new();
        let mut mgr = quiet_manager();
        mgr.spawn(MobKind::Zombie, stand(0.0, 0.0));
        let player = PlayerState::new(stand(DESPAWN_DISTANCE + 10.0, 0.0), 1.62);
        run(&mut mgr, &mut w, &mut sound, &player, DT);
        assert!(mgr.is_empty(), "far mobs should be culled");
    }

    #[test]
    fn is_dark_distinguishes_a_cave_from_a_meadow() {
        let mut w = flat();
        assert!(
            !is_dark(&w, IVec3::new(0, GROUND + 1, 0), 1.0),
            "a sunlit meadow is not dark"
        );
        assert!(
            is_dark(&w, IVec3::new(0, GROUND + 1, 0), 0.0),
            "the same meadow at night is"
        );
        w.fill((-2, GROUND + 5, -2), (2, GROUND + 6, 2), BlockId::STONE);
        assert!(
            is_dark(&w, IVec3::new(0, GROUND + 1, 0), 1.0),
            "a roof makes it dark at noon"
        );
    }

    #[test]
    fn a_burst_of_noise_never_costs_more_than_the_flood_cap() {
        let mut w = flat();
        let mut sound = SoundField::new();
        for i in 0..500 {
            sound.emit(NoiseEvent::smash(stand(i as f32 * 3.0, 0.0)));
        }
        sound.update(&w, DT);
        assert!(sound.active_count() <= sound::MAX_FLOODS_PER_TICK);
        let _ = &mut w;
    }
}
