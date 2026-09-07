//! Camera, the player's physical body, and the GPU camera uniform.

use crate::config::*;
use crate::world::World;
use glam::{Mat4, Vec3};

pub struct Camera {
    pub pos: Vec3,
    /// Radians. 0 looks down +X.
    pub yaw: f32,
    /// Radians, clamped just short of straight up/down.
    pub pitch: f32,
    pub aspect: f32,
}

impl Camera {
    pub fn new(pos: Vec3) -> Self {
        Self {
            pos,
            yaw: 0.0,
            pitch: -0.3,
            aspect: 16.0 / 9.0,
        }
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
        .normalize()
    }

    /// Forward projected onto the ground plane, for walk-style movement.
    pub fn forward_flat(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin()).normalize()
    }

    pub fn right(&self) -> Vec3 {
        self.forward_flat().cross(Vec3::Y).normalize()
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * MOUSE_SENSITIVITY;
        self.pitch = (self.pitch - dy * MOUSE_SENSITIVITY).clamp(-1.5533, 1.5533);
    }

    /// View-projection for wgpu.
    ///
    /// glam 0.33 deprecated `Mat4::perspective_rh` / `Mat4::look_to_rh` in favour
    /// of the explicit `glam::camera` helpers. `rh::proj::directx` is the right
    /// one here: it documents itself as the "DirectX and WebGPU" convention,
    /// clip-space Z in **0..1** with Y up, which is exactly what wgpu's
    /// rasteriser and our `LoadOp::Clear(1.0)` + `CompareFunction::Less` depth
    /// state expect. It produces the same matrix the deprecated call did, so the
    /// depth convention and the front-face winding are unchanged.
    pub fn view_proj(&self) -> Mat4 {
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y_DEG.to_radians(),
            self.aspect,
            Z_NEAR,
            Z_FAR,
        );
        let view = glam::camera::rh::view::look_to_mat4(self.pos, self.forward(), Vec3::Y);
        proj * view
    }
}

/// Per-frame movement intent, gathered from the keyboard.
#[derive(Default, Copy, Clone)]
pub struct MoveInput {
    pub fwd: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    /// Space. Jump on the ground, ascend in noclip.
    pub up: bool,
    /// Left Shift. Descend in noclip.
    pub down: bool,
    /// Left Ctrl. Sprint on foot, fast fly in noclip.
    pub fast: bool,
}

impl MoveInput {
    /// Normalised horizontal wish direction in world space.
    fn wish_dir(&self, cam: &Camera) -> Vec3 {
        let mut dir = Vec3::ZERO;
        if self.fwd {
            dir += cam.forward_flat();
        }
        if self.back {
            dir -= cam.forward_flat();
        }
        if self.right {
            dir += cam.right();
        }
        if self.left {
            dir -= cam.right();
        }
        if dir.length_squared() > 1e-6 {
            dir.normalize()
        } else {
            Vec3::ZERO
        }
    }
}

/// The player's body. `pos` is the centre of the AABB's base -- the point the
/// feet stand on -- so the eye sits at `pos.y + PLAYER_EYE_HEIGHT`.
pub struct Player {
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    /// Debug fly. Toggled with F; ignores gravity and collision entirely.
    pub noclip: bool,
}

impl Player {
    pub fn new(pos: Vec3) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            on_ground: false,
            noclip: true,
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::Y * PLAYER_EYE_HEIGHT
    }

    /// The collision box, as (min, max).
    pub fn aabb(&self) -> (Vec3, Vec3) {
        Self::aabb_at(self.pos)
    }

    pub fn aabb_at(pos: Vec3) -> (Vec3, Vec3) {
        let h = PLAYER_WIDTH * 0.5;
        (
            Vec3::new(pos.x - h, pos.y, pos.z - h),
            Vec3::new(pos.x + h, pos.y + PLAYER_HEIGHT, pos.z + h),
        )
    }

    fn collides_at(world: &World, pos: Vec3) -> bool {
        let (min, max) = Self::aabb_at(pos);
        world.box_collides(min, max)
    }

    pub fn update(&mut self, world: &World, cam: &Camera, input: &MoveInput, dt: f32) {
        if self.noclip {
            self.fly(cam, input, dt);
            return;
        }
        let wish = input.wish_dir(cam);
        let speed = if input.fast { SPRINT_SPEED } else { WALK_SPEED };
        let target = wish * speed;

        // Blend toward the wish velocity rather than snapping to it; the air
        // rate is much lower so a jump commits to its arc.
        let accel = if self.on_ground {
            GROUND_ACCEL
        } else {
            AIR_ACCEL
        };
        let blend = (accel * dt).min(1.0);
        self.vel.x += (target.x - self.vel.x) * blend;
        self.vel.z += (target.z - self.vel.z) * blend;

        if input.up && self.on_ground {
            self.vel.y = JUMP_SPEED;
            self.on_ground = false;
        }
        self.vel.y = (self.vel.y + GRAVITY * dt).max(TERMINAL_VELOCITY);

        let delta = self.vel * dt;
        self.on_ground = false;
        self.move_y(world, delta.y);
        self.move_horizontal(world, 0, delta.x);
        self.move_horizontal(world, 2, delta.z);
    }

    fn fly(&mut self, cam: &Camera, input: &MoveInput, dt: f32) {
        let mut dir = input.wish_dir(cam);
        if input.up {
            dir += Vec3::Y;
        }
        if input.down {
            dir -= Vec3::Y;
        }
        self.vel = Vec3::ZERO;
        self.on_ground = false;
        if dir.length_squared() > 1e-6 {
            let speed = if input.fast {
                FLY_SPEED_FAST
            } else {
                FLY_SPEED
            };
            self.pos += dir.normalize() * speed * dt;
        }
    }

    fn move_y(&mut self, world: &World, dy: f32) {
        if dy == 0.0 {
            // Still probe for ground so standing still keeps `on_ground` true.
            let probe = self.pos - Vec3::Y * (COLLIDE_EPSILON * 4.0);
            self.on_ground = Self::collides_at(world, probe);
            return;
        }
        let want = self.pos + Vec3::Y * dy;
        if !Self::collides_at(world, want) {
            self.pos = want;
            // A tiny downward probe keeps ground contact stable on slopes of
            // carved sub-voxels, where the surface is not on a block boundary.
            if dy < 0.0 {
                let probe = self.pos - Vec3::Y * (COLLIDE_EPSILON * 4.0);
                self.on_ground = Self::collides_at(world, probe);
            }
            return;
        }
        // Blocked: bisect to sit flush against the surface.
        self.pos.y = Self::bisect(world, self.pos, 1, dy);
        if dy < 0.0 {
            self.on_ground = true;
        }
        self.vel.y = 0.0;
    }

    fn move_horizontal(&mut self, world: &World, axis: usize, d: f32) {
        if d == 0.0 {
            return;
        }
        let mut want = self.pos;
        want[axis] += d;
        if !Self::collides_at(world, want) {
            self.pos = want;
            return;
        }
        if self.try_step_up(world, axis, d) {
            return;
        }
        self.pos[axis] = Self::bisect(world, self.pos, axis, d);
        self.vel[axis] = 0.0;
    }

    /// Walk up a lip of at most `STEP_HEIGHT` without jumping. Works against
    /// partially carved blocks too, because the search is a plain collision
    /// query rather than a block-height lookup.
    fn try_step_up(&mut self, world: &World, axis: usize, d: f32) -> bool {
        if !self.on_ground {
            return false;
        }
        let from = self.pos;
        let mut lifted = from;
        lifted[axis] += d;
        lifted.y = from.y + STEP_HEIGHT;
        if Self::collides_at(world, lifted) {
            return false; // no headroom, or the lip is too tall
        }
        // Settle back down to the lowest clear height.
        let (mut lo, mut hi) = (0.0f32, STEP_HEIGHT);
        for _ in 0..10 {
            let mid = 0.5 * (lo + hi);
            let mut probe = lifted;
            probe.y = from.y + mid;
            if Self::collides_at(world, probe) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lifted.y = from.y + hi;
        if Self::collides_at(world, lifted) {
            return false;
        }
        self.pos = lifted;
        true
    }

    /// Largest movement along one axis that stays clear, found by bisection.
    /// Cheap (10 collision queries) and exact enough to sit flush against a
    /// sub-voxel surface, which a block-aligned snap could not do.
    fn bisect(world: &World, from: Vec3, axis: usize, d: f32) -> f32 {
        let (mut lo, mut hi) = (0.0f32, d);
        for _ in 0..10 {
            let mid = 0.5 * (lo + hi);
            let mut probe = from;
            probe[axis] += mid;
            if Self::collides_at(world, probe) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        from[axis] + lo
    }

    /// Lift the player out of anything they are standing inside. Used at spawn
    /// and when leaving noclip.
    pub fn unstick(&mut self, world: &World) {
        for _ in 0..WORLD_HEIGHT {
            if !Self::collides_at(world, self.pos) {
                return;
            }
            self.pos.y += 1.0;
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 3],
    pub fog_start: f32,
    /// Linear, not sRGB -- see `params.x`.
    pub sky_color: [f32; 3],
    pub fog_end: f32,
    /// `x`: 1.0 when the shader must encode linear -> sRGB itself because the
    /// swapchain format is not an sRGB one. The rest is spare.
    pub params: [f32; 4],
    /// `xyz`: unit vector pointing at the sun, world space. `w`: how much
    /// directional light it casts, 0 at night and 1 at noon.
    pub sun: [f32; 4],
}

impl CameraUniform {
    pub fn new(
        cam: &Camera,
        sky_linear: [f32; 3],
        fog_start: f32,
        fog_end: f32,
        encode_srgb: bool,
        sun: [f32; 4],
    ) -> Self {
        Self {
            view_proj: cam.view_proj().to_cols_array_2d(),
            cam_pos: cam.pos.to_array(),
            fog_start,
            sky_color: sky_linear,
            fog_end,
            params: [if encode_srgb { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            sun,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::block::BlockId;
    use crate::world::chunk::{Chunk, ChunkPos};
    use std::sync::Arc;

    /// A world with an empty 3x3x2 chunk region so we can build test geometry
    /// without fighting the noise field.
    fn sandbox() -> World {
        let mut w = World::new(1337);
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
        // A stone floor at y = 4.
        for z in 0..12 {
            for x in 0..12 {
                w.set_block(x, 4, z, BlockId::STONE);
            }
        }
        w
    }

    fn walking_player(pos: Vec3) -> Player {
        let mut p = Player::new(pos);
        p.noclip = false;
        p
    }

    #[test]
    fn projection_uses_the_wgpu_depth_range() {
        let cam = Camera {
            pos: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            aspect: 1.0,
        };
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y_DEG.to_radians(),
            1.0,
            Z_NEAR,
            Z_FAR,
        );
        // Right-handed view space looks down -Z. A point at the near plane must
        // land at depth 0 and one at the far plane at depth 1.
        let near = proj * glam::Vec4::new(0.0, 0.0, -Z_NEAR, 1.0);
        let far = proj * glam::Vec4::new(0.0, 0.0, -Z_FAR, 1.0);
        assert!(
            (near.z / near.w).abs() < 1e-4,
            "near mapped to {}",
            near.z / near.w
        );
        assert!(
            (far.z / far.w - 1.0).abs() < 1e-4,
            "far mapped to {}",
            far.z / far.w
        );
        // And the camera's own matrix is finite and invertible.
        assert!(cam.view_proj().determinant().abs() > 0.0);
    }

    #[test]
    fn gravity_lands_the_player_on_the_floor() {
        let w = sandbox();
        let cam = Camera::new(Vec3::ZERO);
        let input = MoveInput::default();
        let mut p = walking_player(Vec3::new(5.5, 12.0, 5.5));
        for _ in 0..400 {
            p.update(&w, &cam, &input, 1.0 / 60.0);
        }
        assert!(p.on_ground, "player never landed");
        assert!(
            (p.pos.y - 5.0).abs() < 0.02,
            "player rested at y={} instead of on top of the y=4 floor",
            p.pos.y
        );
        assert!(p.vel.y.abs() < 1e-3);
    }

    #[test]
    fn a_wall_stops_horizontal_movement() {
        let mut w = sandbox();
        // A two-block-tall wall at x = 8, too tall to step over.
        for y in 5..8 {
            for z in 0..12 {
                w.set_block(8, y, z, BlockId::STONE);
            }
        }
        let mut cam = Camera::new(Vec3::ZERO);
        cam.yaw = 0.0; // facing +X
        let mut input = MoveInput::default();
        input.fwd = true;
        let mut p = walking_player(Vec3::new(5.5, 5.0, 5.5));
        for _ in 0..300 {
            p.update(&w, &cam, &input, 1.0 / 60.0);
        }
        assert!(
            p.pos.x < 8.0,
            "player walked through the wall to x={}",
            p.pos.x
        );
        assert!(p.pos.x > 7.0, "player stopped short at x={}", p.pos.x);
    }

    #[test]
    fn the_player_steps_up_a_single_block() {
        let mut w = sandbox();
        // `sandbox` already lays a floor at y = 4, whose top surface is y = 5.0.
        // Extend it, then raise everything from x = 8 onward by one block so the
        // step is a plateau rather than a ledge: 300 frames is about 23 blocks of
        // walking, so a short lip would just be walked off the far end and the
        // player would be mid-fall by the time the assertion runs.
        for z in 0..12 {
            for x in 0..30 {
                w.set_block(x, 4, z, BlockId::STONE);
                if x >= 8 {
                    w.set_block(x, 5, z, BlockId::STONE);
                }
            }
        }
        let mut cam = Camera::new(Vec3::ZERO);
        cam.yaw = 0.0;
        let mut input = MoveInput::default();
        input.fwd = true;
        let mut p = walking_player(Vec3::new(5.5, 5.0, 5.5));
        for _ in 0..300 {
            p.update(&w, &cam, &input, 1.0 / 60.0);
        }
        assert!(
            p.pos.y > 5.5,
            "player failed to step up; y={} x={}",
            p.pos.y,
            p.pos.x
        );
        assert!(
            p.pos.x > 8.5,
            "player did not get past the lip: x={}",
            p.pos.x
        );
    }

    #[test]
    fn collision_uses_the_carve_mask_not_just_block_solidity() {
        let mut w = sandbox();
        // A pillar the player would normally walk into.
        for y in 5..7 {
            w.set_block(8, y, 5, BlockId::STONE);
        }
        let mut p = walking_player(Vec3::new(8.5, 5.0, 5.5));
        p.pos = Vec3::new(8.5, 5.0, 5.5);
        assert!(
            Player::collides_at(&w, p.pos),
            "player should start intersecting the pillar"
        );
        // Hollow the whole lower block out and the intersection goes away.
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    w.carve(8, 5, 5, sx, sy, sz);
                }
            }
        }
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    w.carve(8, 6, 5, sx, sy, sz);
                }
            }
        }
        assert!(!Player::collides_at(&w, p.pos));
    }

    #[test]
    fn noclip_ignores_geometry() {
        let mut w = sandbox();
        for y in 0..12 {
            for z in 0..12 {
                w.set_block(8, y, z, BlockId::STONE);
            }
        }
        let mut cam = Camera::new(Vec3::ZERO);
        cam.yaw = 0.0;
        let mut input = MoveInput::default();
        input.fwd = true;
        let mut p = Player::new(Vec3::new(5.5, 6.0, 5.5));
        assert!(p.noclip);
        for _ in 0..120 {
            p.update(&w, &cam, &input, 1.0 / 60.0);
        }
        assert!(p.pos.x > 9.0, "noclip should pass straight through");
    }

    #[test]
    fn unstick_lifts_the_player_out_of_rock() {
        let mut w = sandbox();
        for y in 0..8 {
            for z in 0..12 {
                for x in 0..12 {
                    w.set_block(x, y, z, BlockId::STONE);
                }
            }
        }
        let mut p = walking_player(Vec3::new(5.5, 2.0, 5.5));
        p.unstick(&w);
        assert!(!Player::collides_at(&w, p.pos));
        assert!(p.pos.y >= 8.0, "unstick left the player at y={}", p.pos.y);
    }
}
