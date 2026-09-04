//! Free-fly camera and its GPU uniform.

use crate::config::{FLY_SPEED, FLY_SPEED_FAST, FOV_Y_DEG, MOUSE_SENSITIVITY, Z_FAR, Z_NEAR};
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

    pub fn view_proj(&self) -> Mat4 {
        let proj = Mat4::perspective_rh(FOV_Y_DEG.to_radians(), self.aspect, Z_NEAR, Z_FAR);
        let view = Mat4::look_to_rh(self.pos, self.forward(), Vec3::Y);
        proj * view
    }
}

/// Per-frame movement input, gathered from the keyboard.
#[derive(Default)]
pub struct FlyInput {
    pub fwd: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
}

impl FlyInput {
    pub fn apply(&self, cam: &mut Camera, dt: f32) {
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
        if self.up {
            dir += Vec3::Y;
        }
        if self.down {
            dir -= Vec3::Y;
        }
        if dir.length_squared() > 0.0 {
            let speed = if self.fast { FLY_SPEED_FAST } else { FLY_SPEED };
            cam.pos += dir.normalize() * speed * dt;
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 3],
    pub fog_start: f32,
    pub sky_color: [f32; 3],
    pub fog_end: f32,
}

impl CameraUniform {
    pub fn new(cam: &Camera, sky: [f32; 3], fog_start: f32, fog_end: f32) -> Self {
        Self {
            view_proj: cam.view_proj().to_cols_array_2d(),
            cam_pos: cam.pos.to_array(),
            fog_start,
            sky_color: sky,
            fog_end,
        }
    }
}
