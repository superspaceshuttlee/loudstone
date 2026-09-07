//! Data-driven entity models: boxes, a skin, and animation.
//!
//! The style rule this file exists to enforce: **detail lives in the texture,
//! never in extra cuboids.** A mob is six boxes in classic humanoid proportions
//! — head, torso, two arms, two legs — and everything that makes one read as a
//! zombie rather than a stack of crates is painted into its skin. An earlier
//! attempt built a fourteen-cuboid asymmetric figure with a jaw and boots; it
//! looked like a low-poly character standing in a blocky world, because that is
//! what it was. Adding geometry fights this art style rather than serving it.
//!
//! Proportions and skin layout follow Minecraft's, in texels where 16 texels is
//! one block: an 8x8x8 head on an 8x12x4 torso, 4x12x4 limbs, 32 texels tall
//! overall. Following the standard exactly means a skin drawn to the usual
//! template maps correctly with no bespoke bookkeeping, and anyone who has made
//! a Minecraft skin already knows how to make one for this.

use crate::render::mesh::Vertex;
use crate::render::texture;
use glam::{Mat3, Vec3};

/// One box of a model, positioned and sized in texels relative to the model's
/// origin at the feet, centred on x and z.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub channel: Channel,
    /// Skin-sheet coordinate of this box's unwrapped faces.
    pub uv: (usize, usize),
    /// Box size in texels: width, height, depth.
    pub size: (usize, usize, usize),
    /// Offset of the box centre from the model origin, in texels.
    pub offset: (f32, f32, f32),
    /// The point the part rotates about, in texels from the model origin.
    /// A limb swings from its shoulder or hip, not from its middle.
    pub pivot: (f32, f32, f32),
}

/// Which animation drives a part.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Head,
    Body,
    ArmRight,
    ArmLeft,
    LegRight,
    LegLeft,
}

/// Texels per block. The whole reason a mob sits correctly beside the terrain.
pub const TEXEL: f32 = 1.0 / 16.0;

/// The standard humanoid. Every mob uses it; only the skin differs.
pub fn humanoid() -> &'static [Part] {
    use texture::*;
    const PARTS: [Part; 6] = [
        Part {
            channel: Channel::Head,
            uv: HEAD_UV,
            size: HEAD_SIZE,
            offset: (0.0, 28.0, 0.0),
            pivot: (0.0, 24.0, 0.0),
        },
        Part {
            channel: Channel::Body,
            uv: BODY_UV,
            size: BODY_SIZE,
            offset: (0.0, 18.0, 0.0),
            pivot: (0.0, 24.0, 0.0),
        },
        Part {
            channel: Channel::ArmRight,
            uv: ARM_R_UV,
            size: ARM_SIZE,
            offset: (-6.0, 18.0, 0.0),
            pivot: (-4.0, 22.0, 0.0),
        },
        Part {
            channel: Channel::ArmLeft,
            uv: ARM_L_UV,
            size: ARM_SIZE,
            offset: (6.0, 18.0, 0.0),
            pivot: (4.0, 22.0, 0.0),
        },
        Part {
            channel: Channel::LegRight,
            uv: LEG_R_UV,
            size: LEG_SIZE,
            offset: (-2.0, 6.0, 0.0),
            pivot: (-2.0, 12.0, 0.0),
        },
        Part {
            channel: Channel::LegLeft,
            uv: LEG_L_UV,
            size: LEG_SIZE,
            offset: (2.0, 6.0, 0.0),
            pivot: (2.0, 12.0, 0.0),
        },
    ];
    &PARTS
}

/// A four-legged body: a horizontal barrel, a head at the front, four short
/// legs. Shares the skin sheet layout with the humanoid so one painter serves
/// both -- the torso rectangle simply wraps a longer box.
pub fn quadruped() -> &'static [Part] {
    use texture::*;
    // Mojang's proportions, in the usual 16-units-to-a-block model space, with
    // the origin between the feet. These are not eyeballed: a pig is a 10x8x16
    // barrel on 4x6x4 legs with an 8-cube head, and the reason it reads as a pig
    // rather than as a pink box on stilts is that the barrel is wide and low and
    // the legs are stubby. An earlier version here used 8-tall legs and a body
    // riding at 12, which is the same silhouette a dog would have.
    const PARTS: [Part; 7] = [
        Part {
            channel: Channel::Head,
            uv: QUAD_HEAD_UV,
            size: QUAD_HEAD_SIZE,
            offset: (0.0, 12.0, -10.0),
            pivot: (0.0, 12.0, -6.0),
        },
        // A snout is one of the few places geometry genuinely beats texture: a
        // painted-on nose reads as a smudge, a box that sticks out reads as a
        // face. It rides the head channel so it turns with it.
        Part {
            channel: Channel::Head,
            uv: QUAD_SNOUT_UV,
            size: QUAD_SNOUT_SIZE,
            offset: (0.0, 10.5, -14.5),
            pivot: (0.0, 12.0, -6.0),
        },
        Part {
            channel: Channel::Body,
            uv: QUAD_BODY_UV,
            size: QUAD_BODY_SIZE,
            offset: (0.0, 10.0, 0.0),
            pivot: (0.0, 10.0, 0.0),
        },
        // Front pair, driven by the arm channels so they swing with the gait.
        // The back pair sits further back than the front pair is forward, which
        // is Mojang's asymmetry and is what gives the animal a rump.
        Part {
            channel: Channel::ArmRight,
            uv: QUAD_LEG_UV,
            size: QUAD_LEG_SIZE,
            offset: (-3.0, 3.0, -5.0),
            pivot: (-3.0, 6.0, -5.0),
        },
        Part {
            channel: Channel::ArmLeft,
            uv: QUAD_LEG_UV,
            size: QUAD_LEG_SIZE,
            offset: (3.0, 3.0, -5.0),
            pivot: (3.0, 6.0, -5.0),
        },
        Part {
            channel: Channel::LegRight,
            uv: QUAD_LEG_UV,
            size: QUAD_LEG_SIZE,
            offset: (-3.0, 3.0, 7.0),
            pivot: (-3.0, 6.0, 7.0),
        },
        Part {
            channel: Channel::LegLeft,
            uv: QUAD_LEG_UV,
            size: QUAD_LEG_SIZE,
            offset: (3.0, 3.0, 7.0),
            pivot: (3.0, 6.0, 7.0),
        },
    ];
    &PARTS
}

/// How a mob is standing this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pose {
    /// Distance walked, in blocks. Drives the limb swing, so the gait matches
    /// the ground rather than the clock and never moonwalks.
    pub stride: f32,
    /// How fast it is going, as a fraction of its top speed.
    pub speed: f32,
    /// Head yaw and pitch relative to the body, in radians.
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// 0 at rest, 1 mid-swing.
    pub attack: f32,
    /// Arms held out in front, as zombies do. 0 for anything else.
    pub arms_forward: f32,
    /// Creeper-style: no limb swing, a waddle instead.
    pub waddle: bool,
    /// Four-legged, so the arm channels drive front legs rather than arms.
    pub on_all_fours: bool,
}

/// Euler rotation for one part, in radians.
fn part_rotation(ch: Channel, p: &Pose) -> Vec3 {
    // A walk cycle is one sine wave over stride, with the arms opposing the
    // legs. Amplitude scales with speed so a standing mob is still.
    let swing = (p.stride * 2.0).sin() * 0.9 * p.speed.clamp(0.0, 1.0);
    match ch {
        Channel::Head => Vec3::new(p.head_pitch, p.head_yaw, 0.0),
        Channel::Body => Vec3::ZERO,
        Channel::ArmRight | Channel::ArmLeft => {
            let side = if ch == Channel::ArmRight { 1.0 } else { -1.0 };
            if p.on_all_fours {
                // A front leg is a leg: it swings, it does not reach.
                return Vec3::new(swing * -side, 0.0, 0.0);
            }
            if p.waddle {
                return Vec3::ZERO;
            }
            // Zombie arms are held forward; the swing rides on top of that, and
            // an attack throws them down and back up.
            //
            // The sign matters and is easy to get backwards: a hanging arm
            // points (0,-1,0), and rotating that about +X by -90 degrees sends
            // it to +Z, which is the model's BACK. Forward is +90.
            let forward = std::f32::consts::FRAC_PI_2 * p.arms_forward;
            let x = forward - swing * side * 0.6 * (1.0 - p.arms_forward * 0.7) - p.attack * 1.2;
            // A slight outward splay stops the arms clipping the torso.
            Vec3::new(x, 0.0, side * 0.06 * (1.0 - p.arms_forward))
        }
        Channel::LegRight | Channel::LegLeft => {
            let side = if ch == Channel::LegRight { 1.0 } else { -1.0 };
            if p.waddle && !p.on_all_fours {
                return Vec3::new(0.0, 0.0, side * 0.12 * p.speed);
            }
            Vec3::new(swing * side, 0.0, 0.0)
        }
    }
}

/// World rotation for a mob at `yaw`.
///
/// A model is built with its front on -Z, but a mob's yaw is measured with 0
/// pointing along +X. This is the one place that gap is bridged, and it is
/// asserted directly in the tests rather than inferred from how a silhouette
/// looks, because reasoning about the sign by eye got it wrong three times.
fn body_rotation(yaw: f32) -> Mat3 {
    Mat3::from_rotation_y(-yaw - std::f32::consts::FRAC_PI_2)
}

/// Append a posed model to an entity mesh.
///
/// `origin` is the mob's feet, `yaw` its facing, `scale` blocks per 16 texels
/// (1.0 for a normal humanoid). `light` is the shading the renderer wants.
#[allow(clippy::too_many_arguments)]
pub fn append(
    parts: &[Part],
    skin: usize,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: Vec3,
    yaw: f32,
    scale: f32,
    pose: &Pose,
    light: f32,
) {
    // A mob's yaw is measured with 0 pointing along +X, but a model's front
    // face is its -Z side. Rotating by -yaw alone therefore leaves every
    // creature standing a quarter turn off: none of them face the way they are
    // walking, and a zombie's forward-held arms stick out sideways. The extra
    // quarter turn maps model-forward onto mob-forward.
    let body = body_rotation(yaw);
    for part in parts {
        let rot = part_rotation(part.channel, pose);
        let local = Mat3::from_rotation_y(rot.y)
            * Mat3::from_rotation_x(rot.x)
            * Mat3::from_rotation_z(rot.z);
        let pivot = Vec3::new(part.pivot.0, part.pivot.1, part.pivot.2);
        let centre = Vec3::new(part.offset.0, part.offset.1, part.offset.2);
        let half = Vec3::new(
            part.size.0 as f32 * 0.5,
            part.size.1 as f32 * 0.5,
            part.size.2 as f32 * 0.5,
        );
        let rects = texture::box_face_rects(part.uv, part.size);

        for (f, corners) in FACE_CORNERS.iter().enumerate() {
            let uv_rect = texture::skin_uv(skin, rects[f]);
            let shade = FACE_SHADE[f] * light;
            let base = verts.len() as u32;
            for c in corners.iter() {
                // Texel-space corner of this box face.
                let p = centre + Vec3::new(c[0], c[1], c[2]) * half;
                // Rotate about the part's pivot, then face the body, then land
                // in the world at the mob's feet.
                let p = local * (p - pivot) + pivot;
                let p = body * p;
                let world = origin + p * TEXEL * scale;
                let uv = face_uv(f, *c);
                verts.push(Vertex {
                    pos: world.to_array(),
                    color: [1.0, 1.0, 1.0],
                    light: shade,
                    uv: [
                        uv_rect[0] + (uv_rect[2] - uv_rect[0]) * uv[0],
                        uv_rect[1] + (uv_rect[3] - uv_rect[1]) * uv[1],
                    ],
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

/// Unit-cube corners per face, counter-clockwise seen from outside, in the same
/// face order the mesher and the skin layout use.
const FACE_CORNERS: [[[f32; 3]; 4]; 6] = [
    [[-1., 1., -1.], [-1., 1., 1.], [1., 1., 1.], [1., 1., -1.]], // top
    [
        [-1., -1., 1.],
        [-1., -1., -1.],
        [1., -1., -1.],
        [1., -1., 1.],
    ], // bottom
    [[1., -1., 1.], [-1., -1., 1.], [-1., 1., 1.], [1., 1., 1.]], // +Z back
    [
        [-1., -1., -1.],
        [1., -1., -1.],
        [1., 1., -1.],
        [-1., 1., -1.],
    ], // -Z face
    [[1., -1., -1.], [1., -1., 1.], [1., 1., 1.], [1., 1., -1.]], // +X
    [
        [-1., -1., 1.],
        [-1., -1., -1.],
        [-1., 1., -1.],
        [-1., 1., 1.],
    ], // -X
];

/// Where a box corner lands in its face's rectangle, derived from the corner's
/// own position rather than from its index.
///
/// Indexing by corner number looks equivalent and is not: the corner ORDER
/// differs between faces, so a fixed table rotates the texture 90 degrees on
/// some of them. On a 4x12 arm that smears a four-texel strip down a twelve-
/// texel face, which is where the vertical streaking on every limb came from.
/// `v` runs downward like an image row, so nothing is drawn upside down.
fn face_uv(face: usize, c: [f32; 3]) -> [f32; 2] {
    // Box corners are -1..1; map to 0..1 first.
    let (x, y, z) = ((c[0] + 1.0) * 0.5, (c[1] + 1.0) * 0.5, (c[2] + 1.0) * 0.5);
    match face {
        0 => [x, z],             // +Y top
        1 => [x, 1.0 - z],       // -Y bottom
        2 => [1.0 - x, 1.0 - y], // +Z back
        3 => [x, 1.0 - y],       // -Z front
        4 => [z, 1.0 - y],       // +X
        _ => [1.0 - z, 1.0 - y], // -X
    }
}

/// Per-face shading, matching the terrain so mobs sit in the same light.
const FACE_SHADE: [f32; 6] = [1.00, 0.45, 0.80, 0.80, 0.65, 0.65];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_humanoid_is_six_boxes_and_thirty_two_texels_tall() {
        let p = humanoid();
        assert_eq!(
            p.len(),
            6,
            "detail belongs in the skin, not in more cuboids"
        );
        let top = p
            .iter()
            .map(|q| q.offset.1 + q.size.1 as f32 * 0.5)
            .fold(f32::MIN, f32::max);
        assert!(
            (top - 32.0).abs() < 0.01,
            "model should stand 32 texels tall, got {top}"
        );
        let bottom = p
            .iter()
            .map(|q| q.offset.1 - q.size.1 as f32 * 0.5)
            .fold(f32::MAX, f32::min);
        assert!(
            bottom.abs() < 0.01,
            "feet should sit at the origin, got {bottom}"
        );
    }

    #[test]
    fn every_part_has_a_distinct_skin_region() {
        let p = humanoid();
        for a in 0..p.len() {
            for b in (a + 1)..p.len() {
                assert_ne!(
                    (p[a].uv, p[a].size),
                    (p[b].uv, p[b].size),
                    "two parts share a skin region, so one is wearing the other's texture"
                );
            }
        }
    }

    #[test]
    fn a_standing_mob_does_not_swing_its_limbs() {
        let still = Pose {
            stride: 3.0,
            speed: 0.0,
            ..Default::default()
        };
        for ch in [Channel::ArmRight, Channel::LegLeft] {
            let r = part_rotation(ch, &still);
            assert!(r.x.abs() < 1.0e-6, "{ch:?} moved while standing still");
        }
    }

    #[test]
    fn arms_and_legs_swing_in_opposition() {
        let walking = Pose {
            stride: 0.7,
            speed: 1.0,
            ..Default::default()
        };
        let arm = part_rotation(Channel::ArmRight, &walking).x;
        let leg = part_rotation(Channel::LegRight, &walking).x;
        assert!(
            arm * leg < 0.0,
            "the right arm and right leg must swing opposite ways, got {arm} and {leg}"
        );
    }

    #[test]
    fn zombie_arms_are_held_out_in_front() {
        let z = Pose {
            arms_forward: 1.0,
            ..Default::default()
        };
        let arm = part_rotation(Channel::ArmRight, &z).x;
        assert!(
            (arm - std::f32::consts::FRAC_PI_2).abs() < 0.2,
            "zombie arms should sit near +90 degrees, which is forward, got {arm}"
        );
    }

    #[test]
    fn a_model_faces_the_way_its_yaw_points() {
        // The model's front is -Z. A mob at yaw t faces (cos t, 0, sin t).
        // Rotating the front by the body matrix must land exactly there.
        for deg in [0.0f32, 90.0, 180.0, 270.0] {
            let yaw = deg.to_radians();
            let front = body_rotation(yaw) * Vec3::new(0.0, 0.0, -1.0);
            let want = Vec3::new(yaw.cos(), 0.0, yaw.sin());
            assert!(
                (front - want).length() < 1.0e-5,
                "at yaw {deg} the model faces {front:?} but the mob faces {want:?}"
            );
        }
    }

    #[test]
    fn forward_held_arms_point_the_way_the_model_faces() {
        // Arms held forward must end up on the same side as the face, not out
        // the back. Both are -Z in model space, so they have to agree.
        let z = Pose {
            arms_forward: 1.0,
            ..Default::default()
        };
        let rot = part_rotation(Channel::ArmRight, &z);
        let hanging = Vec3::new(0.0, -1.0, 0.0);
        let pointed = Mat3::from_rotation_x(rot.x) * hanging;
        assert!(
            pointed.z < -0.9,
            "a raised arm should point along -Z, the model's front; got {pointed:?}"
        );
    }

    /// Every part's unwrap must fit inside the 64x64 skin sheet. The pig's body
    /// originally borrowed the humanoid torso rectangle, and a 10x8x16 barrel
    /// does not fit where an 8x12x4 torso lives -- it ran into the arm and leg
    /// regions and wore them.
    #[test]
    fn every_part_unwraps_inside_its_sheet() {
        for (name, parts) in [("humanoid", humanoid()), ("quadruped", quadruped())] {
            for p in parts {
                for r in texture::box_face_rects(p.uv, p.size) {
                    assert!(
                        r[0] + r[2] <= texture::SKIN_SIZE && r[1] + r[3] <= texture::SKIN_SIZE,
                        "{name} part {:?} unwraps to {r:?}, off a {}x{} sheet",
                        p.channel,
                        texture::SKIN_SIZE,
                        texture::SKIN_SIZE
                    );
                }
            }
        }
    }

    #[test]
    fn a_quadruped_has_four_legs_on_the_ground() {
        let parts = quadruped();
        let legs = parts
            .iter()
            .filter(|p| !matches!(p.channel, Channel::Head | Channel::Body))
            .count();
        assert_eq!(legs, 4);
        for p in parts
            .iter()
            .filter(|p| !matches!(p.channel, Channel::Head | Channel::Body))
        {
            let bottom = p.offset.1 - p.size.1 as f32 * 0.5;
            assert!(
                bottom.abs() < 0.01,
                "a leg should reach the ground, got {bottom}"
            );
        }
    }

    #[test]
    fn a_faces_texture_is_not_transposed() {
        // Walking a box face along its width must move along the texture's
        // width, not its height. Indexing UVs by corner number satisfied every
        // other test while silently rotating the texture on four of six faces.
        // On a +/-Z face the width runs along x; on a +/-X face it runs along z,
        // because that axis is the face normal. Height is y on all four sides.
        for (face, along) in [(2usize, 0usize), (3, 0), (4, 2), (5, 2)] {
            let mut a = [-1.0f32; 3];
            let mut b = [-1.0f32; 3];
            b[along] = 1.0;
            let du = face_uv(face, b)[0] - face_uv(face, a)[0];
            a[1] = -1.0;
            let mut up = [-1.0f32; 3];
            up[1] = 1.0;
            let dv = face_uv(face, up)[1] - face_uv(face, a)[1];
            assert!(
                du.abs() > 0.9,
                "face {face}: texture width should follow axis {along}, du={du}"
            );
            assert!(
                dv.abs() > 0.9,
                "face {face}: texture height should follow y, dv={dv}"
            );
        }
    }

    #[test]
    fn a_posed_model_produces_six_quads_per_part() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        append(
            humanoid(),
            texture::SKIN_ZOMBIE,
            &mut v,
            &mut i,
            Vec3::ZERO,
            0.0,
            1.0,
            &Pose::default(),
            1.0,
        );
        assert_eq!(v.len(), 6 * 6 * 4);
        assert_eq!(i.len(), 6 * 6 * 6);
        assert!(v.iter().all(|x| x.pos.iter().all(|c| c.is_finite())));
    }

    #[test]
    fn the_model_stands_within_its_own_height() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        append(
            humanoid(),
            texture::SKIN_ZOMBIE,
            &mut v,
            &mut i,
            Vec3::ZERO,
            0.0,
            1.0,
            &Pose {
                stride: 1.0,
                speed: 1.0,
                arms_forward: 1.0,
                ..Default::default()
            },
            1.0,
        );
        let top = v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max);
        let bottom = v.iter().map(|x| x.pos[1]).fold(f32::MAX, f32::min);
        assert!(
            top <= 2.05,
            "a humanoid should be about 2 blocks tall, got {top}"
        );
        assert!(
            bottom >= -0.7,
            "limbs should not swing far below the feet, got {bottom}"
        );
    }
}
