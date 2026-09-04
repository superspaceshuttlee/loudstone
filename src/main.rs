//! Loudstone -- a voxel survival game.
//!
//! Two mechanics define it: blocks are internally 8^3 sub-voxel grids that can be
//! chipped away progressively, and mining emits noise that hostile mobs hear.
//! Chipping is quiet and slow, smashing is fast and loud.

mod block;
mod camera;
mod chunk;
mod config;
mod gfx;
mod mesh;
mod world;
mod worldgen;

use camera::{Camera, FlyInput};
use chunk::ChunkPos;
use config::{CHUNK_SIZE_I, RENDER_DISTANCE, SKY_COLOR};
use glam::Vec3;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};
use world::World;

/// How many chunks may be generated in a single frame. Generation is the expensive
/// half, so this is the knob that trades load speed against frame hitching.
const GEN_BUDGET_PER_FRAME: usize = 4;
/// How many chunk meshes may be rebuilt and uploaded in a single frame.
const MESH_BUDGET_PER_FRAME: usize = 6;

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<gfx::Renderer>,
    world: World,
    camera: Camera,
    input: FlyInput,
    cursor_locked: bool,
    last_frame: Instant,
    fps_accum: f32,
    fps_frames: u32,
    loaded_center: Option<ChunkPos>,
}

impl App {
    fn new() -> Self {
        let seed = 1337;
        let world = World::new(seed);
        // Drop the camera in above the terrain at the origin column.
        let surface = world.surface_y(0, 0) as f32;
        let camera = Camera::new(Vec3::new(0.5, surface + 12.0, 0.5));
        Self {
            window: None,
            gfx: None,
            world,
            camera,
            input: FlyInput::default(),
            cursor_locked: false,
            last_frame: Instant::now(),
            fps_accum: 0.0,
            fps_frames: 0,
            loaded_center: None,
        }
    }

    fn camera_chunk(&self) -> ChunkPos {
        ChunkPos::new(
            (self.camera.pos.x as i32).div_euclid(CHUNK_SIZE_I),
            (self.camera.pos.y as i32).div_euclid(CHUNK_SIZE_I),
            (self.camera.pos.z as i32).div_euclid(CHUNK_SIZE_I),
        )
    }

    fn set_cursor_locked(&mut self, locked: bool) {
        let Some(window) = &self.window else { return };
        if locked {
            // Locked is unsupported on Windows; Confined plus raw mouse deltas
            // gives the same result.
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

    /// Stream chunks in and out around the camera, then remesh whatever is stale.
    fn update_world(&mut self) {
        let center = self.camera_chunk();
        let Some(gfx) = self.gfx.as_mut() else { return };

        // Unload first so memory does not spike while moving.
        if self.loaded_center != Some(center) {
            for pos in self.world.unload_far(center, RENDER_DISTANCE + 2) {
                gfx.drop_mesh(&pos);
            }
            self.loaded_center = Some(center);
        }

        let mut generated = 0;
        for pos in self.world.wanted_chunks(center, RENDER_DISTANCE) {
            if generated >= GEN_BUDGET_PER_FRAME {
                break;
            }
            if self.world.ensure(pos) {
                generated += 1;
            }
        }

        let mut meshed = 0;
        for pos in self.world.dirty_chunks(center) {
            if meshed >= MESH_BUDGET_PER_FRAME {
                break;
            }
            if let Some((verts, indices)) = self.world.mesh_now(pos) {
                gfx.upload_mesh(pos, &verts, &indices);
                self.world.clear_dirty(pos);
                meshed += 1;
            }
        }
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
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.cursor_locked {
                self.camera.look(delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size);
                    self.camera.aspect = size.width as f32 / size.height.max(1) as f32;
                }
            }

            WindowEvent::MouseInput { state, .. } => {
                if state == ElementState::Pressed && !self.cursor_locked {
                    self.set_cursor_locked(true);
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
                        KeyCode::Escape if pressed => self.set_cursor_locked(false),
                        _ => {}
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;

                self.input.apply(&mut self.camera, dt);
                self.update_world();

                self.fps_accum += dt;
                self.fps_frames += 1;
                if self.fps_accum >= 0.5 {
                    let fps = self.fps_frames as f32 / self.fps_accum;
                    if let Some(w) = &self.window {
                        w.set_title(&format!(
                            "Loudstone  |  {:.0} fps  |  chunks {}  |  xyz {:.0} {:.0} {:.0}",
                            fps,
                            self.world.chunks.len(),
                            self.camera.pos.x,
                            self.camera.pos.y,
                            self.camera.pos.z
                        ));
                    }
                    self.fps_accum = 0.0;
                    self.fps_frames = 0;
                }

                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.render(&self.camera, SKY_COLOR);
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

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop");
}
