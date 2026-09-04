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
mod gfx;
mod hud;
mod inventory;
mod item;
mod mesh;
mod mob;
mod pathfind;
mod save;
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

/// Which on-screen panel has focus. The cursor is only released for the menu.
#[derive(PartialEq, Copy, Clone)]
enum Ui {
    Playing,
    Inventory,
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
    craft_grid: [Option<ItemStack>; 4],
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

    time_of_day: f32,
    spawn: Vec3,
    last_frame: Instant,
    fps: f32,
    fps_accum: f32,
    fps_frames: u32,
    /// Set while the streamer is still filling the initial radius.
    loading: bool,
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
            craft_grid: [None; 4],
            carried: None,
            cursor: (0.0, 0.0),
            cursor_locked: false,
            mining: false,
            placing: false,
            smash_mode: false,
            chip_timer: 0.0,
            place_timer: 0.0,
            time_of_day: 0.0,
            spawn,
            last_frame: Instant::now(),
            fps: 0.0,
            fps_accum: 0.0,
            fps_frames: 0,
            loading: true,
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

        let hit = self
            .world
            .raycast(self.camera.pos, self.camera.forward(), REACH);

        if let Some(gfx) = self.gfx.as_mut() {
            gfx.set_highlight(hit.map(|h| h.block));
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
                    self.world.chip_sphere(&hit, CHIP_RADIUS);
                    // Record what was carved so it survives streaming and saving.
                    self.note_carves(&hit);
                    if self.world.block_at(bx, by, bz).is_air() && !before.is_air() {
                        self.grant_drop(tool, before);
                    }
                }
            }
        }

        if self.placing && self.place_timer <= 0.0 {
            let Some(stack) = self.data.inventory.selected_stack() else {
                return;
            };
            let Some(id) = stack.item.places() else { return };
            let (px, py, pz) = hit.adjacent();
            let (min, max) = self.player.aabb();
            if self.world.place_block(px, py, pz, id, min, max) {
                self.data.edits.note_set_block(px, py, pz, id);
                let sel = self.data.inventory.selected();
                self.data.inventory.take_from_slot(sel, 1);
                self.place_timer = PLACE_INTERVAL;
            }
        }
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
        self.time_of_day += dt;
        let daylight = daylight_at(self.time_of_day);

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

        if self.loading && self.world.is_idle() {
            self.loading = false;
            println!(
                "[loudstone] world ready in {:.2}s ({} chunks)",
                self.start.elapsed().as_secs_f32(),
                self.world.chunks.len()
            );
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
        gfx.hud.hotbar(&slots, self.data.inventory.selected());
        gfx.hud.health(self.data.player.health, 20.0);
        if self.ui == Ui::Playing {
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
                2.0,
                [1.0, 1.0, 1.0, 1.0],
                "loading world",
            );
        }

        if self.ui == Ui::Inventory {
            draw_inventory(
                gfx,
                &self.data.inventory,
                &self.craft_grid,
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

fn craft_rect(w: f32, h: f32, index: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let cx = ox + 5.0 * (SLOT_PX + SLOT_GAP);
    let cy = oy - 2.2 * (SLOT_PX + SLOT_GAP);
    (
        cx + (index % 2) as f32 * (SLOT_PX + SLOT_GAP),
        cy + (index / 2) as f32 * (SLOT_PX + SLOT_GAP),
    )
}

fn craft_output_rect(w: f32, h: f32) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    (
        ox + 7.6 * (SLOT_PX + SLOT_GAP),
        oy - 1.7 * (SLOT_PX + SLOT_GAP),
    )
}

fn draw_inventory(
    gfx: &mut gfx::Renderer,
    inv: &inventory::Inventory,
    grid: &[Option<ItemStack>; 4],
    carried: Option<ItemStack>,
    cursor: (f32, f32),
) {
    let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
    gfx.hud.screen_dim();
    let (ox, oy) = inv_origin(w, h);
    let gw = 9.0 * SLOT_PX + 8.0 * SLOT_GAP;
    gfx.hud.panel(
        ox - 16.0,
        oy - 3.6 * (SLOT_PX + SLOT_GAP),
        gw + 32.0,
        5.6 * (SLOT_PX + SLOT_GAP) + 40.0,
    );

    let to_slot = |s: Option<ItemStack>| s.map(|st| Slot::new(st.item.color(), st.count as u16));

    for i in 0..36 {
        let (x, y) = slot_rect(w, h, i);
        gfx.hud
            .slot(x, y, SLOT_PX, to_slot(inv.slot(i)), i == inv.selected());
    }
    for i in 0..4 {
        let (x, y) = craft_rect(w, h, i);
        gfx.hud.slot(x, y, SLOT_PX, to_slot(grid[i]), false);
    }
    let (cx, cy) = craft_output_rect(w, h);
    gfx.hud
        .slot(cx, cy, SLOT_PX, to_slot(crafting::resolve(grid)), false);
    gfx.hud.text_shadowed(
        ox,
        oy - 3.3 * (SLOT_PX + SLOT_GAP),
        1.4,
        [0.9, 0.9, 0.92, 1.0],
        "INVENTORY   [E] close   click to move   click output to craft",
    );

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
    /// Click handling for the inventory screen. Returns true if it consumed the click.
    fn inventory_click(&mut self, right: bool) -> bool {
        let Some(gfx) = self.gfx.as_ref() else {
            return false;
        };
        let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
        let (mx, my) = self.cursor;
        let inside = |x: f32, y: f32, s: f32| mx >= x && mx < x + s && my >= y && my < y + s;

        // The crafting output: take the result and spend the ingredients.
        let (cx, cy) = craft_output_rect(w, h);
        if inside(cx, cy, SLOT_PX) {
            if let Some(out) = crafting::craft(&mut self.craft_grid) {
                let spilled = self.data.inventory.add_item(out.item, out.count as u32);
                if spilled > 0 {
                    // No room: put it in the hand instead of destroying it.
                    self.carried = Some(ItemStack::new(out.item, spilled as u8));
                }
            }
            return true;
        }

        for i in 0..4 {
            let (x, y) = craft_rect(w, h, i);
            if inside(x, y, SLOT_PX) {
                swap_carried(&mut self.carried, &mut self.craft_grid[i], right);
                return true;
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
                if self.ui == Ui::Inventory {
                    if pressed {
                        self.inventory_click(button == MouseButton::Right);
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
                        KeyCode::KeyF if pressed => {
                            self.player.noclip = !self.player.noclip;
                            if !self.player.noclip {
                                self.player.unstick(&self.world);
                            }
                        }
                        KeyCode::KeyE if pressed => {
                            self.ui = if self.ui == Ui::Inventory {
                                self.set_cursor_locked(true);
                                Ui::Playing
                            } else {
                                self.set_cursor_locked(false);
                                self.mining = false;
                                self.placing = false;
                                Ui::Inventory
                            };
                        }
                        KeyCode::Escape if pressed => {
                            self.ui = Ui::Playing;
                            self.set_cursor_locked(false);
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

                self.update(dt);

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
