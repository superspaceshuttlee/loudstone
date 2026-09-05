//! Loudstone -- a voxel survival game.
//!
//! Two mechanics define it, and they are deliberately wired into each other:
//! blocks are internally 8^3 sub-voxel grids that can be chipped away a grain at
//! a time, and mining emits noise that hostile mobs hear and walk toward.
//! Chipping is slow and quiet. Smashing a whole block out is fast and loud.
//! Every mining decision is therefore a bet on speed against safety.

mod block;
mod camera;
mod chunk;
mod config;
mod crafting;
mod gauntlet;
mod gfx;
mod hud;
mod inventory;
mod item;
mod light;
mod mesh;
mod mob;
mod pathfind;
mod save;
mod screenshot;
mod sound;
mod world;
mod worldgen;

use block::BlockId;
use camera::{Camera, MoveInput, Player};
use chunk::ChunkPos;
use config::*;
use glam::Vec3;
use hud::Slot;
use inventory::ItemStack;
use item::ItemId;
use mob::{MobEvent, MobKind, MobManager, PlayerState};
use sound::SoundField;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};
use world::World;

// ---------------------------------------------------------------------------
// The two trait bridges. Both modules were written against narrow traits so
// they could be built in parallel without depending on the engine; these are
// where they meet the real world.
// ---------------------------------------------------------------------------

impl sound::VoxelWorld for World {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        World::block_at(self, x, y, z)
    }
    fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        World::sub_solid(self, x, y, z, sx, sy, sz)
    }
    fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32 {
        World::fill_ratio(self, x, y, z)
    }
    fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        World::carve(self, x, y, z, sx, sy, sz)
    }
    fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        World::set_block(self, x, y, z, id)
    }
}

impl save::WorldEdit for World {
    fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        self.set_block(x, y, z, id);
    }
    fn carve_at(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        self.carve(x, y, z, sx, sy, sz)
    }
}

// ---------------------------------------------------------------------------
// Day/night. Kept here rather than in config.rs because it is the only system
// that is purely presentation plus one number the mob spawner reads.
// ---------------------------------------------------------------------------

/// Seconds for one full day/night cycle.
const DAY_LENGTH: f32 = 600.0;
/// Fraction of the cycle spent in full daylight.
const DAY_FRACTION: f32 = 0.55;
const SKY_NIGHT: [f32; 3] = [0.03, 0.04, 0.08];

/// 1.0 at noon, 0.0 at midnight, with dawn and dusk ramps between.
fn daylight_at(time_of_day: f32) -> f32 {
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

fn sky_for(daylight: f32) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = SKY_NIGHT[i] + (SKY_COLOR[i] - SKY_NIGHT[i]) * daylight;
    }
    out
}

// ---------------------------------------------------------------------------

/// Which on-screen panel has focus. The cursor is only released for a panel.
#[derive(PartialEq, Copy, Clone)]
enum Ui {
    Playing,
    /// The player's own 2x2 grid.
    Inventory,
    /// A crafting table: the full 3x3, which is what every tool needs.
    Table,
    /// A specific furnace block in the world.
    Furnace((i32, i32, i32)),
}

impl Ui {
    fn is_panel(self) -> bool {
        self != Ui::Playing
    }
    /// How many crafting cells this panel exposes. Tools are 3x3 recipes, so a
    /// 2x2 grid can only ever make planks, sticks, torches and the table itself.
    fn craft_cells(self) -> usize {
        match self {
            Ui::Table => 9,
            _ => 4,
        }
    }
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<gfx::Renderer>,
    world: World,
    camera: Camera,
    player: Player,
    input: MoveInput,
    mobs: MobManager,
    sound: SoundField,

    /// Seed, player record, inventory and the durable edit log. This IS the save.
    data: save::SaveData,
    save_path: std::path::PathBuf,
    time_since_save: f32,

    /// Chunks whose saved edits have already been replayed since becoming
    /// resident. Cleared for a chunk when it streams out.
    replayed: HashSet<ChunkPos>,

    ui: Ui,
    craft_grid: [Option<ItemStack>; 9],
    /// Furnace contents, keyed by the block they belong to, so two furnaces do
    /// not share one inventory.
    furnaces: std::collections::HashMap<(i32, i32, i32), crafting::Furnace>,
    /// The stack held by the cursor in the inventory screen.
    carried: Option<ItemStack>,
    cursor: (f32, f32),

    cursor_locked: bool,
    mining: bool,
    placing: bool,
    /// Left Alt: the loud, fast, whole-block break.
    smash_mode: bool,
    chip_timer: f32,
    place_timer: f32,
    attack_timer: f32,
    /// Set by `--shot <path>`: capture one frame once the world is loaded, then quit.
    shot_path: Option<std::path::PathBuf>,
    /// `--demo`: carve a crater and spawn one of each mob before capturing, so
    /// the two headline mechanics can be verified in a still frame.
    demo: bool,
    /// `--ui table` / `--ui furnace`: open that panel before capturing.
    ui_demo: Option<String>,
    /// `--gauntlet`: a robot plays the game and reports what broke.
    gauntlet: Option<gauntlet::Gauntlet>,
    /// Multiplier on the day/night clock, driven by the gauntlet.
    time_scale: f32,
    // Monotonic counters the gauntlet watches to tell whether anything happened.
    stat_carved: u64,
    stat_broken: u64,
    stat_placed: u64,
    shot_countdown: i32,

    time_of_day: f32,
    spawn: Vec3,
    last_frame: Instant,
    fps: f32,
    fps_accum: f32,
    fps_frames: u32,
    /// Set while the streamer is still filling the initial radius.
    loading: bool,
    gauntlet_done: bool,
    gauntlet_stocked: bool,
    load_frames: u32,
    start: Instant,
}

impl App {
    fn new() -> Self {
        let save_path = save::default_save_path();
        let data = if save::save_exists(&save_path) {
            match save::load_from_file(&save_path) {
                Ok(d) => {
                    println!("[loudstone] loaded save (seed {})", d.seed);
                    d
                }
                Err(e) => {
                    eprintln!("[loudstone] could not load save: {e} -- starting fresh");
                    save::SaveData::new(1337)
                }
            }
        } else {
            save::SaveData::new(1337)
        };

        let world = World::new(data.seed);
        let ground = world.surface_y(0, 0) as f32 + 1.0;
        let spawn = Vec3::new(0.5, ground, 0.5);
        let start_pos = if data.player.pos == Vec3::ZERO {
            spawn
        } else {
            data.player.pos
        };

        let mut camera = Camera::new(start_pos + Vec3::Y * PLAYER_EYE_HEIGHT);
        camera.yaw = data.player.yaw;
        camera.pitch = data.player.pitch;

        let mut player = Player::new(start_pos);
        player.noclip = false;

        Self {
            window: None,
            gfx: None,
            world,
            camera,
            player,
            input: MoveInput::default(),
            mobs: MobManager::new(data.seed as u64 ^ 0x9E37_79B9),
            sound: SoundField::new(),
            data,
            save_path,
            time_since_save: 0.0,
            replayed: HashSet::new(),
            ui: Ui::Playing,
            craft_grid: [None; 9],
            furnaces: std::collections::HashMap::new(),
            carried: None,
            cursor: (0.0, 0.0),
            cursor_locked: false,
            mining: false,
            placing: false,
            smash_mode: false,
            chip_timer: 0.0,
            place_timer: 0.0,
            attack_timer: 0.0,
            shot_path: std::env::args()
                .skip_while(|a| a != "--shot")
                .nth(1)
                .map(std::path::PathBuf::from),
            demo: std::env::args().any(|a| a == "--demo"),
            ui_demo: std::env::args()
                .skip_while(|a| a != "--ui")
                .nth(1),
            gauntlet: std::env::args()
                .any(|a| a == "--gauntlet")
                .then(gauntlet::Gauntlet::new),
            time_scale: 1.0,
            stat_carved: 0,
            stat_broken: 0,
            stat_placed: 0,
            shot_countdown: 90,
            time_of_day: 0.0,
            spawn,
            last_frame: Instant::now(),
            fps: 0.0,
            fps_accum: 0.0,
            fps_frames: 0,
            loading: true,
            gauntlet_done: false,
            gauntlet_stocked: false,
            load_frames: 0,
            start: Instant::now(),
        }
    }

    fn camera_chunk(&self) -> ChunkPos {
        ChunkPos::new(
            (self.camera.pos.x.floor() as i32).div_euclid(CHUNK_SIZE_I),
            (self.camera.pos.y.floor() as i32).div_euclid(CHUNK_SIZE_I),
            (self.camera.pos.z.floor() as i32).div_euclid(CHUNK_SIZE_I),
        )
    }

    fn set_cursor_locked(&mut self, locked: bool) {
        let Some(window) = &self.window else { return };
        if locked {
            // Locked is unsupported on Windows; Confined plus raw mouse deltas
            // is the same thing in practice.
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.cursor_locked = locked;
    }

    fn save_now(&mut self) {
        self.data.player.pos = self.player.pos;
        self.data.player.yaw = self.camera.yaw;
        self.data.player.pitch = self.camera.pitch;
        match save::save_to_file(&self.save_path, &self.data) {
            Ok(()) => self.time_since_save = 0.0,
            Err(e) => eprintln!("[loudstone] save failed: {e}"),
        }
    }

    /// Mining, placing, and the noise both of them make.
    fn interact(&mut self, dt: f32) {
        self.chip_timer -= dt;
        self.place_timer -= dt;
        self.attack_timer -= dt;

        let hit = self
            .world
            .raycast(self.camera.pos, self.camera.forward(), REACH);

        if let Some(gfx) = self.gfx.as_mut() {
            gfx.set_highlight(hit.map(|h| h.block));
        }

        // A swing hits a mob before it touches the world behind it. Mining a
        // block the mob is standing in front of would otherwise be impossible.
        if self.mining && self.attack_timer <= 0.0 {
            let block_dist = hit.map(|h| h.distance).unwrap_or(REACH);
            if let Some((id, mob_pos, dist)) = self.mob_under_crosshair(REACH) {
                if dist <= block_dist {
                    self.attack_timer = ATTACK_INTERVAL;
                    let damage = self
                        .data
                        .inventory
                        .selected_item()
                        .map(|i| i.attack_damage())
                        .unwrap_or(1.0);
                    let knock = (mob_pos - self.camera.pos).normalize_or_zero() * KNOCKBACK
                        + Vec3::Y * KNOCKBACK_LIFT;
                    self.mobs.damage(id, damage, knock);
                    self.data.inventory.damage_selected(1);
                    // Swinging is loud: a fight is not a quiet way to spend time.
                    self.sound.emit(sound::NoiseEvent {
                        pos: self.camera.pos,
                        loudness: NOISE_ATTACK,
                    });
                    return;
                }
            }
        }

        let Some(hit) = hit else { return };
        let (bx, by, bz) = hit.block;

        if self.mining {
            let target = self.world.block_at(bx, by, bz);
            if target.hardness().is_finite() && !target.is_air() {
                let tool = self.data.inventory.selected_item();

                if self.smash_mode {
                    // Loud and fast: the whole block leaves at once. Everything
                    // within earshot hears it.
                    if self.chip_timer <= 0.0 {
                        self.chip_timer = SMASH_CHARGE;
                        if let Some(broken) = self.world.smash_block(bx, by, bz) {
                            self.stat_broken += 1;
                            self.data.edits.note_set_block(bx, by, bz, BlockId::AIR);
                            self.grant_drop(tool, broken);
                        }
                    }
                } else if self.chip_timer <= 0.0 {
                    // Quiet and slow: carve a small sphere of sub-voxels. Hard
                    // blocks take proportionally longer per bite.
                    let speed = item::mining_speed_multiplier(tool, target);
                    self.chip_timer = CHIP_INTERVAL * target.hardness() / speed.max(0.01);

                    let before = self.world.block_at(bx, by, bz);
                    self.stat_carved += self.world.chip_sphere(&hit, CHIP_RADIUS) as u64;
                    // Record what was carved so it survives streaming and saving.
                    self.note_carves(&hit);
                    if self.world.block_at(bx, by, bz).is_air() && !before.is_air() {
                        self.grant_drop(tool, before);
                    }
                }
            }
        }

        if self.placing && self.place_timer <= 0.0 {
            // Right-clicking a workstation opens it rather than placing against it.
            let aimed = self.world.block_at(bx, by, bz);
            if aimed == BlockId::CRAFTING_TABLE || aimed == BlockId::FURNACE {
                self.place_timer = PLACE_INTERVAL;
                self.placing = false;
                self.mining = false;
                self.ui = if aimed == BlockId::FURNACE {
                    self.furnaces.entry((bx, by, bz)).or_default();
                    Ui::Furnace((bx, by, bz))
                } else {
                    Ui::Table
                };
                self.set_cursor_locked(false);
                return;
            }

            let Some(stack) = self.data.inventory.selected_stack() else {
                return;
            };
            let Some(id) = stack.item.places() else { return };
            let (px, py, pz) = hit.adjacent();
            let (min, max) = self.player.aabb();
            if self.world.place_block(px, py, pz, id, min, max) {
                self.stat_placed += 1;
                self.data.edits.note_set_block(px, py, pz, id);
                let sel = self.data.inventory.selected();
                self.data.inventory.take_from_slot(sel, 1);
                self.place_timer = PLACE_INTERVAL;
            }
        }
    }

    /// Leave whatever panel is open, returning everything on the cursor and in
    /// the crafting grid to the inventory. Closing a menu must never eat items.
    fn close_panel(&mut self) {
        if let Some(st) = self.carried.take() {
            self.data.inventory.add_item(st.item, st.count as u32);
        }
        for cell in self.craft_grid.iter_mut() {
            if let Some(st) = cell.take() {
                self.data.inventory.add_item(st.item, st.count as u32);
            }
        }
        self.ui = Ui::Playing;
        self.set_cursor_locked(true);
    }

    /// Stage the two headline mechanics in front of the camera so a single
    /// captured frame shows both: a chipped crater, and mobs standing near it.
    fn run_demo(&mut self) {
        let eye = self.camera.pos;
        let fwd = Vec3::new(self.camera.yaw.cos(), 0.0, self.camera.yaw.sin());

        // Carve a hemisphere out of the ground a few blocks ahead.
        let target = eye + fwd * 7.0;
        let gy = self.world.surface_y(target.x as i32, target.z as i32) as f32;
        let centre = Vec3::new(target.x, gy + 0.5, target.z);
        let r = 2.6f32;
        let span = r.ceil() as i32 + 1;
        let (bx, by, bz) = (
            centre.x.floor() as i32,
            centre.y.floor() as i32,
            centre.z.floor() as i32,
        );
        for dy in -span..=span {
            for dz in -span..=span {
                for dx in -span..=span {
                    let (x, y, z) = (bx + dx, by + dy, bz + dz);
                    for sy in 0..SUBVOX {
                        for sz in 0..SUBVOX {
                            for sx in 0..SUBVOX {
                                let p = Vec3::new(
                                    x as f32 + (sx as f32 + 0.5) / SUBVOX_F,
                                    y as f32 + (sy as f32 + 0.5) / SUBVOX_F,
                                    z as f32 + (sz as f32 + 0.5) / SUBVOX_F,
                                );
                                if (p - centre).length() <= r {
                                    self.world.carve(x, y, z, sx, sy, sz);
                                }
                            }
                        }
                    }
                }
            }
        }

        // One of each mob, arranged across the view.
        for (i, kind) in MobKind::ALL.iter().enumerate() {
            let off = (i as f32 - 1.5) * 2.4;
            let side = Vec3::new(-fwd.z, 0.0, fwd.x) * off;
            let p = eye + fwd * 11.0 + side;
            let y = self.world.surface_y(p.x as i32, p.z as i32) as f32 + 1.0;
            self.mobs.spawn(*kind, Vec3::new(p.x, y, p.z));
        }
        println!("[loudstone] demo: crater carved, 4 mobs spawned");
    }

    /// Sample the game for the gauntlet, hand it the frame, and apply whatever
    /// it decides to do. Returns a screenshot name when a step asks for one.
    fn drive_gauntlet(&mut self, dt: f32) -> Option<&'static str> {
        if self.gauntlet.is_none() {
            return None;
        }
        // Still streaming: hold the robot at the gate so a slow first load is
        // not mistaken for the player refusing to move.
        if self.loading {
            return None;
        }
        if !self.gauntlet_stocked {
            // A robot with empty pockets cannot test placing, and an empty hand
            // mines at the slowest possible rate.
            self.gauntlet_stocked = true;
            self.data.inventory.add_item(ItemId::COBBLESTONE, 64);
            self.data.inventory.add_item(ItemId::TORCH, 16);
            self.data.inventory.add_item(ItemId::STONE_PICKAXE, 1);
        }

        let (min, max) = self.player.aabb();
        let nearest_hostile = self
            .mobs
            .mobs()
            .iter()
            .filter(|m| m.kind.is_hostile())
            .map(|m| (m.pos - self.player.pos).length())
            .fold(f32::INFINITY, f32::min);

        let probe = gauntlet::Probe {
            pos: self.player.pos,
            on_ground: self.player.on_ground,
            health: self.data.player.health,
            fps: self.fps,
            chunks: self.world.chunks.len(),
            mobs: self.mobs.mobs().len(),
            inside_solid: self.world.box_collides(min, max),
            daylight: daylight_at(self.time_of_day),
            nearest_hostile: nearest_hostile.is_finite().then_some(nearest_hostile),
            carved: self.stat_carved,
            broken: self.stat_broken,
            placed: self.stat_placed,
        };

        let g = self.gauntlet.as_mut().unwrap();
        let frame = g.tick(dt, &probe);

        let Some(frame) = frame else {
            // Finished, or aborted. Report and quit.
            let g = self.gauntlet.take().unwrap();
            println!("\n[gauntlet] ---- report ----");
            if g.findings.is_empty() {
                println!("[gauntlet] {} steps, no findings", g.total_steps());
            } else {
                println!("[gauntlet] {} findings:", g.findings.len());
                for f in &g.findings {
                    println!("[gauntlet]   {}: {}", f.step, f.what);
                }
            }
            self.gauntlet_done = true;
            return None;
        };

        self.input = frame.input;
        self.mining = frame.mine;
        self.smash_mode = frame.smash;
        self.placing = frame.place;
        self.time_scale = frame.time_scale;
        self.camera.yaw += frame.yaw_rate * dt;
        if let Some(pitch) = frame.look_pitch {
            self.camera.pitch = pitch;
        }
        if let Some(frac) = frame.set_time_frac {
            self.time_of_day = frac * DAY_LENGTH;
        }

        for i in 0..frame.spawn_hostiles {
            let a = i as f32 * std::f32::consts::TAU / frame.spawn_hostiles.max(1) as f32;
            let d = 9.0;
            let p = self.player.pos + Vec3::new(a.cos() * d, 0.0, a.sin() * d);
            let y = self.world.surface_y(p.x as i32, p.z as i32) as f32 + 1.0;
            let kind = MobKind::HOSTILES[i % MobKind::HOSTILES.len()];
            self.mobs.spawn(kind, Vec3::new(p.x, y, p.z));
        }
        frame.shot
    }

    /// Stock the inventory and open a panel, so the crafting and furnace screens
    /// can be checked in a captured frame.
    fn run_ui_demo(&mut self, which: &str) {
        for (item, n) in [
            (ItemId::COBBLESTONE, 32u32),
            (ItemId::PLANKS, 12),
            (ItemId::STICK, 8),
            (ItemId::COAL, 6),
            (ItemId::RAW_IRON, 4),
            (ItemId::TORCH, 16),
            (ItemId::IRON_PICKAXE, 1),
        ] {
            self.data.inventory.add_item(item, n);
        }

        if which == "furnace" {
            let key = (0, 0, 0);
            let mut f = crafting::Furnace::new();
            f.input = Some(ItemStack::new(ItemId::RAW_IRON, 3));
            f.fuel = Some(ItemStack::new(ItemId::COAL, 2));
            // Run it far enough to light the fire and part-fill the bar.
            for _ in 0..300 {
                f.tick(1.0 / 60.0);
            }
            self.furnaces.insert(key, f);
            self.ui = Ui::Furnace(key);
        } else {
            // A stone pickaxe laid out in the 3x3, so the output slot is filled.
            self.craft_grid = [None; 9];
            for c in self.craft_grid.iter_mut().take(3) {
                *c = Some(ItemStack::new(ItemId::COBBLESTONE, 1));
            }
            self.craft_grid[4] = Some(ItemStack::new(ItemId::STICK, 1));
            self.craft_grid[7] = Some(ItemStack::new(ItemId::STICK, 1));
            self.ui = Ui::Table;
        }
        self.cursor = (640.0, 360.0);
        println!("[loudstone] demo: {which} panel open");
    }

    /// Nearest mob whose box the aim ray enters, within `reach`.
    fn mob_under_crosshair(&self, reach: f32) -> Option<(u32, Vec3, f32)> {
        let origin = self.camera.pos;
        let dir = self.camera.forward();
        let mut best: Option<(u32, Vec3, f32)> = None;
        for m in self.mobs.mobs() {
            let size = m.kind.size();
            let half = size.x * 0.5;
            let min = Vec3::new(m.pos.x - half, m.pos.y, m.pos.z - half);
            let max = Vec3::new(m.pos.x + half, m.pos.y + size.y, m.pos.z + half);
            if let Some(t) = ray_box(origin, dir, min, max) {
                if t <= reach && best.map(|(_, _, bd)| t < bd).unwrap_or(true) {
                    best = Some((m.id, m.pos + Vec3::Y * size.y * 0.5, t));
                }
            }
        }
        best
    }

    /// Mirror the sub-voxels `chip_sphere` just cleared into the durable edit
    /// log. The world is the authority; this reads back what it decided.
    fn note_carves(&mut self, hit: &world::RayHit) {
        let (bx, by, bz) = hit.block;
        let r = CHIP_RADIUS.ceil() as i32;
        let (cx, cy, cz) = (
            hit.sub.0 as i32,
            hit.sub.1 as i32,
            hit.sub.2 as i32,
        );
        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    let (sx, sy, sz) = (cx + dx, cy + dy, cz + dz);
                    if !(0..SUBVOX_I).contains(&sx)
                        || !(0..SUBVOX_I).contains(&sy)
                        || !(0..SUBVOX_I).contains(&sz)
                    {
                        continue;
                    }
                    if !self
                        .world
                        .sub_solid(bx, by, bz, sx as usize, sy as usize, sz as usize)
                    {
                        self.data.edits.note_carve(
                            bx, by, bz, sx as usize, sy as usize, sz as usize,
                        );
                    }
                }
            }
        }
    }

    fn grant_drop(&mut self, tool: Option<ItemId>, broken: BlockId) {
        if let Some(dropped) = item::mining_drop(tool, broken) {
            self.data.inventory.add_item(dropped, 1);
        }
        self.data.inventory.damage_selected(1);
    }

    fn respawn(&mut self) {
        let _dropped = self.data.inventory.drop_all();
        self.data.player.health = 20.0;
        self.player.pos = self.spawn;
        self.player.vel = Vec3::ZERO;
        self.camera.pos = self.spawn + Vec3::Y * PLAYER_EYE_HEIGHT;
    }

    /// One simulation step.
    fn update(&mut self, dt: f32) {
        self.time_of_day += dt * self.time_scale;
        let daylight = daylight_at(self.time_of_day);
        // Sky light is baked into the chunk meshes, so the world has to be told
        // what time it is or night only changes the sky colour and the ground
        // stays lit as if at noon. This is cheap: it does nothing until the
        // daylight crosses one of twelve steps, then queues a background remesh.
        self.world.set_daylight(daylight);

        // --- movement, then the camera rides the player's eyes ---
        if self.ui == Ui::Playing {
            self.player.update(&self.world, &self.camera, &self.input, dt);
        }
        self.camera.pos = self.player.eye();

        // --- streaming ---
        let center = self.camera_chunk();
        let stream = self.world.stream(center);
        for pos in &stream.dropped {
            self.replayed.remove(pos);
        }
        // A freshly generated chunk knows nothing about what the player built
        // there, so saved edits are replayed the first time it becomes visible.
        let arrived: Vec<ChunkPos> = stream
            .ready
            .iter()
            .map(|m| m.pos)
            .filter(|p| !self.replayed.contains(p))
            .collect();
        if let Some(gfx) = self.gfx.as_mut() {
            gfx.apply_stream(&stream.dropped, stream.ready);
        }
        for pos in arrived {
            self.replayed.insert(pos);
            let edits = std::mem::take(&mut self.data.edits);
            edits.replay_chunk((pos.x, pos.y, pos.z), &mut self.world);
            self.data.edits = edits;
        }

        if self.loading {
            self.load_frames += 1;
        }
        if self.loading && self.world.is_idle() {
            self.loading = false;
            let st = self.world.stats;
            println!(
                "[loudstone] world ready in {:.2}s -- {} chunks resident, {} generated, {} meshed, over {} frames",
                self.start.elapsed().as_secs_f32(),
                self.world.chunks.len(),
                st.chunks_generated,
                st.chunks_meshed,
                self.load_frames
            );
            if let Some(g) = self.gfx.as_ref() {
                let idx = g.total_indices();
                println!(
                    "[loudstone] geometry: {:.1}M indices, {:.1}M triangles, {:.0} tris/chunk, ~{:.0} MB vertex data",
                    idx as f64 / 1.0e6,
                    idx as f64 / 3.0e6,
                    idx as f64 / 3.0 / g.loaded_mesh_count().max(1) as f64,
                    idx as f64 / 6.0 * 4.0 * 28.0 / 1.0e6,
                );
            }
        }

        if self.ui == Ui::Playing {
            self.interact(dt);
        }

        // --- sound, then the mobs that listen to it ---
        for ev in self.world.drain_noise() {
            self.sound.emit(sound::NoiseEvent {
                pos: ev.pos,
                loudness: ev.loudness,
            });
        }
        self.sound.update(&self.world, dt);

        let mut pstate = PlayerState::new(self.player.pos, PLAYER_EYE_HEIGHT);
        pstate.alive = self.data.player.health > 0.0;
        let events = self
            .mobs
            .update(&mut self.world, &mut self.sound, &pstate, daylight, dt);

        for ev in events {
            match ev {
                MobEvent::PlayerDamaged { amount, .. } => {
                    self.data.player.health -= amount;
                }
                // Mob drops (meat, bone, gunpowder) have no item counterpart:
                // hunger and brewing are deliberately out of scope, so there is
                // nothing for them to become. The events are left in place so a
                // later build can give them one.
                MobEvent::Drop { .. }
                | MobEvent::Exploded { .. }
                | MobEvent::MobDied { .. } => {}
            }
        }

        if self.data.player.health <= 0.0 {
            self.respawn();
        }

        // Furnaces keep working whether or not anyone is looking at them.
        for f in self.furnaces.values_mut() {
            f.tick(dt);
        }

        // --- autosave ---
        self.time_since_save += dt;
        if save::should_autosave(self.time_since_save) {
            self.save_now();
        }
    }

    /// Rebuild the overlay and the mob geometry, then draw.
    fn draw(&mut self) {
        let daylight = daylight_at(self.time_of_day);
        let sky = sky_for(daylight);

        // Mob geometry: a body box and a head box per mob. Flat colours, which
        // is all this project wants.
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for m in self.mobs.mobs() {
            let size = m.kind.size();
            let half = size.x * 0.5;
            let min = Vec3::new(m.pos.x - half, m.pos.y, m.pos.z - half);
            let max = Vec3::new(m.pos.x + half, m.pos.y + size.y, m.pos.z + half);
            gfx::Renderer::box_geometry(&mut verts, &mut indices, min, max, m.kind.color());

            // A darker head cube marks facing without needing a model.
            let hs = size.x * 0.34;
            let f = Vec3::new(m.yaw.cos(), 0.0, m.yaw.sin()) * (half * 0.7);
            let hc = Vec3::new(m.pos.x, m.pos.y + size.y * 0.86, m.pos.z) + f;
            let c = m.kind.color();
            gfx::Renderer::box_geometry(
                &mut verts,
                &mut indices,
                hc - Vec3::splat(hs),
                hc + Vec3::splat(hs),
                [c[0] * 0.55, c[1] * 0.55, c[2] * 0.55],
            );
        }
        for p in self.mobs.projectiles() {
            gfx::Renderer::box_geometry(
                &mut verts,
                &mut indices,
                p.pos - Vec3::splat(0.06),
                p.pos + Vec3::splat(0.06),
                [0.85, 0.85, 0.88],
            );
        }

        let Some(gfx) = self.gfx.as_mut() else { return };
        gfx.set_entities(&verts, &indices);

        let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
        gfx.hud.begin(w, h);

        // Hotbar, built from the inventory.
        let slots: Vec<Option<Slot>> = self
            .data
            .inventory
            .hotbar()
            .iter()
            .map(|s| s.map(|st| Slot::new(st.item.color(), st.count as u16)))
            .collect();
        if self.ui == Ui::Playing {
            gfx.hud.hotbar(&slots, self.data.inventory.selected());
            gfx.hud.health(self.data.player.health, 20.0);
            gfx.hud.crosshair();
        }

        let held = self
            .data
            .inventory
            .selected_stack()
            .map(|s| s.item.name().to_string())
            .unwrap_or_else(|| "empty hand".to_string());
        gfx.hud.readout(
            self.player.pos.to_array(),
            self.fps,
            &format!(
                "{held}  |  {}  |  {}",
                if self.smash_mode { "SMASH (loud)" } else { "chip (quiet)" },
                if daylight > 0.5 { "day" } else { "NIGHT" }
            ),
        );

        if self.loading {
            gfx.hud.text_shadowed(
                w * 0.5 - 60.0,
                h * 0.5 - 40.0,
                hud::TEXT_SIZE,
                [1.0, 1.0, 1.0, 1.0],
                "loading world",
            );
        }

        if self.ui.is_panel() {
            let furnace = match self.ui {
                Ui::Furnace(key) => self.furnaces.get(&key),
                _ => None,
            };
            draw_panel(
                gfx,
                self.ui,
                &self.data.inventory,
                &self.craft_grid,
                furnace,
                self.carried,
                self.cursor,
            );
        }

        gfx.render(&self.camera, sky);
    }
}

// --- inventory screen geometry, shared by drawing and hit-testing ------------

const SLOT_PX: f32 = 44.0;
const SLOT_GAP: f32 = 4.0;

/// Top-left of the 9x4 inventory grid for a given screen size.
fn inv_origin(w: f32, h: f32) -> (f32, f32) {
    let gw = 9.0 * SLOT_PX + 8.0 * SLOT_GAP;
    ((w - gw) * 0.5, h * 0.5 - 40.0)
}

/// Screen rect of one flat slot index (0..36).
fn slot_rect(w: f32, h: f32, index: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    // Row 0 is the hotbar, drawn at the bottom of the panel with a gap.
    let (col, row) = (index % 9, index / 9);
    let y = if row == 0 {
        oy + 3.0 * (SLOT_PX + SLOT_GAP) + 14.0
    } else {
        oy + (row as f32 - 1.0) * (SLOT_PX + SLOT_GAP)
    };
    (ox + col as f32 * (SLOT_PX + SLOT_GAP), y)
}

/// Crafting cell rect. `cells` is 4 (2x2) or 9 (3x3); the grid stays centred on
/// the same column either way, so the panel does not jump when it widens.
fn craft_rect(w: f32, h: f32, index: usize, cells: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let side = if cells == 9 { 3 } else { 2 };
    let cx = ox + 4.6 * step - side as f32 * step * 0.5;
    let cy = oy - (0.4 + side as f32) * step;
    (
        cx + (index % side) as f32 * step,
        cy + (index / side) as f32 * step,
    )
}

fn craft_output_rect(w: f32, h: f32, cells: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let side = if cells == 9 { 3 } else { 2 };
    (
        ox + 6.4 * step,
        oy - (0.9 + side as f32 * 0.5) * step,
    )
}

/// Furnace slots: 0 input (top), 1 fuel (below it), 2 output (to the right).
fn furnace_rect(w: f32, h: f32, index: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let cx = ox + 3.2 * step;
    let cy = oy - 3.4 * step;
    match index {
        0 => (cx, cy),
        1 => (cx, cy + 2.0 * step),
        _ => (cx + 3.0 * step, cy + step),
    }
}

fn draw_panel(
    gfx: &mut gfx::Renderer,
    ui: Ui,
    inv: &inventory::Inventory,
    grid: &[Option<ItemStack>; 9],
    furnace: Option<&crafting::Furnace>,
    carried: Option<ItemStack>,
    cursor: (f32, f32),
) {
    let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
    gfx.hud.screen_dim();
    let (ox, oy) = inv_origin(w, h);
    let gw = 9.0 * SLOT_PX + 8.0 * SLOT_GAP;
    let step = SLOT_PX + SLOT_GAP;
    gfx.hud.panel(ox - 16.0, oy - 4.8 * step, gw + 32.0, 8.2 * step + 40.0);

    let to_slot = |s: Option<ItemStack>| s.map(|st| Slot::new(st.item.color(), st.count as u16));

    // The 36 inventory slots are common to every panel.
    for i in 0..36 {
        let (x, y) = slot_rect(w, h, i);
        gfx.hud
            .slot(x, y, SLOT_PX, to_slot(inv.slot(i)), i == inv.selected());
    }

    let title = match ui {
        Ui::Furnace(_) => "FURNACE   ore above, fuel below",
        Ui::Table => "CRAFTING TABLE   3x3",
        _ => "INVENTORY   2x2, table for tools",
    };

    match ui {
        Ui::Furnace(_) => {
            let f = furnace.expect("furnace panel opened without a furnace");
            for (i, stack) in [f.input, f.fuel, f.output].iter().enumerate() {
                let (x, y) = furnace_rect(w, h, i);
                gfx.hud.slot(x, y, SLOT_PX, to_slot(*stack), false);
            }
            // Flame and progress gauges, as plain bars.
            let (fx, fy) = furnace_rect(w, h, 1);
            let burn = f.burn_fraction();
            gfx.hud.rect(
                fx + SLOT_PX + 8.0,
                fy + SLOT_PX * (1.0 - burn),
                10.0,
                SLOT_PX * burn,
                [0.95, 0.55, 0.15, 1.0],
            );
            let (px, py) = furnace_rect(w, h, 0);
            let prog = f.progress_fraction();
            gfx.hud.rect(
                px + SLOT_PX + 8.0,
                py + SLOT_PX * 0.45,
                (2.6 * step - 16.0) * prog,
                10.0,
                [0.85, 0.85, 0.9, 1.0],
            );
        }
        _ => {
            let cells = ui.craft_cells();
            for i in 0..cells {
                let (x, y) = craft_rect(w, h, i, cells);
                gfx.hud.slot(x, y, SLOT_PX, to_slot(grid[i]), false);
            }
            let (cx, cy) = craft_output_rect(w, h, cells);
            gfx.hud.slot(
                cx,
                cy,
                SLOT_PX,
                to_slot(crafting::resolve(&grid[..cells])),
                false,
            );
        }
    }

    gfx.hud
        .text_shadowed(ox, oy - 4.55 * step, hud::TEXT_SIZE, [0.92, 0.92, 0.95, 1.0], title);

    // The carried stack rides the cursor so it is obvious what is in hand.
    if let Some(st) = carried {
        gfx.hud.slot(
            cursor.0 - SLOT_PX * 0.4,
            cursor.1 - SLOT_PX * 0.4,
            SLOT_PX * 0.8,
            to_slot(Some(st)),
            false,
        );
    }
}

impl App {
    /// Click handling for whichever panel is open. Returns true if it consumed
    /// the click.
    fn panel_click(&mut self, right: bool) -> bool {
        let Some(gfx) = self.gfx.as_ref() else {
            return false;
        };
        let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
        let (mx, my) = self.cursor;
        let inside = |x: f32, y: f32, s: f32| mx >= x && mx < x + s && my >= y && my < y + s;

        if let Ui::Furnace(key) = self.ui {
            if let Some(mut f) = self.furnaces.get(&key).cloned() {
                for i in 0..3 {
                    let (x, y) = furnace_rect(w, h, i);
                    if !inside(x, y, SLOT_PX) {
                        continue;
                    }
                    if i == 2 {
                        // The output slot only ever gives; it never accepts.
                        if let Some(out) = f.take_output() {
                            let spilled = self.data.inventory.add_item(out.item, out.count as u32);
                            if spilled > 0 {
                                self.carried = Some(ItemStack::new(out.item, spilled as u8));
                            }
                        }
                    } else {
                        let cell = if i == 0 { &mut f.input } else { &mut f.fuel };
                        swap_carried(&mut self.carried, cell, right);
                    }
                    self.furnaces.insert(key, f);
                    return true;
                }
            }
        } else {
            let cells = self.ui.craft_cells();
            let (cx, cy) = craft_output_rect(w, h, cells);
            if inside(cx, cy, SLOT_PX) {
                if let Some(out) = crafting::craft(&mut self.craft_grid[..cells]) {
                    let spilled = self.data.inventory.add_item(out.item, out.count as u32);
                    if spilled > 0 {
                        self.carried = Some(ItemStack::new(out.item, spilled as u8));
                    }
                }
                return true;
            }
            for i in 0..cells {
                let (x, y) = craft_rect(w, h, i, cells);
                if inside(x, y, SLOT_PX) {
                    swap_carried(&mut self.carried, &mut self.craft_grid[i], right);
                    return true;
                }
            }
        }

        for i in 0..36 {
            let (x, y) = slot_rect(w, h, i);
            if inside(x, y, SLOT_PX) {
                let mut cell = self.data.inventory.slot(i);
                swap_carried(&mut self.carried, &mut cell, right);
                self.data.inventory.set_slot(i, cell);
                return true;
            }
        }
        false
    }
}

/// Pick up, put down, merge, or split one stack against the carried one.
fn swap_carried(carried: &mut Option<ItemStack>, cell: &mut Option<ItemStack>, right: bool) {
    match (carried.take(), cell.take()) {
        (None, Some(s)) => {
            if right && s.count > 1 {
                // Right click takes half and leaves the rest.
                let half = s.count / 2;
                *carried = Some(ItemStack::new(s.item, half));
                *cell = Some(ItemStack::new(s.item, s.count - half));
            } else {
                *carried = Some(s);
            }
        }
        (Some(c), None) => {
            if right && c.count > 1 {
                *cell = Some(ItemStack::new(c.item, 1));
                *carried = Some(ItemStack::new(c.item, c.count - 1));
            } else {
                *cell = Some(c);
            }
        }
        (Some(c), Some(s)) => {
            if c.item == s.item && s.count < 64 {
                let room = 64 - s.count;
                let moved = room.min(c.count);
                *cell = Some(ItemStack::new(s.item, s.count + moved));
                *carried = if c.count > moved {
                    Some(ItemStack::new(c.item, c.count - moved))
                } else {
                    None
                };
            } else {
                *cell = Some(c);
                *carried = Some(s);
            }
        }
        (None, None) => {}
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Loudstone")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let renderer = pollster::block_on(gfx::Renderer::new(window.clone()));
        self.camera.aspect = renderer.config.width as f32 / renderer.config.height as f32;
        self.gfx = Some(renderer);
        self.window = Some(window);
        self.set_cursor_locked(true);
        self.last_frame = Instant::now();
        self.start = Instant::now();
        println!(
            "[loudstone] WASD move  Space jump  Ctrl sprint  F noclip\n\
             [loudstone] LMB chip (quiet)  Alt+LMB smash (LOUD)  RMB place\n\
             [loudstone] 1-9 / wheel hotbar  E inventory  Esc release cursor"
        );
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.cursor_locked && self.ui == Ui::Playing {
                self.camera.look(delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_now();
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size);
                    self.camera.aspect = size.width as f32 / size.height.max(1) as f32;
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / 60.0,
                };
                if d.abs() > 0.01 {
                    self.data.inventory.cycle_selection(d.signum() as i32);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if self.ui.is_panel() {
                    if pressed {
                        self.panel_click(button == MouseButton::Right);
                    }
                    return;
                }
                if pressed && !self.cursor_locked {
                    self.set_cursor_locked(true);
                    return;
                }
                match button {
                    MouseButton::Left => self.mining = pressed,
                    MouseButton::Right => self.placing = pressed,
                    _ => {}
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::KeyW => self.input.fwd = pressed,
                        KeyCode::KeyS => self.input.back = pressed,
                        KeyCode::KeyA => self.input.left = pressed,
                        KeyCode::KeyD => self.input.right = pressed,
                        KeyCode::Space => self.input.up = pressed,
                        KeyCode::ShiftLeft => self.input.down = pressed,
                        KeyCode::ControlLeft => self.input.fast = pressed,
                        KeyCode::AltLeft => self.smash_mode = pressed,
                        KeyCode::F2 if pressed => {
                            if let Some(gfx) = self.gfx.as_mut() {
                                let n = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                gfx.request_capture(
                                    std::path::PathBuf::from("shots")
                                        .join(format!("loudstone_{n}.png")),
                                );
                            }
                        }
                        KeyCode::KeyF if pressed => {
                            self.player.noclip = !self.player.noclip;
                            if !self.player.noclip {
                                self.player.unstick(&self.world);
                            }
                        }
                        KeyCode::KeyE if pressed => {
                            if self.ui.is_panel() {
                                self.close_panel();
                            } else {
                                self.mining = false;
                                self.placing = false;
                                self.ui = Ui::Inventory;
                                self.set_cursor_locked(false);
                            }
                        }
                        KeyCode::Escape if pressed => {
                            if self.ui.is_panel() {
                                self.close_panel();
                            } else {
                                self.set_cursor_locked(false);
                            }
                        }
                        _ => {
                            if pressed {
                                if let Some(n) = digit_row(code) {
                                    self.data.inventory.set_selected(n);
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;

                let gauntlet_shot = self.drive_gauntlet(dt);
                self.update(dt);
                if let Some(name) = gauntlet_shot {
                    if let Some(gfx) = self.gfx.as_mut() {
                        gfx.request_capture(
                            std::path::PathBuf::from("shots").join(format!("gauntlet_{name}.png")),
                        );
                    }
                }
                if self.gauntlet_done {
                    event_loop.exit();
                }

                self.fps_accum += dt;
                self.fps_frames += 1;
                if self.fps_accum >= 0.4 {
                    self.fps = self.fps_frames as f32 / self.fps_accum;
                    self.fps_accum = 0.0;
                    self.fps_frames = 0;
                    if let Some(w) = &self.window {
                        w.set_title(&format!(
                            "Loudstone  |  {:.0} fps  |  {} chunks  |  {} mobs",
                            self.fps,
                            self.world.chunks.len(),
                            self.mobs.mobs().len()
                        ));
                    }
                }

                self.draw();

                // `--shot <path>`: wait for the world to finish streaming, give
                // it a few frames to settle, capture, and quit.
                if let Some(path) = self.shot_path.clone() {
                    if !self.loading {
                        self.shot_countdown -= 1;
                        if self.shot_countdown == 60 {
                            if self.demo {
                                self.run_demo();
                            }
                            if let Some(which) = self.ui_demo.clone() {
                                self.run_ui_demo(&which);
                            }
                        }
                        if self.shot_countdown == 0 {
                            if let Some(gfx) = self.gfx.as_mut() {
                                gfx.request_capture(path);
                            }
                        } else if self.shot_countdown < 0 {
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// Slab-method ray/AABB intersection. Returns the entry distance, or `None`.
fn ray_box(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut tmin = 0.0f32;
    let mut tmax = f32::INFINITY;
    for a in 0..3 {
        let d = dir[a];
        if d.abs() < 1.0e-8 {
            // Parallel to this slab: a miss unless the origin is already inside.
            if origin[a] < min[a] || origin[a] > max[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let (mut t1, mut t2) = ((min[a] - origin[a]) * inv, (max[a] - origin[a]) * inv);
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmax < tmin {
            return None;
        }
    }
    Some(tmin)
}

fn digit_row(code: KeyCode) -> Option<usize> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        _ => return None,
    })
}

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> (Vec3, Vec3) {
        (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.8, 0.5))
    }

    #[test]
    fn a_ray_aimed_at_a_mob_reports_the_entry_distance() {
        let (min, max) = unit_box();
        let t = ray_box(Vec3::new(0.0, 1.0, -6.0), Vec3::Z, min, max)
            .expect("a ray straight at the box must hit it");
        assert!((t - 5.5).abs() < 1.0e-3, "entry distance was {t}");
    }

    #[test]
    fn a_ray_beside_a_mob_misses_it() {
        let (min, max) = unit_box();
        assert!(ray_box(Vec3::new(4.0, 1.0, -6.0), Vec3::Z, min, max).is_none());
    }

    #[test]
    fn a_ray_over_a_mob_misses_it() {
        let (min, max) = unit_box();
        assert!(ray_box(Vec3::new(0.0, 9.0, -6.0), Vec3::Z, min, max).is_none());
    }

    #[test]
    fn a_ray_pointing_away_from_a_mob_misses_it() {
        let (min, max) = unit_box();
        assert!(ray_box(Vec3::new(0.0, 1.0, -6.0), -Vec3::Z, min, max).is_none());
    }

    #[test]
    fn a_ray_parallel_to_a_slab_still_hits_when_it_is_inside_it() {
        // Travelling along +X at a height and depth that sit inside the box:
        // the X slab is parallel, so the degenerate branch must not reject it.
        let (min, max) = unit_box();
        assert!(ray_box(Vec3::new(-8.0, 1.0, 0.0), Vec3::X, min, max).is_some());
    }

    #[test]
    fn daylight_runs_a_full_cycle_from_noon_to_midnight_and_back() {
        assert_eq!(daylight_at(0.0), 1.0);
        assert_eq!(daylight_at(DAY_LENGTH * 0.5), 1.0, "still day at half a cycle");
        // Deep night sits between the two twilight ramps.
        let night = DAY_LENGTH * (DAY_FRACTION + (1.0 - DAY_FRACTION) * 0.5 + 0.02);
        assert_eq!(daylight_at(night), 0.0);
        // And the sky follows it in both directions.
        assert_eq!(sky_for(1.0), SKY_COLOR);
        assert_eq!(sky_for(0.0), SKY_NIGHT);
    }
    // -----------------------------------------------------------------------
    // The Phase 4 acceptance criterion, as an actual test: start from nothing,
    // reach a stone pickaxe, then smelt iron. This is the whole progression the
    // 2x2-only build silently could not do -- every tool is a 3x3 recipe, so
    // without a crafting table panel the chain dead-ends at planks and sticks.
    // -----------------------------------------------------------------------

    fn stack(item: ItemId, n: u8) -> Option<ItemStack> {
        Some(ItemStack::new(item, n))
    }

    #[test]
    fn a_2x2_grid_cannot_make_a_pickaxe_but_a_3x3_can() {
        // Every tool needs three across the top and two sticks down the middle,
        // so it cannot fit in the player's own 2x2 grid at any offset.
        assert_eq!(Ui::Inventory.craft_cells(), 4);
        assert_eq!(Ui::Table.craft_cells(), 9);

        let two = [
            stack(ItemId::COBBLESTONE, 1),
            stack(ItemId::COBBLESTONE, 1),
            stack(ItemId::STICK, 1),
            stack(ItemId::STICK, 1),
        ];
        assert!(
            crafting::resolve(&two).is_none(),
            "a pickaxe must not be craftable in the 2x2 grid"
        );

        let three = [
            stack(ItemId::COBBLESTONE, 1),
            stack(ItemId::COBBLESTONE, 1),
            stack(ItemId::COBBLESTONE, 1),
            None,
            stack(ItemId::STICK, 1),
            None,
            None,
            stack(ItemId::STICK, 1),
            None,
        ];
        let out = crafting::resolve(&three).expect("3x3 must resolve a stone pickaxe");
        assert_eq!(out.item, ItemId::STONE_PICKAXE);
    }

    #[test]
    fn the_full_progression_from_bare_hands_to_an_iron_ingot() {
        let mut inv = inventory::Inventory::new();

        // Punch a tree: wood -> planks -> sticks, both 2x2 recipes.
        inv.add_item(ItemId::WOOD, 2);
        let mut g = [None; 9];
        g[0] = stack(ItemId::WOOD, 1);
        let planks = crafting::craft(&mut g[..4]).expect("wood makes planks");
        assert_eq!(planks.item, ItemId::PLANKS);
        inv.add_item(planks.item, planks.count as u32);

        // A crafting table is itself a 2x2 recipe, which is what unlocks 3x3.
        let mut g = [None; 9];
        for c in g.iter_mut().take(4) {
            *c = stack(ItemId::PLANKS, 1);
        }
        let table = crafting::craft(&mut g[..4]).expect("four planks make a table");
        assert_eq!(table.item, ItemId::CRAFTING_TABLE);

        // At the table: a stone pickaxe.
        let mut g = [None; 9];
        for c in g.iter_mut().take(3) {
            *c = stack(ItemId::COBBLESTONE, 1);
        }
        g[4] = stack(ItemId::STICK, 1);
        g[7] = stack(ItemId::STICK, 1);
        let pick = crafting::craft(&mut g[..9]).expect("the table makes a stone pickaxe");
        assert_eq!(pick.item, ItemId::STONE_PICKAXE);

        // That pickaxe is exactly what iron ore requires -- a wooden one is not.
        assert!(item::can_harvest(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE));
        assert!(!item::can_harvest(Some(ItemId::WOODEN_PICKAXE), BlockId::IRON_ORE));
        assert_eq!(
            item::mining_drop(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE),
            Some(ItemId::RAW_IRON)
        );

        // Smelt it. Coal is the fuel; the furnace ticks on its own.
        let mut f = crafting::Furnace::new();
        f.input = stack(ItemId::RAW_IRON, 1);
        f.fuel = stack(ItemId::COAL, 1);
        for _ in 0..1200 {
            f.tick(1.0 / 60.0);
        }
        let ingot = f.take_output().expect("raw iron smelts into an ingot");
        assert_eq!(ingot.item, ItemId::IRON_INGOT);

        // And an iron pickaxe is what gets diamond, closing the chain.
        let mut g = [None; 9];
        for c in g.iter_mut().take(3) {
            *c = stack(ItemId::IRON_INGOT, 1);
        }
        g[4] = stack(ItemId::STICK, 1);
        g[7] = stack(ItemId::STICK, 1);
        let iron_pick = crafting::craft(&mut g[..9]).expect("ingots make an iron pickaxe");
        assert_eq!(iron_pick.item, ItemId::IRON_PICKAXE);
        assert!(item::can_harvest(Some(ItemId::IRON_PICKAXE), BlockId::DIAMOND_ORE));
        assert!(!item::can_harvest(Some(ItemId::STONE_PICKAXE), BlockId::DIAMOND_ORE));

        let _ = inv.slot(0);
    }

    #[test]
    fn a_furnace_will_not_burn_fuel_with_nothing_to_smelt() {
        let mut f = crafting::Furnace::new();
        f.fuel = stack(ItemId::COAL, 1);
        for _ in 0..600 {
            f.tick(1.0 / 60.0);
        }
        assert!(!f.is_burning(), "an idle furnace must not waste its fuel");
        assert_eq!(f.fuel.map(|s| s.count), Some(1));
    }
    #[test]
    fn a_block_can_be_placed_into_water_and_ground_cover() {
        // The gauntlet found this: standing beside a lake, every placement was
        // refused, because the target had to be air rather than replaceable.
        assert!(BlockId::WATER.is_replaceable());
        assert!(BlockId::TALL_GRASS.is_replaceable());
        assert!(BlockId::AIR.is_replaceable());
        // Solid ground never gives way.
        assert!(!BlockId::STONE.is_replaceable());
        assert!(!BlockId::DIRT.is_replaceable());
        assert!(!BlockId::LEAVES.is_replaceable());
    }
}
