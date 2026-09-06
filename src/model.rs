//! Data-driven low-poly entity models.
//!
//! The JSON files are Loudstone's stable runtime format. Blockbench projects
//! are generated from the same source for visual editing and inspection.

use crate::gfx::Renderer;
use crate::mesh::Vertex;
use crate::texture::{self, TileId};
use glam::Vec3;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct Model {
    pub name: String,
    pub parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
pub struct Part {
    pub name: String,
    pub pivot: [f32; 3],
    pub from: [f32; 3],
    pub to: [f32; 3],
    #[serde(default)]
    pub bend: f32,
    pub tint: [f32; 3],
    pub material: Material,
    #[serde(default)]
    pub front: Option<Material>,
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    Skin,
    Face,
    Shirt,
}

impl Material {
    fn tile(self) -> TileId {
        match self {
            Material::Skin => texture::T_ZOMBIE_HEAD,
            Material::Face => texture::T_ZOMBIE_FACE,
            Material::Shirt => texture::T_ZOMBIE_BODY,
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct PartPose {
    pub bend: f32,
    pub offset: Vec3,
}

pub fn zombie() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        ron::from_str(include_str!("../assets/models/zombie.model.ron"))
            .expect("the built-in zombie model must be valid")
    })
}

pub fn append(
    model: &Model,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: Vec3,
    yaw: f32,
    pose: impl Fn(Option<&str>) -> PartPose,
) {
    let (sy, cy) = yaw.sin_cos();
    let turn = |p: Vec3| Vec3::new(p.x * cy - p.z * sy, p.y, p.x * sy + p.z * cy);

    for part in &model.parts {
        let animated = pose(part.channel.as_deref());
        let pivot = origin + turn(Vec3::from(part.pivot) + animated.offset);
        let base = part.material.tile();
        let mut tiles = [base; 6];
        if let Some(front) = part.front {
            tiles[4] = front.tile();
        }
        Renderer::model_box_geometry(
            verts,
            indices,
            pivot,
            Vec3::from(part.from),
            Vec3::from(part.to),
            yaw,
            part.bend + animated.bend,
            part.tint,
            tiles,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zombie_asset_is_named_and_has_unique_parts() {
        let model = zombie();
        assert_eq!(model.name, "grave_roamer");
        let mut names = std::collections::HashSet::new();
        assert!(model.parts.len() >= 16);
        assert!(model.parts.iter().all(|part| names.insert(&part.name)));
    }
}
