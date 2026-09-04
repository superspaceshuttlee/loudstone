//! wgpu setup, the terrain pipeline, the target-block highlight, and the HUD pass.

use crate::camera::{Camera, CameraUniform};
use crate::chunk::ChunkPos;
use crate::config::{
    render_distance_blocks, CHUNK_SIZE_I, FOG_END_FRAC, FOG_START_FRAC, HIGHLIGHT_COLOR,
    HIGHLIGHT_INFLATE,
};
use crate::hud::Hud;
use crate::mesh::Vertex;
use crate::world::ChunkMeshData;
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// GPU buffers for one chunk's mesh.
pub struct ChunkMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    index_count: u32,
    /// World-space centre, for frustum culling.
    center: glam::Vec3,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    meshes: HashMap<ChunkPos, ChunkMesh>,
    /// Rebuilt only when the targeted block changes.
    highlight: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    highlight_at: Option<(i32, i32, i32)>,
    /// Mobs and other moving geometry, rebuilt every frame and never frustum
    /// culled as a unit -- it is one small buffer covering the whole scene.
    entities: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// True when the swapchain is not an sRGB format and the shader must encode.
    encode_srgb: bool,
    /// The overlay. Owned here so a frame is recorded in one place; call
    /// `hud.begin(..)` and the draw helpers from the main loop before `render`.
    pub hud: Hud,
    pub drawn_indices: u32,
    pub drawn_chunks: u32,
    /// When set, the next rendered frame is copied back and written here.
    capture_to: Option<std::path::PathBuf>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .expect("no suitable GPU adapter");

        let info = adapter.get_info();
        println!("[loudstone] adapter: {} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("request device");

        // Start from the driver's own defaults so new fields in future wgpu
        // versions need not be enumerated here, then override what matters.
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface is not supported by this adapter");
        // COPY_SRC lets a finished frame be read back for screenshots. Every
        // desktop backend supports it; if one ever does not, drop it rather than
        // failing to start.
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let caps = surface.get_capabilities(&adapter);
        if !caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            eprintln!("[loudstone] surface cannot be copied; screenshots disabled");
            config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        let format = config.format;
        let encode_srgb = !format.is_srgb();
        surface.configure(&device, &config);

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(Vertex::layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let depth_view = create_depth(&device, &config);
        let hud = Hud::with_depth(&device, format, Some(DEPTH_FORMAT));

        Self {
            surface,
            device,
            queue,
            config,
            size,
            pipeline,
            camera_buf,
            camera_bind_group,
            depth_view,
            meshes: HashMap::new(),
            highlight: None,
            highlight_at: None,
            entities: None,
            encode_srgb,
            hud,
            drawn_indices: 0,
            drawn_chunks: 0,
            capture_to: None,
        }
    }

    /// Ask for the next frame to be written to disk as a PNG.
    pub fn request_capture(&mut self, path: std::path::PathBuf) {
        self.capture_to = Some(path);
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth(&self.device, &self.config);
    }

    pub fn loaded_mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Apply one frame of streaming output: drop what left the radius, upload
    /// what finished meshing.
    pub fn apply_stream(&mut self, dropped: &[ChunkPos], ready: Vec<ChunkMeshData>) {
        for pos in dropped {
            self.meshes.remove(pos);
        }
        for m in ready {
            self.upload_mesh(m.pos, &m.verts, &m.indices);
        }
    }

    /// Replace (or insert) the GPU mesh for one chunk. An empty mesh removes it,
    /// which is how a chunk the player hollows out stops being drawn at all.
    pub fn upload_mesh(&mut self, pos: ChunkPos, verts: &[Vertex], indices: &[u32]) {
        if indices.is_empty() {
            self.meshes.remove(&pos);
            return;
        }
        let origin = pos.origin();
        let vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vbuf"),
                contents: bytemuck::cast_slice(verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let ibuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk ibuf"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let half = CHUNK_SIZE_I as f32 * 0.5;
        self.meshes.insert(
            pos,
            ChunkMesh {
                vbuf,
                ibuf,
                index_count: indices.len() as u32,
                center: glam::Vec3::new(
                    origin.0 as f32 + half,
                    origin.1 as f32 + half,
                    origin.2 as f32 + half,
                ),
            },
        );
    }

    /// Replace the per-frame entity geometry (mobs, projectiles). Empty clears it.
    pub fn set_entities(&mut self, verts: &[Vertex], indices: &[u32]) {
        if indices.is_empty() {
            self.entities = None;
            return;
        }
        let vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("entity vbuf"),
                contents: bytemuck::cast_slice(verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let ibuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("entity ibuf"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.entities = Some((vbuf, ibuf, indices.len() as u32));
    }

    /// Build an axis-aligned coloured box. Mobs are drawn as a body and a head,
    /// which is all the fidelity this project wants.
    pub fn box_geometry(
        verts: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        min: glam::Vec3,
        max: glam::Vec3,
        color: [f32; 3],
    ) {
        push_box_shaded(
            verts,
            indices,
            [min.x, min.y, min.z],
            [max.x - min.x, max.y - min.y, max.z - min.z],
            color,
        );
    }

    /// Point the selection box at a block, or clear it with `None`. Rebuilds the
    /// buffers only when the target actually changes, so holding still is free.
    pub fn set_highlight(&mut self, block: Option<(i32, i32, i32)>) {
        if self.highlight_at == block {
            return;
        }
        self.highlight_at = block;
        self.highlight = block.map(|(x, y, z)| {
            let (verts, indices) = wire_box(x as f32, y as f32, z as f32);
            let vbuf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("highlight vbuf"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ibuf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("highlight ibuf"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
            (vbuf, ibuf, indices.len() as u32)
        });
    }

    /// Draw one frame. Surface loss and resize races are handled here rather than
    /// bubbled up, so the caller only ever asks for a frame.
    ///
    /// `sky` is an ordinary sRGB colour; converting it to the linear values the
    /// shader and the clear both want happens here, in one place.
    pub fn render(&mut self, cam: &Camera, sky: [f32; 3]) {
        let far = render_distance_blocks();
        let sky_linear = srgb_to_linear(sky);
        let uniform = CameraUniform::new(
            cam,
            sky_linear,
            far * FOG_START_FRAC,
            far * FOG_END_FRAC,
            self.encode_srgb,
        );
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::cast_slice(&[uniform]));

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                let size = self.size;
                self.resize(size);
                return;
            }
            // Timeout, Occluded, or a validation error: skip this frame.
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // An sRGB swapchain encodes on write, so its clear value must be linear.
        // A non-sRGB one is written raw, so it must already be encoded.
        let clear = if self.encode_srgb { sky } else { sky_linear };
        let planes = frustum_planes(cam.view_proj());
        let mut drawn_indices = 0u32;
        let mut drawn_chunks = 0u32;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);

            // Chunk meshes are already in world space, so there is no per-chunk
            // transform: one pipeline, one bind group, one draw call per chunk.
            let radius = (CHUNK_SIZE_I as f32) * 0.8661; // half-diagonal of a 16^3 cube
            for mesh in self.meshes.values() {
                if !sphere_in_frustum(&planes, mesh.center, radius) {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vbuf.slice(..));
                pass.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                drawn_indices += mesh.index_count;
                drawn_chunks += 1;
            }

            if let Some((vbuf, ibuf, count)) = &self.entities {
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
                drawn_indices += *count;
            }

            if let Some((vbuf, ibuf, count)) = &self.highlight {
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }

            // The overlay shares this pass so there is no second clear; its own
            // pipeline has depth testing off, so it always draws on top.
            self.hud.draw(&self.device, &self.queue, &mut pass);
        }

        self.drawn_indices = drawn_indices;
        self.drawn_chunks = drawn_chunks;

        // A screenshot is a copy of this same frame, queued into the same
        // encoder before it is presented.
        let capture = self.capture_to.take().map(|path| {
            let (w, h) = (self.config.width, self.config.height);
            let row = (w * 4).div_ceil(256) * 256; // COPY_BYTES_PER_ROW_ALIGNMENT
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("screenshot readback"),
                size: (row * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            (path, buf, row, w, h)
        });

        self.queue.submit(std::iter::once(encoder.finish()));

        if let Some((path, buf, row, w, h)) = capture {
            self.write_capture(&path, &buf, row, w, h);
        }

        self.queue.present(frame);
    }
}

impl Renderer {
    /// Map the readback buffer and write it out. Blocks on the GPU, which is
    /// fine: taking a screenshot is explicitly not on the hot path.
    fn write_capture(
        &self,
        path: &std::path::Path,
        buf: &wgpu::Buffer,
        row: u32,
        w: u32,
        h: u32,
    ) {
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        if self.device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
            eprintln!("[loudstone] screenshot: device poll failed");
            return;
        }
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => {
                eprintln!("[loudstone] screenshot: buffer map failed");
                return;
            }
        }

        let Ok(data) = slice.get_mapped_range() else {
            eprintln!("[loudstone] screenshot: could not read mapped range");
            buf.unmap();
            return;
        };
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        for y in 0..h as usize {
            let src = &data[y * row as usize..y * row as usize + (w as usize) * 4];
            let dst = &mut rgba[y * (w as usize) * 4..(y + 1) * (w as usize) * 4];
            if bgra {
                for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            } else {
                dst.copy_from_slice(src);
                for px in dst.chunks_exact_mut(4) {
                    px[3] = 255;
                }
            }
        }
        drop(data);
        buf.unmap();

        match crate::screenshot::write_rgba_png(path, w, h, &rgba) {
            Ok(()) => println!("[loudstone] screenshot written to {}", path.display()),
            Err(e) => eprintln!("[loudstone] screenshot failed: {e}"),
        }
    }
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Twelve thin bars along the edges of a block, slightly inflated so they sit
/// just outside the surface instead of z-fighting with it.
fn wire_box(x: f32, y: f32, z: f32) -> (Vec<Vertex>, Vec<u32>) {
    let e = HIGHLIGHT_INFLATE;
    let t = 0.02; // bar half-thickness
    let (lo, hi) = (-e, 1.0 + e);
    let mut verts = Vec::new();
    let mut indices = Vec::new();

    // Each bar runs the full length of one axis, pinned at a corner of the other two.
    let mut bar = |ax: usize, a: f32, b: f32| {
        let mut min = [0.0f32; 3];
        let mut max = [0.0f32; 3];
        let (u, v) = ((ax + 1) % 3, (ax + 2) % 3);
        min[ax] = lo;
        max[ax] = hi;
        min[u] = a - t;
        max[u] = a + t;
        min[v] = b - t;
        max[v] = b + t;
        push_box(&mut verts, &mut indices, [x, y, z], min, max);
    };
    for &a in &[lo, hi] {
        for &b in &[lo, hi] {
            bar(0, a, b);
            bar(1, a, b);
            bar(2, a, b);
        }
    }
    (verts, indices)
}

fn push_box(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    min: [f32; 3],
    max: [f32; 3],
) {
    let c = |i: usize, hi: bool| origin[i] + if hi { max[i] } else { min[i] };
    let p = |xh: bool, yh: bool, zh: bool| [c(0, xh), c(1, yh), c(2, zh)];
    let corners = [
        p(false, false, false),
        p(true, false, false),
        p(true, false, true),
        p(false, false, true),
        p(false, true, false),
        p(true, true, false),
        p(true, true, true),
        p(false, true, true),
    ];
    // Counter-clockwise seen from outside, matching the terrain winding.
    const FACES: [[usize; 4]; 6] = [
        [4, 7, 6, 5], // +Y
        [0, 1, 2, 3], // -Y
        [3, 2, 6, 7], // +Z
        [1, 0, 4, 5], // -Z
        [2, 1, 5, 6], // +X
        [0, 3, 7, 4], // -X
    ];
    for f in FACES {
        let base = verts.len() as u32;
        for i in f {
            verts.push(Vertex {
                pos: corners[i],
                color: HIGHLIGHT_COLOR,
                light: 1.0,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// A box with per-face shading, so an untextured mob still reads as a solid.
fn push_box_shaded(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    size: [f32; 3],
    color: [f32; 3],
) {
    let p = |xh: bool, yh: bool, zh: bool| {
        [
            origin[0] + if xh { size[0] } else { 0.0 },
            origin[1] + if yh { size[1] } else { 0.0 },
            origin[2] + if zh { size[2] } else { 0.0 },
        ]
    };
    let corners = [
        p(false, false, false),
        p(true, false, false),
        p(true, false, true),
        p(false, false, true),
        p(false, true, false),
        p(true, true, false),
        p(true, true, true),
        p(false, true, true),
    ];
    const FACES: [[usize; 4]; 6] = [
        [4, 7, 6, 5],
        [0, 1, 2, 3],
        [3, 2, 6, 7],
        [1, 0, 4, 5],
        [2, 1, 5, 6],
        [0, 3, 7, 4],
    ];
    for (fi, f) in FACES.iter().enumerate() {
        let light = crate::config::FACE_SHADE[fi];
        let base = verts.len() as u32;
        for &i in f {
            verts.push(Vertex {
                pos: corners[i],
                color,
                light,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn create_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Extract the six frustum planes from a view-projection matrix (Gribb/Hartmann),
/// normals pointing inward, normalised.
fn frustum_planes(vp: glam::Mat4) -> [glam::Vec4; 6] {
    let m = vp.to_cols_array_2d();
    // glam is column-major, so row r of the matrix is m[c][r] across c.
    let row = |r: usize| glam::Vec4::new(m[0][r], m[1][r], m[2][r], m[3][r]);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let raw = [
        r3 + r0, // left
        r3 - r0, // right
        r3 + r1, // bottom
        r3 - r1, // top
        r2,      // near
        r3 - r2, // far
    ];
    raw.map(|p| {
        let n = glam::Vec3::new(p.x, p.y, p.z);
        let len = n.length();
        if len > 0.0 { p / len } else { p }
    })
}

fn sphere_in_frustum(planes: &[glam::Vec4; 6], center: glam::Vec3, radius: f32) -> bool {
    planes
        .iter()
        .all(|p| p.x * center.x + p.y * center.y + p.z * center.z + p.w >= -radius)
}
