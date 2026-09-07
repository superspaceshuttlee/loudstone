//! Loudstone -- a voxel survival game.
//!
//! Two mechanics define it, and they are deliberately wired into each other:
//! blocks are internally 8^3 sub-voxel grids that can be chipped away a grain at
//! a time, and mining emits noise that hostile mobs hear and walk toward.
//! Chipping is slow and quiet. Smashing a whole block out is fast and loud.
//! Every mining decision is therefore a bet on speed against safety.

mod app;
mod audio;
mod config;
mod content;
mod dev;
mod persist;
mod render;
mod sim;
mod world;

use app::cli::Cli;
use app::ui::{
    SLOT_PX, TitleAction, craft_output_rect, craft_rect, draw_panel, draw_title_screen,
    furnace_rect, merge_into_cell, return_panel_items, slot_rect, store_output, swap_carried,
    title_action,
};
use config::*;
use content::block::BlockId;
use content::inventory::ItemStack;
use content::item::ItemId;
use glam::Vec3;
use render::hud::Slot;
use sim::camera::{Camera, MoveInput, Player};
use sim::daylight::{DAY_FRACTION, DAY_LENGTH, daylight_at, sky_for, sun_for};
use sim::mob::{MobEvent, MobKind, MobManager, PlayerState};
use sim::session::{Hands, Stats};
use sim::sound::SoundField;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};
use world::World;
use world::chunk::ChunkPos;

// ---------------------------------------------------------------------------
// The two trait bridges. Both modules were written against narrow traits so
// they could be built in parallel without depending on the engine; these are
// where they meet the real world.
// ---------------------------------------------------------------------------

impl sim::sound::VoxelWorld for World {
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

impl persist::save::WorldEdit for World {
    fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        self.set_block(x, y, z, id);
    }
    fn carve_at(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        self.carve(x, y, z, sx, sy, sz)
    }
}

/// A hunched, asymmetrical grave-roamer built from articulated low-poly parts.
/// It deliberately avoids the familiar square-shirt humanoid silhouette: the
/// shoulders are uneven, the jaw projects, and its long arms lead its gait.
/// Draw one mob as a posed humanoid.
///
/// Every mob shares the same six-box rig and differs only by skin and a couple
/// of pose flags. That is deliberate: in this art style, character comes from
/// the texture, and giving each kind its own geometry is what made an earlier
/// attempt look like it belonged to a different game.
fn append_mob_model(
    verts: &mut Vec<render::mesh::Vertex>,
    indices: &mut Vec<u32>,
    m: &sim::mob::Mob,
    light: f32,
) {
    let kind_index = sim::mob::MobKind::ALL
        .iter()
        .position(|k| *k == m.kind)
        .unwrap_or(0);
    let speed = Vec3::new(m.vel.x, 0.0, m.vel.z).length();

    let pose = render::model::Pose {
        // Driving the gait by distance travelled rather than by a clock is what
        // stops the legs cycling while the mob is stuck against a wall.
        stride: m.gait * 6.0,
        speed: (speed / 4.0).clamp(0.0, 1.0),
        head_yaw: 0.0,
        head_pitch: 0.0,
        attack: 0.0,
        arms_forward: if m.kind == sim::mob::MobKind::Zombie {
            1.0
        } else {
            0.0
        },
        waddle: m.kind == sim::mob::MobKind::Creeper,
        on_all_fours: m.kind == sim::mob::MobKind::Pig,
    };

    // Every rig is built at true scale now. The pig used to be drawn at 0.8
    // because it was a humanoid bent over on all fours and needed shrinking to
    // pass; the real quadruped rig is 14 units tall, which is the 0.875 blocks a
    // pig is supposed to be, and shrinking it again would leave it rattling
    // around inside its own hitbox.
    let scale = 1.0;

    let parts = if m.kind == sim::mob::MobKind::Pig {
        render::model::quadruped()
    } else {
        render::model::humanoid()
    };

    render::model::append(
        parts, kind_index, verts, indices, m.pos, m.yaw, scale, &pose, light,
    );
}

// ---------------------------------------------------------------------------

/// Which on-screen panel has focus. The cursor is only released for a panel.
#[derive(PartialEq, Copy, Clone)]
enum Ui {
    /// Startup menu shown before simulation begins.
    Title,
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
        matches!(self, Ui::Inventory | Ui::Table | Ui::Furnace(_))
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

/// Gives simulation systems the same world view as `World` while making every
/// mutation durable. Mobs use this adapter because creeper blasts happen below
/// the app layer, where a later event cannot reconstruct the exact ragged mask.
struct TrackedWorld<'a> {
    world: &'a mut World,
    edits: &'a mut persist::save::ChangeTracker,
}

impl TrackedWorld<'_> {
    fn resident(&self, x: i32, y: i32, z: i32) -> bool {
        self.world.chunks.contains_key(&ChunkPos::new(
            x.div_euclid(CHUNK_SIZE_I),
            y.div_euclid(CHUNK_SIZE_I),
            z.div_euclid(CHUNK_SIZE_I),
        ))
    }
}

impl sim::sound::VoxelWorld for TrackedWorld<'_> {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.world.block_at(x, y, z)
    }

    fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        self.world.sub_solid(x, y, z, sx, sy, sz)
    }

    fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32 {
        self.world.fill_ratio(x, y, z)
    }

    fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        let changed = self.resident(x, y, z) && self.world.sub_solid(x, y, z, sx, sy, sz);
        let destroyed = self.world.carve(x, y, z, sx, sy, sz);
        if changed {
            self.edits.note_carve(x, y, z, sx, sy, sz);
        }
        destroyed
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if self.resident(x, y, z) {
            self.world.set_block(x, y, z, id);
            self.edits.note_set_block(x, y, z, id);
        }
    }
}

fn restore_mobs(data: &persist::save::SaveData, mobs: &mut MobManager) {
    for saved in &data.mobs {
        saved.spawn_into(mobs);
    }
}

fn restore_furnaces(
    data: &persist::save::SaveData,
) -> HashMap<persist::save::BlockPos, content::crafting::Furnace> {
    data.containers
        .iter()
        .filter_map(|(&pos, saved)| saved.to_furnace().map(|furnace| (pos, furnace)))
        .collect()
}

fn capture_live_state(
    data: &mut persist::save::SaveData,
    mobs: &MobManager,
    furnaces: &HashMap<persist::save::BlockPos, content::crafting::Furnace>,
    player_pos: Vec3,
) {
    data.capture_mobs(mobs.mobs(), player_pos);
    // Preserve unknown future container kinds, while replacing every furnace
    // record with the current simulation state.
    data.containers
        .retain(|_, container| container.kind != persist::save::ContainerKind::FURNACE);
    for (&pos, furnace) in furnaces {
        data.set_container(pos, persist::save::ContainerSave::from_furnace(furnace));
    }
}

/// Forget every furnace in this region whose block is no longer there, and hand
/// back whatever those furnaces were holding.
///
/// A furnace's contents live in a position-keyed map, not in the block itself,
/// so destroying the block leaves the map entry behind. Left alone that orphan
/// is written out by every autosave, restored on load, and then silently adopted
/// by the next furnace placed on the same coordinates -- handing the player back
/// everything the old one held, as often as they care to repeat it. The save
/// file also grows forever, one dead furnace at a time.
///
/// It takes a *region* and a block lookup rather than a single position because a
/// furnace can also stop existing by being blown up, and a creeper's blast is
/// nowhere near the code that knows what a container is. The region is not
/// decoration: a bare "check every furnace" sweep would delete furnaces sitting
/// in unloaded chunks, where `block_at` answers from raw terrain and so can never
/// say FURNACE.
/// Simulated seconds per frame during a gauntlet session. A fixed step is what
/// makes a run a pure function of its seed; 1/60 matches how the game actually
/// plays, so the physics and timers the robot exercises behave as a player's do.
const GAUNTLET_STEP: f32 = 1.0 / 60.0;

fn forget_orphan_furnaces(
    furnaces: &mut HashMap<persist::save::BlockPos, content::crafting::Furnace>,
    lo: persist::save::BlockPos,
    hi: persist::save::BlockPos,
    block_at: impl Fn(i32, i32, i32) -> BlockId,
) -> Vec<ItemStack> {
    let mut salvaged = Vec::new();
    furnaces.retain(|&(x, y, z), furnace| {
        let inside =
            (lo.0..=hi.0).contains(&x) && (lo.1..=hi.1).contains(&y) && (lo.2..=hi.2).contains(&z);
        if !inside || block_at(x, y, z) == BlockId::FURNACE {
            return true;
        }
        salvaged.extend(
            [furnace.input, furnace.fuel, furnace.output]
                .into_iter()
                .flatten(),
        );
        false
    });
    salvaged
}

fn replay_arrived_chunks(
    data: &mut persist::save::SaveData,
    world: &mut World,
    replayed: &mut HashSet<ChunkPos>,
    arrived: &[ChunkPos],
) {
    for &pos in arrived {
        if replayed.insert(pos) {
            data.edits.replay_chunk((pos.x, pos.y, pos.z), world);
        }
    }
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<render::gfx::Renderer>,
    world: World,
    camera: Camera,
    player: Player,
    input: MoveInput,
    mobs: MobManager,
    sound: SoundField,
    /// What the player actually hears. `sound` above is the mob-AI
    /// propagation model and makes no noise of its own.
    audio: audio::Audio,
    /// Ground contact last frame, so a landing can be detected.
    was_on_ground: bool,
    /// Distance walked since the last footstep.
    step_accum: f32,

    /// Seed, player record, inventory and the durable edit log. This IS the save.
    data: persist::save::SaveData,
    save_path: std::path::PathBuf,
    has_save: bool,
    time_since_save: f32,

    /// Chunks whose saved edits have already been replayed since becoming
    /// resident. Cleared for a chunk when it streams out.
    replayed: HashSet<ChunkPos>,

    ui: Ui,
    craft_grid: [Option<ItemStack>; 9],
    /// Furnace contents, keyed by the block they belong to, so two furnaces do
    /// not share one inventory.
    furnaces: HashMap<(i32, i32, i32), content::crafting::Furnace>,
    /// The stack held by the cursor in the inventory screen.
    carried: Option<ItemStack>,
    cursor: (f32, f32),

    cursor_locked: bool,
    /// Set by `--shot <path>`: capture one frame once the world is loaded, then quit.

    /// `--demo`: carve a crater and spawn one of each mob before capturing, so
    /// the two headline mechanics can be verified in a still frame.

    /// `--models`: a review stand showing every mob together.

    /// `--angle <degrees>`: turn the review models by this much.

    /// `--ui table` / `--ui furnace`: open that panel before capturing.

    /// `--model zombie`: stage one model close to the camera for visual QA.

    /// What the program was asked to do on the command line.
    cli: Cli,
    /// What the player is doing with their hands, and its cooldowns.
    hands: Hands,
    /// Counters for the overlay and the test harness. Never read back by the
    /// simulation.
    stats: Stats,
    /// `--gauntlet`: a robot plays the game and reports what broke.
    gauntlet: Option<dev::gauntlet::Harness>,
    save_roundtrip: Option<bool>,
    /// Multiplier on the day/night clock, driven by the gauntlet.
    time_scale: f32,
    // Monotonic counters the gauntlet watches to tell whether anything happened.
    shot_countdown: i32,

    time_of_day: f32,
    spawn: Vec3,
    last_frame: Instant,
    /// Wall clock at the previous rendered frame, for the real-time `dt`.
    last_frame_at: Instant,
    /// Set while the streamer is still filling the initial radius.
    loading: bool,
    gauntlet_done: bool,
    gauntlet_stocked: bool,
    gauntlet_findings: usize,
    load_frames: u32,
    start: Instant,
}

impl App {
    fn new() -> Self {
        // The gauntlet gets its own world and its own save file.
        //
        // It used to load whatever save happened to be on disk, which broke it
        // in two ways at once. It was not reproducible -- `--seed N` seeds only
        // the robot's choices, so the same seed replayed in whatever world the
        // player last left behind, and "reproduce with --seed 7" was a promise
        // the harness could not keep. And it was destructive: the robot mines,
        // smashes, places and autosaves, so running the test suite quietly
        // rearranged the world the player was actually playing in.
        //
        // Deriving the world seed from the run seed makes a session a pure
        // function of `--seed`, which is the whole point of a replayable harness.
        let cli = Cli::from_args();
        let gauntlet_seed = cli.gauntlet_seed;
        let save_path = match gauntlet_seed {
            Some(_) => std::env::temp_dir().join("loudstone_gauntlet_world.lsw"),
            None => persist::save::default_save_path(),
        };
        if let Some(seed) = gauntlet_seed {
            // Start from bare terrain every time, or yesterday's run leaks into
            // today's and the seed stops determining the session again.
            let _ = std::fs::remove_file(&save_path);
            println!(
                "[gauntlet] scratch world seed {seed}, save {}",
                save_path.display()
            );
        }
        let (data, has_save) = if persist::save::save_exists(&save_path) {
            match persist::save::load_from_file(&save_path) {
                Ok(d) => {
                    println!("[loudstone] loaded save (seed {})", d.seed);
                    (d, true)
                }
                Err(e) => {
                    eprintln!("[loudstone] could not load save: {e} -- starting fresh");
                    (persist::save::SaveData::new(1337), false)
                }
            }
        } else {
            (
                persist::save::SaveData::new(gauntlet_seed.unwrap_or(1337) as u32),
                false,
            )
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

        let mut mobs = MobManager::new(data.seed as u64 ^ 0x9E37_79B9);
        restore_mobs(&data, &mut mobs);
        let furnaces = restore_furnaces(&data);

        Self {
            window: None,
            gfx: None,
            world,
            camera,
            player,
            input: MoveInput::default(),
            mobs,
            sound: SoundField::new(),
            audio: audio::Audio::new(),
            was_on_ground: false,
            step_accum: 0.0,
            data,
            save_path,
            has_save,
            time_since_save: 0.0,
            replayed: HashSet::new(),
            ui: if cli.automated() {
                Ui::Playing
            } else {
                Ui::Title
            },
            craft_grid: [None; 9],
            furnaces,
            carried: None,
            cursor: (0.0, 0.0),
            cursor_locked: false,
            // The same seed that chose the world above also drives the robot,
            // so the whole session is a pure function of `--seed`.
            gauntlet: cli.gauntlet_seed.map(|seed| {
                let secs = cli.gauntlet_secs.unwrap_or(dev::gauntlet::DEFAULT_SECONDS);
                println!("[gauntlet] seed {seed}, {secs:.0}s session");
                dev::gauntlet::Harness::new(seed, secs)
            }),
            cli,
            hands: Hands::default(),
            stats: Stats::default(),
            save_roundtrip: None,
            time_scale: 1.0,
            shot_countdown: 90,
            time_of_day: 0.0,
            spawn,
            last_frame: Instant::now(),
            last_frame_at: Instant::now(),
            loading: true,
            gauntlet_done: false,
            gauntlet_stocked: false,
            gauntlet_findings: 0,
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
        capture_live_state(&mut self.data, &self.mobs, &self.furnaces, self.player.pos);
        match persist::save::save_to_file(&self.save_path, &self.data) {
            Ok(()) => {
                self.time_since_save = 0.0;
                self.has_save = true;
            }
            Err(e) => eprintln!("[loudstone] save failed: {e}"),
        }
    }

    fn enter_world(&mut self) {
        self.ui = Ui::Playing;
        self.loading = true;
        self.load_frames = 0;
        self.last_frame = Instant::now();
        self.start = Instant::now();
        self.set_cursor_locked(true);
    }

    fn start_new_world(&mut self) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u32)
            .unwrap_or(1337);
        let world = World::new(seed);
        let ground = world.surface_y(0, 0) as f32 + 1.0;
        let spawn = Vec3::new(0.5, ground, 0.5);
        let aspect = self.camera.aspect;

        self.data = persist::save::SaveData::new(seed);
        self.world = world;
        self.spawn = spawn;
        self.player = Player::new(spawn);
        self.camera = Camera::new(spawn + Vec3::Y * PLAYER_EYE_HEIGHT);
        self.camera.aspect = aspect;
        self.mobs = MobManager::new(seed as u64 ^ 0x9E37_79B9);
        self.sound = SoundField::new();
        self.furnaces.clear();
        self.replayed.clear();
        self.craft_grid.fill(None);
        self.carried = None;
        self.input = MoveInput::default();
        self.hands.stop();
        self.time_of_day = 0.0;
        self.save_now();
        self.enter_world();
    }

    /// Mining, placing, and the noise both of them make.
    fn interact(&mut self, dt: f32) {
        self.hands.tick(dt);

        // Let water find its level. Budgeted per frame so that flooding a long
        // tunnel is a wave you can watch rather than a hitch, and recorded in
        // the edit log so the sea does not drain again on the next load.
        for (x, y, z) in self.world.flow_water(WATER_FLOW_PER_FRAME) {
            self.data.edits.note_set_block(x, y, z, BlockId::WATER);
        }

        let eye = self.camera.pos;
        let hit = self
            .world
            .raycast(self.camera.pos, self.camera.forward(), REACH);

        if let Some(gfx) = self.gfx.as_mut() {
            gfx.set_highlight(hit.map(|h| h.block));
        }

        // A swing hits a mob before it touches the world behind it. Mining a
        // block the mob is standing in front of would otherwise be impossible.
        if self.hands.mining && self.hands.attack_timer <= 0.0 {
            let block_dist = hit.map(|h| h.distance).unwrap_or(REACH);
            if let Some((id, mob_pos, dist)) = self.mob_under_crosshair(REACH) {
                if dist <= block_dist {
                    self.hands.attack_timer = ATTACK_INTERVAL;
                    let damage = self
                        .data
                        .inventory
                        .selected_item()
                        .map(|i| i.attack_damage())
                        .unwrap_or(1.0);
                    let knock = (mob_pos - self.camera.pos).normalize_or_zero() * KNOCKBACK
                        + Vec3::Y * KNOCKBACK_LIFT;
                    let killed = self.mobs.damage(id, damage, knock);
                    self.audio.play(
                        if killed {
                            audio::Sound::MobDeath
                        } else {
                            audio::Sound::MobHurt
                        },
                        audio::PlayOpts::at(mob_pos, eye),
                    );
                    self.data.inventory.damage_selected(1);
                    // Swinging is loud: a fight is not a quiet way to spend time.
                    self.sound.emit(sim::sound::NoiseEvent {
                        pos: self.camera.pos,
                        loudness: NOISE_ATTACK,
                    });
                    return;
                }
            }
        }

        let Some(hit) = hit else { return };
        let (bx, by, bz) = hit.block;

        if self.hands.mining {
            let target = self.world.block_at(bx, by, bz);
            if target.hardness().is_finite() && !target.is_air() {
                let tool = self.data.inventory.selected_item();

                if self.hands.smash {
                    // Loud and fast: the whole block leaves at once. Everything
                    // within earshot hears it.
                    if self.hands.chip_timer <= 0.0 {
                        self.hands.chip_timer = SMASH_CHARGE;
                        if let Some(broken) = self.world.smash_block(bx, by, bz) {
                            self.stats.broken += 1;
                            self.stats.mined_kinds.push(broken.0);
                            // A smash is the loud option, and it should sound it.
                            self.audio.play(
                                audio::Sound::Break(broken),
                                audio::PlayOpts::at(hit.point, eye).with_volume(1.3),
                            );
                            self.data.edits.note_set_block(bx, by, bz, BlockId::AIR);
                            self.grant_drop(tool, broken);
                            self.break_furnace_at((bx, by, bz));
                            self.world.disturb_water(bx, by, bz);
                        }
                    }
                } else if self.hands.chip_timer <= 0.0 {
                    // Sticky targeting: keep eating the block the swing started
                    // on. Without it, the moment the ray drills through, mining
                    // silently jumps to the block behind and leaves a ring of
                    // the first one standing.
                    let aimed = (bx, by, bz);
                    if self.hands.target != Some(aimed)
                        && self
                            .hands
                            .target
                            .map(|t| self.world.block_at(t.0, t.1, t.2).is_air())
                            .unwrap_or(true)
                    {
                        self.hands.target = Some(aimed);
                    }
                    // Quiet and slow: carve a small sphere of sub-voxels. Hard
                    // blocks take proportionally longer per bite.
                    let speed = content::item::mining_speed_multiplier(tool, target);
                    self.hands.chip_timer = CHIP_INTERVAL * target.hardness() / speed.max(0.01);

                    let t = self.hands.target.unwrap_or(aimed);
                    let before = self.world.block_at(t.0, t.1, t.2);
                    let removed = self.world.chip_block(t, hit.point, CHIP_RADIUS);
                    self.stats.carved += removed as u64;
                    if removed > 0 {
                        // The dig tick has its own cooldown inside the mixer, so
                        // firing it every chip becomes a steady scrape rather
                        // than a machine-gun buzz.
                        self.audio.play(
                            audio::Sound::Dig(before),
                            audio::PlayOpts::at(hit.point, eye),
                        );
                    }
                    // Record what was carved so it survives streaming and saving.
                    self.note_carves(t);
                    if self.world.block_at(t.0, t.1, t.2).is_air() && !before.is_air() {
                        self.grant_drop(tool, before);
                        self.break_furnace_at(t);
                        self.world.disturb_water(t.0, t.1, t.2);
                        self.stats.mined_kinds.push(before.0);
                        self.hands.target = None;
                        self.audio.play(
                            audio::Sound::Break(before),
                            audio::PlayOpts::at(hit.point, eye),
                        );
                    }
                }
            }
        }

        if self.hands.placing && self.hands.place_timer <= 0.0 {
            // Right-clicking a workstation opens it rather than placing against it.
            let aimed = self.world.block_at(bx, by, bz);
            if aimed == BlockId::CRAFTING_TABLE || aimed == BlockId::FURNACE {
                self.hands.place_timer = PLACE_INTERVAL;
                self.hands.stop();
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
            // A bucket is the one item that moves a block without being that
            // block's item, so it is handled before the ordinary place path.
            if matches!(stack.item, ItemId::BUCKET | ItemId::WATER_BUCKET)
                && self.use_bucket(stack.item, &hit, eye)
            {
                self.hands.place_timer = PLACE_INTERVAL;
                return;
            }

            let Some(id) = stack.item.places() else {
                return;
            };
            let (px, py, pz) = hit.adjacent();
            let (min, max) = self.player.aabb();
            if self.world.place_block(px, py, pz, id, min, max) {
                self.stats.placed += 1;
                self.stats.placed_kinds.push(id.0);
                self.audio.play(
                    audio::Sound::Place(id),
                    audio::PlayOpts::at(
                        Vec3::new(px as f32 + 0.5, py as f32 + 0.5, pz as f32 + 0.5),
                        eye,
                    ),
                );
                self.data.edits.note_set_block(px, py, pz, id);
                let sel = self.data.inventory.selected();
                self.data.inventory.take_from_slot(sel, 1);
                self.hands.place_timer = PLACE_INTERVAL;
            }
        }
    }

    /// Fill an empty bucket from water, or pour a full one out.
    ///
    /// Filling takes the whole block, which the sea then flows back into a
    /// moment later -- so scooping from an ocean looks like scooping from an
    /// ocean, and scooping the last block of a puddle actually empties it.
    /// Returns whether the bucket did anything.
    fn use_bucket(&mut self, item: ItemId, hit: &world::RayHit, eye: Vec3) -> bool {
        let sel = self.data.inventory.selected();
        let (bx, by, bz) = hit.block;
        let at = Vec3::new(bx as f32 + 0.5, by as f32 + 0.5, bz as f32 + 0.5);

        if item == ItemId::BUCKET {
            if self.world.block_at(bx, by, bz) != BlockId::WATER {
                return false;
            }
            self.world.set_block(bx, by, bz, BlockId::AIR);
            self.data.edits.note_set_block(bx, by, bz, BlockId::AIR);
            self.world.disturb_water(bx, by, bz);
            self.data.inventory.take_from_slot(sel, 1);
            self.data.inventory.add_item(ItemId::WATER_BUCKET, 1);
            self.audio.play(
                audio::Sound::Place(BlockId::WATER),
                audio::PlayOpts::at(at, eye),
            );
            return true;
        }

        // Pouring: into the face we are looking at, unless that block is itself
        // replaceable, in which case fill it directly.
        let (px, py, pz) = if self.world.block_at(bx, by, bz).is_replaceable() {
            (bx, by, bz)
        } else {
            hit.adjacent()
        };
        if !self.world.block_at(px, py, pz).is_replaceable() {
            return false;
        }
        self.world.set_block(px, py, pz, BlockId::WATER);
        self.data.edits.note_set_block(px, py, pz, BlockId::WATER);
        self.world.disturb_water(px, py, pz);
        self.data.inventory.take_from_slot(sel, 1);
        self.data.inventory.add_item(ItemId::BUCKET, 1);
        self.audio.play(
            audio::Sound::Place(BlockId::WATER),
            audio::PlayOpts::at(
                Vec3::new(px as f32 + 0.5, py as f32 + 0.5, pz as f32 + 0.5),
                eye,
            ),
        );
        true
    }

    /// Leave whatever panel is open, returning everything on the cursor and in
    /// the crafting grid to the inventory. Closing a menu must never eat items.
    fn close_panel(&mut self) -> bool {
        let returned = match self.ui {
            Ui::Furnace(key) => return_panel_items(
                &mut self.data.inventory,
                &mut self.carried,
                &mut self.craft_grid,
                self.furnaces.get_mut(&key),
            ),
            _ => return_panel_items(
                &mut self.data.inventory,
                &mut self.carried,
                &mut self.craft_grid,
                None,
            ),
        };
        if !returned {
            return false;
        }
        self.ui = Ui::Playing;
        self.set_cursor_locked(true);
        true
    }

    /// A model review stand: flat ground, even light, one of every mob in a row
    /// at reading distance, facing the camera.
    ///
    /// Judging a character alone on a hillside is the hardest possible way to
    /// tell whether it fits the world. This puts them all in one frame, in
    /// context, so the question can be answered in a glance.
    fn run_model_review(&mut self) {
        let base = self.player.pos;
        let gy = self.world.surface_y(base.x as i32, base.z as i32);
        let fwd = Vec3::new(self.camera.yaw.cos(), 0.0, self.camera.yaw.sin());
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);

        // A clean stone platform, so nothing behind the models competes.
        for d in -3..14 {
            for w in -8..9 {
                let p = base + fwd * d as f32 + right * w as f32;
                let (x, z) = (p.x.floor() as i32, p.z.floor() as i32);
                for y in (gy - 2)..=(gy + 6) {
                    let id = if y <= gy {
                        BlockId::STONE
                    } else {
                        BlockId::AIR
                    };
                    self.world.set_block(x, y, z, id);
                }
            }
        }

        let stand = base + fwd * 4.2;
        for (i, kind) in MobKind::ALL.iter().enumerate() {
            let off = (i as f32 - 1.5) * 1.7;
            let p = stand + right * off;
            let m = self.mobs.spawn(*kind, Vec3::new(p.x, gy as f32 + 1.0, p.z));
            // Face the camera and hold still, so the rig is judged in its rest
            // pose and nothing wanders toward the lens before the shutter.
            // Point each mob at the camera by construction rather than by
            // reasoning about yaw conventions: a mob at yaw t faces
            // (cos t, 0, sin t), so the yaw that looks at the camera is just the
            // angle of the vector from the mob to it. `--angle` then turns the
            // whole stand from there, so front, side and back are all reachable.
            // Aim each mob at the camera. The extra half turn is measured, not
            // reasoned: the model's own rotation is asserted correct in
            // model.rs, so this offset belongs to the review stand alone.
            let to_cam = self.camera.pos - Vec3::new(p.x, gy as f32 + 1.0, p.z);
            let face_cam = to_cam.z.atan2(to_cam.x) + std::f32::consts::PI;
            let turn = self.cli.review_angle.to_radians();
            self.mobs.face_and_freeze(m, face_cam + turn);
            self.mobs.pin(m, Vec3::new(p.x, gy as f32 + 1.0, p.z));
        }
        self.player.pos = Vec3::new(base.x, gy as f32 + 1.0, base.z);
        self.camera.pos = self.player.eye();
        self.camera.pitch = -0.05;
        println!("[loudstone] model review: 4 mobs on a platform");
    }

    /// Stage the two headline mechanics in front of the camera so a single
    /// captured frame shows both: a chipped crater, and mobs standing near it.
    /// Render a top-down map of the surface, and quit.
    ///
    /// A screenshot from inside the world shows a couple of hundred blocks of
    /// it, from one angle, hazed at the back. That is the wrong instrument for
    /// judging whether a *river meanders* or whether a coastline has shape: both
    /// are questions about kilometres, and from the ground a river that runs
    /// dead straight for a thousand blocks looks exactly like a river.
    fn run_map(&mut self, path: &std::path::Path, span: i32, step: i32) {
        let n = (span * 2 / step) as u32;
        let mut px = vec![0u8; (n * n * 4) as usize];
        let terrain = &self.world.terrain;
        for iy in 0..n {
            for ix in 0..n {
                let x = -span + ix as i32 * step;
                let z = -span + iy as i32 * step;
                let h = terrain.height_at(x, z);
                let water = h <= world::worldgen::tuning::WATER_LEVEL;
                // Height as brightness, with a shaded relief term so ridges and
                // valleys read as shape rather than as a smooth gradient.
                let west = terrain.height_at(x - step, z);
                let relief = ((h - west) as f32 * 0.10).clamp(-0.35, 0.35);
                let t = ((h - 40) as f32 / 180.0).clamp(0.0, 1.0);
                let rgb = if water {
                    let d =
                        ((world::worldgen::tuning::WATER_LEVEL - h) as f32 / 40.0).clamp(0.0, 1.0);
                    [0.15 - d * 0.1, 0.35 - d * 0.2, 0.70 - d * 0.3]
                } else {
                    let top = terrain.column(x, z).top;
                    let base = match top {
                        BlockId::SAND => [0.86, 0.80, 0.55],
                        BlockId::SNOW => [0.95, 0.96, 0.98],
                        BlockId::STONE | BlockId::ANDESITE | BlockId::GRAVEL => [0.55, 0.55, 0.57],
                        _ => [0.30 + t * 0.25, 0.55 - t * 0.12, 0.25],
                    };
                    [base[0] + relief, base[1] + relief, base[2] + relief]
                };
                let o = ((iy * n + ix) * 4) as usize;
                for c in 0..3 {
                    px[o + c] = (rgb[c].clamp(0.0, 1.0) * 255.0) as u8;
                }
                px[o + 3] = 255;
            }
        }
        match render::screenshot::write_rgba_png(path, n, n, &px) {
            Ok(()) => println!(
                "[map] {n}x{n} covering {} blocks -> {}",
                span * 2,
                path.display()
            ),
            Err(e) => eprintln!("[map] could not write: {e}"),
        }
    }

    /// Stand somewhere high and look out over the land.
    ///
    /// There was previously no way to look at the terrain except to play the
    /// game and walk somewhere with a view, which meant that every judgement
    /// about the *shape* of the world -- whether ridges ripple, whether slopes
    /// terrace, whether mountains are landmarks or wallpaper -- depended on the
    /// player happening to stand in the right place. Those are exactly the
    /// questions a screenshot answers instantly and a close-up never does.
    fn run_vista(&mut self) {
        // Search a spiral of columns for the highest ground within reach, so the
        // camera is not aimed at the inside of a hill.
        let mut best = (0i32, 0i32, i32::MIN);
        let mut r = 0i32;
        while r < 220 {
            r += 20;
            for k in 0..24 {
                let a = k as f32 * std::f32::consts::TAU / 24.0;
                let (x, z) = ((a.cos() * r as f32) as i32, (a.sin() * r as f32) as i32);
                let y = self.world.surface_y(x, z);
                if y > best.2 {
                    best = (x, z, y);
                }
            }
        }
        let (bx, bz, by) = best;
        // Back off from the summit and rise above it: standing exactly on a peak
        // fills half the frame with the peak.
        let eye = Vec3::new(bx as f32 + 0.5, by as f32 + 10.0, bz as f32 + 0.5);
        self.player.pos = eye;
        self.player.vel = Vec3::ZERO;
        self.player.noclip = true;
        self.camera.pos = eye;
        // Look back at the summit, tilted down enough to hold land and sky.
        self.camera.yaw = (bz as f32 - eye.z).atan2(bx as f32 - eye.x);
        self.camera.pitch = -0.22;
        self.time_of_day = DAY_LENGTH * 0.14;
        println!("[vista] looking at the peak at {bx}, {by}, {bz}");
    }

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
            let off = (i as f32 - 1.5) * 1.7;
            let side = Vec3::new(-fwd.z, 0.0, fwd.x) * off;
            let p = eye + fwd * 11.0 + side;
            let y = self.world.surface_y(p.x as i32, p.z as i32) as f32 + 1.0;
            self.mobs.spawn(*kind, Vec3::new(p.x, y, p.z));
        }
        println!("[loudstone] demo: crater carved, 4 mobs spawned");
    }

    fn run_model_demo(&mut self, which: &str) {
        if which != "zombie" {
            eprintln!("[loudstone] unknown model preview: {which}");
            return;
        }
        self.mobs.clear();
        let x = self.player.pos.x.floor() as i32 + 5;
        let z = self.player.pos.z.floor() as i32;
        let y = self.world.surface_y(x, z) as f32 + 1.0;
        let target = Vec3::new(x as f32 + 0.5, y, z as f32 + 0.5);
        self.mobs.spawn(MobKind::Zombie, target);
        self.camera.pos = target + Vec3::new(3.7, 1.18, 2.2);
        let look = target + Vec3::Y * 1.12 - self.camera.pos;
        self.camera.yaw = look.z.atan2(look.x);
        self.camera.pitch = look.y.atan2(Vec3::new(look.x, 0.0, look.z).length());
        self.player.pos = self.camera.pos - Vec3::Y * PLAYER_EYE_HEIGHT;
        self.player.noclip = true;
    }

    /// Sample the game, hand the harness a probe, and carry out what it decides.
    /// Returns a screenshot name when the session asks for one.
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
            // A robot with empty pockets cannot test placing or crafting, and
            // bare hands mine at the slowest possible rate.
            self.gauntlet_stocked = true;
            self.data.inventory.add_item(ItemId::COBBLESTONE, 64);
            self.data.inventory.add_item(ItemId::PLANKS, 32);
            self.data.inventory.add_item(ItemId::STICK, 16);
            self.data.inventory.add_item(ItemId::TORCH, 16);
            self.data.inventory.add_item(ItemId::STONE_PICKAXE, 1);
            self.data.inventory.add_item(ItemId::IRON_AXE, 1);
        }

        let (min, max) = self.player.aabb();
        let nearest_hostile = self
            .mobs
            .mobs()
            .iter()
            .filter(|m| m.kind.is_hostile())
            .map(|m| (m.pos - self.player.pos).length())
            .fold(f32::INFINITY, f32::min);
        let mob_pos_bad = self.mobs.mobs().iter().any(|m| !m.pos.is_finite());

        // Inventory health: totals for the conservation oracle, plus a scan for
        // stacks that are over their limit or sitting at zero.
        let mut inventory_total = 0u32;
        let mut inventory_bad = false;
        for slot in self.data.inventory.slots().iter().flatten() {
            inventory_total += slot.count as u32;
            if slot.count == 0 || slot.count as u32 > 64 {
                inventory_bad = true;
            }
        }

        let eye = self.player.eye();
        let probe = dev::gauntlet::Probe {
            pos: self.player.pos,
            vel: self.player.vel,
            on_ground: self.player.on_ground,
            health: self.data.player.health,
            fps: self.stats.fps,
            chunks: self.world.chunks.len(),
            mobs: self.mobs.mobs().len(),
            mob_pos_bad,
            inside_solid: self.world.box_collides(min, max),
            daylight: daylight_at(self.time_of_day),
            nearest_hostile: nearest_hostile.is_finite().then_some(nearest_hostile),
            biome: self
                .world
                .terrain
                .biome_at(self.player.pos.x as i32, self.player.pos.z as i32)
                as u8,
            light_here: self.world.light_at(
                eye.x.floor() as i32,
                eye.y.floor() as i32,
                eye.z.floor() as i32,
            ),
            inventory_total,
            inventory_bad,
            world_idle: self.world.streaming_idle(),
            relight_pending: self.world.relight_pending(),
            queues: self.world.queue_depths(),
            panel: match self.ui {
                Ui::Title => 0,
                Ui::Playing => 0,
                Ui::Inventory => 1,
                Ui::Table => 2,
                Ui::Furnace(_) => 3,
            },
            carved: self.stats.carved,
            broken: self.stats.broken,
            placed: self.stats.placed,
            crafted: self.stats.crafted,
            mined_kinds: std::mem::take(&mut self.stats.mined_kinds),
            placed_kinds: std::mem::take(&mut self.stats.placed_kinds),
            save_roundtrip: self.save_roundtrip.take(),
        };

        let frame = self.gauntlet.as_mut().unwrap().tick(dt, &probe);

        let Some(frame) = frame else {
            let mut g = self.gauntlet.take().unwrap();
            g.finish();
            g.report();
            self.gauntlet_findings = g.findings.len();
            self.gauntlet_done = true;
            return None;
        };

        self.input = frame.input;
        self.hands.mining = frame.mine;
        self.hands.smash = frame.smash;
        self.hands.placing = frame.place;
        self.camera.yaw += frame.yaw_rate * dt;
        if let Some(pitch) = frame.look_pitch {
            self.camera.pitch = pitch.clamp(-1.5, 1.5);
        }
        if let Some(frac) = frame.set_time_frac {
            self.time_of_day = frac * DAY_LENGTH;
        }
        if let Some(slot) = frame.hotbar {
            self.data.inventory.set_selected(slot);
        }
        match frame.set_inventory {
            Some(true) if self.ui == Ui::Playing => self.ui = Ui::Inventory,
            Some(false) if self.ui.is_panel() => {
                self.close_panel();
            }
            _ => {}
        }
        if frame.attack {
            // Swing at whatever is in front, through the same path a click takes.
            self.hands.mining = true;
        }
        if frame.craft {
            // Fill the 2x2 with planks and take whatever it resolves to. This
            // exercises resolve/consume/add_item together rather than in a test.
            self.craft_grid = [None; 9];
            for c in self.craft_grid.iter_mut().take(4) {
                *c = Some(ItemStack::new(ItemId::PLANKS, 1));
            }
            if let Some(out) = content::crafting::craft(&mut self.craft_grid[..4]) {
                self.data.inventory.add_item(out.item, out.count as u32);
                self.stats.crafted += 1;
            }
            self.craft_grid = [None; 9];
        }
        if let Some((x, z)) = frame.teleport {
            let y = self.world.surface_y(x, z) as f32 + 2.0;
            self.player.pos = Vec3::new(x as f32 + 0.5, y, z as f32 + 0.5);
            self.player.vel = Vec3::ZERO;
            self.camera.pos = self.player.eye();

            // Footsteps are driven by distance covered, not by a timer, so walking
            // and sprinting sound different without any extra bookkeeping.
            let ground_block = {
                let f = self.player.pos;
                self.world.block_at(
                    f.x.floor() as i32,
                    (f.y - 0.2).floor() as i32,
                    f.z.floor() as i32,
                )
            };
            if self.player.on_ground {
                let moved = Vec3::new(self.player.vel.x, 0.0, self.player.vel.z).length() * dt;
                self.step_accum += moved;
                if self.step_accum > 2.2 && !ground_block.is_air() {
                    self.step_accum = 0.0;
                    self.audio.play(
                        audio::Sound::Footstep(ground_block),
                        audio::PlayOpts::at(self.player.pos, self.camera.pos),
                    );
                }
                if !self.was_on_ground && !ground_block.is_air() {
                    // Landing: one firmer step.
                    self.audio.play(
                        audio::Sound::Footstep(ground_block),
                        audio::PlayOpts::at(self.player.pos, self.camera.pos).with_volume(1.5),
                    );
                }
            }
            self.was_on_ground = self.player.on_ground;
            self.player.unstick(&self.world);
        }
        if frame.save_check {
            self.save_roundtrip = Some(self.check_save_roundtrip());
        }
        for i in 0..frame.spawn_hostiles {
            let a = i as f32 * std::f32::consts::TAU / frame.spawn_hostiles.max(1) as f32;
            let p = self.player.pos + Vec3::new(a.cos() * 9.0, 0.0, a.sin() * 9.0);
            let y = self.world.surface_y(p.x as i32, p.z as i32) as f32 + 1.0;
            let kind = MobKind::HOSTILES[i % MobKind::HOSTILES.len()];
            self.mobs.spawn(kind, Vec3::new(p.x, y, p.z));
        }
        frame.shot
    }

    /// Write the save, read it back, and check it describes the same world.
    /// This is the one oracle that cannot be judged from inside the running
    /// game -- a save that silently loses edits looks perfect until you reload.
    fn check_save_roundtrip(&mut self) -> bool {
        self.data.player.pos = self.player.pos;
        capture_live_state(&mut self.data, &self.mobs, &self.furnaces, self.player.pos);
        let path = std::env::temp_dir().join("loudstone_gauntlet_roundtrip.lsw");
        if persist::save::save_to_file(&path, &self.data).is_err() {
            return false;
        }
        match persist::save::load_from_file(&path) {
            Ok(back) => {
                let mut same = back.seed == self.data.seed
                    && back.player.pos == self.data.player.pos
                    && back.edits.modified_chunk_count() == self.data.edits.modified_chunk_count()
                    && back.inventory.slots() == self.data.inventory.slots()
                    && back.mobs == self.data.mobs
                    && back.containers == self.data.containers;
                // Every recorded block edit must come back identical. Counting
                // chunks alone would not notice a delta that loaded empty.
                for (key, _) in self.data.edits.chunks() {
                    if back.edits.chunk(*key).is_none() {
                        same = false;
                        break;
                    }
                }
                let _ = std::fs::remove_file(&path);
                same
            }
            Err(_) => false,
        }
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
            let mut f = content::crafting::Furnace::new();
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
    fn note_carves(&mut self, block: (i32, i32, i32)) {
        let (bx, by, bz) = block;
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    if !self.world.sub_solid(bx, by, bz, sx, sy, sz) {
                        self.data.edits.note_carve(bx, by, bz, sx, sy, sz);
                    }
                }
            }
        }
    }

    /// A furnace block just stopped existing at `pos`. Drop its stored contents
    /// into the player's pack, since they broke it deliberately and the items
    /// have nowhere else to go -- there are no item entities in this game to
    /// scatter them onto the ground. Anything that will not fit is lost.
    fn break_furnace_at(&mut self, pos: persist::save::BlockPos) {
        let world = &self.world;
        let salvage = forget_orphan_furnaces(&mut self.furnaces, pos, pos, |x, y, z| {
            world.block_at(x, y, z)
        });
        for stack in salvage {
            self.data.inventory.add_item(stack.item, stack.count as u32);
        }
    }

    fn grant_drop(&mut self, tool: Option<ItemId>, broken: BlockId) {
        if let Some(dropped) = content::item::mining_drop(tool, broken) {
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
        if self.ui == Ui::Playing && self.cli.model_demo.is_none() {
            self.player
                .update(&self.world, &self.camera, &self.input, dt);
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
            .arrived
            .iter()
            .copied()
            .filter(|p| !self.replayed.contains(p))
            .collect();
        if let Some(gfx) = self.gfx.as_mut() {
            gfx.apply_stream(&stream.dropped, stream.ready);
        }
        replay_arrived_chunks(
            &mut self.data,
            &mut self.world,
            &mut self.replayed,
            &arrived,
        );

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
            self.sound.emit(sim::sound::NoiseEvent {
                pos: ev.pos,
                loudness: ev.loudness,
            });
        }
        self.sound.update(&self.world, dt);

        let mut pstate = PlayerState::new(self.player.pos, PLAYER_EYE_HEIGHT);
        pstate.alive = self.data.player.health > 0.0;
        let events = {
            let mut world = TrackedWorld {
                world: &mut self.world,
                edits: &mut self.data.edits,
            };
            self.mobs
                .update(&mut world, &mut self.sound, &pstate, daylight, dt)
        };

        let eye_now = self.camera.pos;
        for ev in events {
            match ev {
                MobEvent::PlayerDamaged { amount, .. } => {
                    self.data.player.health -= amount;
                    self.audio
                        .play(audio::Sound::PlayerHurt, audio::PlayOpts::ui());
                }
                // Mob drops (meat, bone, gunpowder) have no item counterpart:
                // hunger and brewing are deliberately out of scope, so there is
                // nothing for them to become. The events are left in place so a
                // later build can give them one.
                MobEvent::Exploded { pos, radius } => {
                    self.audio
                        .play(audio::Sound::Explosion, audio::PlayOpts::at(pos, eye_now));
                    // A blast can take a furnace with it. Whatever was inside is
                    // gone: teleporting it into the player's pack from across the
                    // map would be stranger than losing it, and leaving the entry
                    // behind is the duplication bug this call exists to prevent.
                    let r = radius.ceil() as i32 + 1;
                    let b = pos.floor();
                    let (bx, by, bz) = (b.x as i32, b.y as i32, b.z as i32);
                    let world = &self.world;
                    forget_orphan_furnaces(
                        &mut self.furnaces,
                        (bx - r, by - r, bz - r),
                        (bx + r, by + r, bz + r),
                        |x, y, z| world.block_at(x, y, z),
                    );
                }
                MobEvent::MobDied { pos, .. } => {
                    self.audio
                        .play(audio::Sound::MobDeath, audio::PlayOpts::at(pos, eye_now));
                }
                // Mob drops have no item counterpart: hunger and brewing are
                // deliberately out of scope, so there is nothing to become.
                MobEvent::Drop { .. } => {}
            }
        }

        for m in self.mobs.mobs() {
            if m.fuse_fraction() > 0.0 {
                self.audio.play(
                    audio::Sound::CreeperFuse,
                    audio::PlayOpts::at(m.pos, eye_now),
                );
            }
        }

        if self.data.player.health <= 0.0 {
            self.audio.play(
                audio::Sound::PlayerHurt,
                audio::PlayOpts::ui().with_pitch(0.6),
            );
            self.respawn();
        }

        // Furnaces keep working whether or not anyone is looking at them.
        for f in self.furnaces.values_mut() {
            f.tick(dt);
        }

        // --- autosave ---
        self.time_since_save += dt;
        // Panel stacks temporarily live outside SaveData. Keep the last complete
        // save until the panel closes rather than writing an incomplete snapshot.
        if persist::save::should_autosave(self.time_since_save) && !self.ui.is_panel() {
            self.save_now();
        }
    }

    /// Rebuild the overlay and the mob geometry, then draw.
    fn draw(&mut self) {
        let daylight = daylight_at(self.time_of_day);
        let sky = sky_for(daylight);

        // Entity geometry is rebuilt into growable buffers each frame. Every
        // mob now uses the same six-box humanoid rig and differs only by skin,
        // so adding a creature costs a palette and a pose flag, not geometry.
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for m in self.mobs.mobs() {
            // Mobs take the light of the block they stand in, so one in a cave
            // is not lit like one in a field.
            let l = self.world.effective_light_at(
                m.pos.x.floor() as i32,
                (m.pos.y + 1.0).floor() as i32,
                m.pos.z.floor() as i32,
                daylight,
            );
            let light = world::light::brightness(l as f32).max(0.12);
            append_mob_model(&mut verts, &mut indices, m, light);
        }

        for p in self.mobs.projectiles() {
            render::gfx::Renderer::box_geometry(
                &mut verts,
                &mut indices,
                p.pos - Vec3::splat(0.06),
                p.pos + Vec3::splat(0.06),
                [0.85, 0.85, 0.88],
                render::texture::T_WHITE,
            );
        }

        let Some(gfx) = self.gfx.as_mut() else { return };
        gfx.set_entities(&verts, &indices);

        let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
        gfx.hud.begin(w, h);

        if self.ui == Ui::Title {
            draw_title_screen(gfx, self.has_save, self.cursor);
            gfx.render(&self.camera, [0.025, 0.045, 0.075], sun_for(0.0));
            return;
        }

        // Hotbar, built from the inventory.
        let slots: Vec<Option<Slot>> = self
            .data
            .inventory
            .hotbar()
            .iter()
            .map(|s| {
                s.map(|st| {
                    Slot::new(
                        render::texture::item_tile(st.item),
                        [1.0; 3],
                        st.count as u16,
                    )
                })
            })
            .collect();
        if self.ui == Ui::Playing && self.cli.model_demo.is_none() {
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
        if self.cli.model_demo.is_none() {
            gfx.hud.readout(
                self.player.pos.to_array(),
                self.stats.fps,
                &format!(
                    "{held}  |  {}  |  {}",
                    if self.hands.smash {
                        "SMASH (loud)"
                    } else {
                        "chip (quiet)"
                    },
                    if daylight > 0.5 { "day" } else { "NIGHT" }
                ),
            );
        }

        if self.loading {
            gfx.hud.text_shadowed(
                w * 0.5 - 60.0,
                h * 0.5 - 40.0,
                render::hud::TEXT_SIZE,
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

        gfx.render(&self.camera, sky, sun_for(self.time_of_day));
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
                        let mut candidate = f.clone();
                        if let Some(out) = candidate.take_output() {
                            if store_output(&mut self.data.inventory, &mut self.carried, out) {
                                f = candidate;
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
                let mut candidate = self.craft_grid;
                if let Some(out) = content::crafting::craft(&mut candidate[..cells]) {
                    if store_output(&mut self.data.inventory, &mut self.carried, out) {
                        self.craft_grid = candidate;
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

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Loudstone")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let renderer = pollster::block_on(render::gfx::Renderer::new(window.clone()));
        self.camera.aspect = renderer.config.width as f32 / renderer.config.height as f32;
        self.gfx = Some(renderer);
        self.window = Some(window);
        self.set_cursor_locked(self.ui != Ui::Title);
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
                if self.ui == Ui::Title {
                    event_loop.exit();
                    return;
                }
                if self.ui.is_panel() && !self.close_panel() {
                    eprintln!("[loudstone] cannot close while panel items have nowhere safe to go");
                    return;
                }
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
                if self.ui == Ui::Title {
                    return;
                }
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
                if self.ui == Ui::Title {
                    if pressed && button == MouseButton::Left {
                        let action = self.gfx.as_ref().and_then(|gfx| {
                            title_action(
                                gfx.config.width as f32,
                                gfx.config.height as f32,
                                self.cursor,
                                self.has_save,
                            )
                        });
                        match action {
                            Some(TitleAction::Continue) => self.enter_world(),
                            Some(TitleAction::NewWorld) => self.start_new_world(),
                            Some(TitleAction::Quit) => event_loop.exit(),
                            None => {}
                        }
                    }
                    return;
                }
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
                    MouseButton::Left => {
                        if pressed {
                            self.hands.mining = true;
                        } else {
                            self.hands.stop();
                        }
                    }
                    MouseButton::Right => self.hands.placing = pressed,
                    _ => {}
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    if self.ui == Ui::Title {
                        if pressed {
                            match code {
                                KeyCode::Enter if self.has_save => self.enter_world(),
                                KeyCode::Enter | KeyCode::KeyN => self.start_new_world(),
                                KeyCode::Escape | KeyCode::KeyQ => event_loop.exit(),
                                _ => {}
                            }
                        }
                        return;
                    }
                    match code {
                        KeyCode::KeyW => self.input.fwd = pressed,
                        KeyCode::KeyS => self.input.back = pressed,
                        KeyCode::KeyA => self.input.left = pressed,
                        KeyCode::KeyD => self.input.right = pressed,
                        KeyCode::Space => self.input.up = pressed,
                        KeyCode::ShiftLeft => self.input.down = pressed,
                        KeyCode::ControlLeft => self.input.fast = pressed,
                        KeyCode::AltLeft => self.hands.smash = pressed,
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
                                // Opening a panel releases the hands. This used
                                // to clear the buttons but leave the committed
                                // mining target set, so closing the panel and
                                // clicking resumed on a block the player might
                                // have walked away from.
                                self.hands.stop();
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
                // A gauntlet session runs on a fixed step, not on wall-clock
                // frame times. The robot's choices come from a seeded RNG drawn
                // once per frame, so with a real `dt` the same seed makes a
                // different number of draws on a busy machine than on an idle
                // one and replays a different session -- which makes the printed
                // "reproduce with --seed N" untrue exactly when it matters, on
                // the rare failure you are trying to chase down.
                let dt = if self.gauntlet.is_some() {
                    GAUNTLET_STEP
                } else {
                    (now - self.last_frame_at).as_secs_f32().min(0.1)
                };
                self.last_frame_at = now;

                let gauntlet_shot = if self.ui == Ui::Title {
                    None
                } else {
                    let shot = self.drive_gauntlet(dt);
                    self.update(dt);
                    shot
                };
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

                if self.stats.note_frame(dt) {
                    if let Some(w) = &self.window
                        && self.ui != Ui::Title
                    {
                        w.set_title(&format!(
                            "Loudstone  |  {:.0} fps  |  {} chunks  |  {} mobs",
                            self.stats.fps,
                            self.world.chunks.len(),
                            self.mobs.mobs().len()
                        ));
                    }
                }

                self.draw();

                // `--shot <path>`: wait for the world to finish streaming, give
                // it a few frames to settle, capture, and quit.
                if let Some(path) = self.cli.shot_path.clone() {
                    if self.ui == Ui::Title || !self.loading {
                        self.shot_countdown -= 1;
                        if self.shot_countdown == 60 {
                            if self.cli.models_review {
                                self.run_model_review();
                            }
                            if let Some(map) = self.cli.map_path.clone() {
                                let span = self.cli.map_span;
                                self.run_map(&map, span, (span / 512).max(1));
                                event_loop.exit();
                                return;
                            }
                            if self.cli.vista {
                                self.run_vista();
                            }
                            if self.cli.demo {
                                self.run_demo();
                            }
                            if let Some(which) = self.cli.ui_demo.clone() {
                                self.run_ui_demo(&which);
                            }
                        }
                        if self.shot_countdown == 2 {
                            if let Some(which) = self.cli.model_demo.clone() {
                                self.run_model_demo(&which);
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
    if let Err(error) = content::registry::init() {
        eprintln!("[loudstone] {error}");
        eprintln!("[loudstone] falling back to the built-in content data");
    }
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
            content::crafting::resolve(&two).is_none(),
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
        let out = content::crafting::resolve(&three).expect("3x3 must resolve a stone pickaxe");
        assert_eq!(out.item, ItemId::STONE_PICKAXE);
    }

    #[test]
    fn the_full_progression_from_bare_hands_to_an_iron_ingot() {
        let mut inv = content::inventory::Inventory::new();

        // Punch a tree: wood -> planks -> sticks, both 2x2 recipes.
        inv.add_item(ItemId::WOOD, 2);
        let mut g = [None; 9];
        g[0] = stack(ItemId::WOOD, 1);
        let planks = content::crafting::craft(&mut g[..4]).expect("wood makes planks");
        assert_eq!(planks.item, ItemId::PLANKS);
        inv.add_item(planks.item, planks.count as u32);

        // A crafting table is itself a 2x2 recipe, which is what unlocks 3x3.
        let mut g = [None; 9];
        for c in g.iter_mut().take(4) {
            *c = stack(ItemId::PLANKS, 1);
        }
        let table = content::crafting::craft(&mut g[..4]).expect("four planks make a table");
        assert_eq!(table.item, ItemId::CRAFTING_TABLE);

        // At the table: a stone pickaxe.
        let mut g = [None; 9];
        for c in g.iter_mut().take(3) {
            *c = stack(ItemId::COBBLESTONE, 1);
        }
        g[4] = stack(ItemId::STICK, 1);
        g[7] = stack(ItemId::STICK, 1);
        let pick = content::crafting::craft(&mut g[..9]).expect("the table makes a stone pickaxe");
        assert_eq!(pick.item, ItemId::STONE_PICKAXE);

        // That pickaxe is exactly what iron ore requires -- a wooden one is not.
        assert!(content::item::can_harvest(
            Some(ItemId::STONE_PICKAXE),
            BlockId::IRON_ORE
        ));
        assert!(!content::item::can_harvest(
            Some(ItemId::WOODEN_PICKAXE),
            BlockId::IRON_ORE
        ));
        assert_eq!(
            content::item::mining_drop(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE),
            Some(ItemId::RAW_IRON)
        );

        // Smelt it. Coal is the fuel; the furnace ticks on its own.
        let mut f = content::crafting::Furnace::new();
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
        let iron_pick = content::crafting::craft(&mut g[..9]).expect("ingots make an iron pickaxe");
        assert_eq!(iron_pick.item, ItemId::IRON_PICKAXE);
        assert!(content::item::can_harvest(
            Some(ItemId::IRON_PICKAXE),
            BlockId::DIAMOND_ORE
        ));
        assert!(!content::item::can_harvest(
            Some(ItemId::STONE_PICKAXE),
            BlockId::DIAMOND_ORE
        ));

        let _ = inv.slot(0);
    }

    #[test]
    fn a_furnace_will_not_burn_fuel_with_nothing_to_smelt() {
        let mut f = content::crafting::Furnace::new();
        f.fuel = stack(ItemId::COAL, 1);
        for _ in 0..600 {
            f.tick(1.0 / 60.0);
        }
        assert!(!f.is_burning(), "an idle furnace must not waste its fuel");
        assert_eq!(f.fuel.map(|s| s.count), Some(1));
    }

    /// The duplication exploit, stated as a test: mine a furnace, and its
    /// contents must not survive to be inherited by the next one built there.
    #[test]
    fn a_broken_furnace_hands_its_contents_back_and_leaves_nothing_behind() {
        let pos = (8, 64, -4);
        let mut furnace = content::crafting::Furnace::new();
        furnace.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        furnace.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        furnace.tick(2.0);
        // Ask the furnace what it is holding rather than assuming: two seconds
        // in, the coal has already been consumed into the flame and some of the
        // iron may have become an ingot.
        let mut expected: Vec<_> = [furnace.input, furnace.fuel, furnace.output]
            .into_iter()
            .flatten()
            .collect();
        assert!(!expected.is_empty(), "the test furnace must hold something");
        let mut furnaces = HashMap::from([(pos, furnace)]);

        // The block is gone; the map entry has not caught up yet.
        let mut salvage = forget_orphan_furnaces(&mut furnaces, pos, pos, |_, _, _| BlockId::AIR);

        assert!(
            furnaces.is_empty(),
            "an orphan entry here is a save that grows forever and a furnace              that resurrects its contents for whoever builds on the spot next"
        );
        expected.sort_by_key(|s| s.item.0);
        salvage.sort_by_key(|s| s.item.0);
        assert_eq!(salvage, expected, "everything inside should come back out");
    }

    #[test]
    fn a_furnace_that_is_still_standing_is_left_alone() {
        let pos = (8, 64, -4);
        let mut furnaces = HashMap::from([(pos, content::crafting::Furnace::new())]);
        let salvage = forget_orphan_furnaces(&mut furnaces, pos, pos, |_, _, _| BlockId::FURNACE);
        assert_eq!(furnaces.len(), 1);
        assert!(salvage.is_empty());
    }

    /// The reason the sweep is bounded to a region. Outside the loaded area
    /// `block_at` answers from raw terrain, which never contains a furnace, so
    /// an unbounded sweep would quietly eat every furnace the player owns the
    /// moment they walked away from one.
    #[test]
    fn a_furnace_outside_the_swept_region_is_never_touched() {
        let far = (900, 64, -900);
        let mut furnaces = HashMap::from([(far, content::crafting::Furnace::new())]);
        forget_orphan_furnaces(&mut furnaces, (0, 0, 0), (16, 80, 16), |_, _, _| {
            BlockId::AIR
        });
        assert_eq!(
            furnaces.len(),
            1,
            "a furnace in an unloaded chunk must survive a sweep somewhere else"
        );
    }

    #[test]
    fn live_mobs_and_furnaces_cross_the_save_boundary() {
        let player_pos = Vec3::new(4.0, 70.0, -3.0);
        let mut live_mobs = MobManager::new(9);
        live_mobs.spawning_enabled = false;
        live_mobs.spawn(MobKind::Pig, player_pos + Vec3::X);

        let furnace_pos = (8, 64, -4);
        let mut furnace = content::crafting::Furnace::new();
        furnace.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        furnace.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        furnace.tick(2.0);
        let live_furnaces = std::collections::HashMap::from([(furnace_pos, furnace)]);

        let mut data = persist::save::SaveData::new(9);
        capture_live_state(&mut data, &live_mobs, &live_furnaces, player_pos);
        assert_eq!(data.mobs.len(), 1);
        assert!(data.container(furnace_pos).is_some());

        let mut restored_mobs = MobManager::new(9);
        restored_mobs.spawning_enabled = false;
        restore_mobs(&data, &mut restored_mobs);
        let restored_furnaces = restore_furnaces(&data);

        assert_eq!(restored_mobs.len(), 1);
        assert_eq!(restored_mobs.mobs()[0].kind, MobKind::Pig);
        assert_eq!(
            restored_furnaces[&furnace_pos].input,
            live_furnaces[&furnace_pos].input
        );
        assert_eq!(
            restored_furnaces[&furnace_pos].progress_fraction(),
            live_furnaces[&furnace_pos].progress_fraction()
        );
    }

    #[test]
    fn mob_world_mutations_are_written_to_the_durable_edit_log() {
        let mut world = World::new(17);
        assert!(world.ensure(ChunkPos::new(0, 4, 0)));
        world.set_block(1, 65, 1, BlockId::STONE);
        let mut edits = persist::save::ChangeTracker::new();

        {
            let mut tracked = TrackedWorld {
                world: &mut world,
                edits: &mut edits,
            };
            sim::sound::VoxelWorld::carve(&mut tracked, 1, 65, 1, 0, 0, 0);
            sim::sound::VoxelWorld::set_block(&mut tracked, 2, 65, 1, BlockId::AIR);
        }

        assert!(edits.mask_at(1, 65, 1).is_some());
        assert_eq!(edits.block_at(2, 65, 1), Some(BlockId::AIR));
    }

    #[test]
    fn an_actual_explosion_is_written_to_the_durable_edit_log() {
        let mut world = World::new(18);
        assert!(world.ensure(ChunkPos::new(0, 4, 0)));
        for x in 5..=11 {
            for y in 62..=68 {
                for z in 5..=11 {
                    world.set_block(x, y, z, BlockId::STONE);
                }
            }
        }
        let mut edits = persist::save::ChangeTracker::new();

        let report = {
            let mut tracked = TrackedWorld {
                world: &mut world,
                edits: &mut edits,
            };
            sim::mob::explode(&mut tracked, Vec3::new(8.5, 65.5, 8.5), 2.5)
        };

        assert!(report.blocks_destroyed + report.blocks_damaged > 0);
        assert!(!edits.is_empty());
    }

    #[test]
    fn saved_edits_replay_when_an_empty_chunk_arrives_without_a_mesh() {
        let pos = ChunkPos::new(0, 10, 0);
        let mut world = World::new(23);
        world
            .chunks
            .insert(pos, Arc::new(world::chunk::Chunk::new(pos)));
        let mut data = persist::save::SaveData::new(23);
        data.edits
            .note_set_block(1, pos.y * CHUNK_SIZE_I + 2, 1, BlockId::PLANKS);
        let mut replayed = HashSet::new();

        replay_arrived_chunks(&mut data, &mut world, &mut replayed, &[pos]);

        assert_eq!(
            world.block_at(1, pos.y * CHUNK_SIZE_I + 2, 1),
            BlockId::PLANKS
        );
        assert!(replayed.contains(&pos));
    }

    #[test]
    fn closing_a_full_furnace_panel_returns_the_cursor_stack_to_the_furnace() {
        let mut inventory = content::inventory::Inventory::new();
        for index in 0..content::inventory::SLOT_COUNT {
            inventory.set_slot(index, Some(ItemStack::new(ItemId::DIRT, 64)));
        }
        let coal = ItemStack::new(ItemId::COAL, 8);
        let mut carried = Some(coal);
        let mut grid = [None; 9];
        let mut furnace = content::crafting::Furnace::new();

        assert!(return_panel_items(
            &mut inventory,
            &mut carried,
            &mut grid,
            Some(&mut furnace)
        ));
        assert!(carried.is_none());
        assert_eq!(furnace.input, Some(coal));
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
