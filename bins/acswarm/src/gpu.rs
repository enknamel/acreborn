//! wgpu state: device, surface, pipeline, and static batched geometry.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::sky::Environment;

/// A batched vertex. `color.rgb` tints the texture; `color.a` is the
/// opacity, offset by [`Vertex::PRELIT`] for a vertex whose `rgb` is the
/// whole lighting term (interior geometry lit from its cell's lights at
/// build time) rather than a tint under the sun.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl Vertex {
    /// Added to `color.a` to mark a pre-lit vertex; the shader subtracts it
    /// back out and skips the sun.
    pub const PRELIT: f32 = 2.0;

    const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];

    /// The opacity, whether or not the vertex is pre-lit.
    pub fn opacity(&self) -> f32 {
        if self.color[3] >= Self::PRELIT {
            self.color[3] - Self::PRELIT
        } else {
            self.color[3]
        }
    }
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

/// Per-vertex terrain blend data, a second vertex buffer alongside
/// [`Vertex`] for [`MaterialKey::Terrain`] batches. Overlay words are
/// `texture layer | alpha layer << 8 | rotation << 16 | 1 << 31` (0 when
/// absent); the alpha layer is sampled at `cell_uv` rotated by quarter turns.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct TerrainBlend {
    /// Position within the cell: u east, v south, both in [0, 1].
    pub cell_uv: [f32; 2],
    /// Base texture layer, then three terrain overlay words.
    pub layers: [u32; 4],
    /// Two road overlay words.
    pub roads: [u32; 2],
}

impl TerrainBlend {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![4 => Float32x2, 5 => Uint32x4, 6 => Uint32x2];
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainBlend>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }

    /// An overlay word.
    pub fn overlay(texture: u8, alpha: u8, rotation: u8) -> u32 {
        1 << 31 | (rotation as u32 & 3) << 16 | (alpha as u32) << 8 | texture as u32
    }
}

/// One particle billboard: a quad `size` metres across centred at
/// `position`, tinted and faded by `color`. Camera-facing unless
/// [`ParticleInstance::FLAT`] is set in `flags`, when it lies in the
/// world's x/y plane instead: a mark drawn on the ground.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq)]
pub struct ParticleInstance {
    pub position: [f32; 3],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub flags: u32,
    /// Turn about the up axis, radians; a flat quad only.
    pub angle: f32,
    _pad: [u32; 2],
}

impl ParticleInstance {
    /// Lie flat in the world's x/y plane, facing up.
    pub const FLAT: u32 = 1;

    pub fn new(
        position: [f32; 3],
        size: [f32; 2],
        color: [f32; 4],
        flags: u32,
        angle: f32,
    ) -> Self {
        ParticleInstance {
            position,
            size,
            color,
            flags,
            angle,
            _pad: [0; 2],
        }
    }

    const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Uint32, 4 => Float32];
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ParticleInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

/// Particles sharing one sprite material and blend mode.
pub struct ParticleDraw {
    pub material: MaterialKey,
    /// Add light over the scene (fire, glows) instead of covering it.
    pub additive: bool,
    pub instances: Vec<ParticleInstance>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParticleGlobals {
    view_proj: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
    camera: [f32; 4],
    fog_color: [f32; 4],
    fog_params: [f32; 4],
}

struct ParticleBatch {
    instance_buf: wgpu::Buffer,
    count: u32,
    bind_group: std::rc::Rc<wgpu::BindGroup>,
    additive: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    /// xyz: camera position, w: seconds since start (for water animation).
    camera: [f32; 4],
    light_dir: [f32; 4],
    ambient: [f32; 4],
    sun_color: [f32; 4],
    fog_color: [f32; 4],
    /// x: fog start, y: fog end.
    fog_params: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    water_color: [f32; 4],
}

/// CPU-side batch: all triangles sharing one material.
#[derive(Default)]
pub struct Batch {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Terrain blend data parallel to `vertices`; empty for other materials.
    pub blend: Vec<TerrainBlend>,
}

impl Batch {
    pub fn push(&mut self, verts: &[Vertex], indices: &[u32]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(verts);
        self.indices.extend(indices.iter().map(|i| i + base));
    }

    /// Push terrain triangles with their blend data.
    pub fn push_terrain(&mut self, verts: &[Vertex], blend: &[TerrainBlend], indices: &[u32]) {
        debug_assert_eq!(verts.len(), blend.len());
        self.push(verts, indices);
        self.blend.extend_from_slice(blend);
    }

    /// Append another batch of the same material.
    pub fn append(&mut self, other: &Batch) {
        self.push(&other.vertices, &other.indices);
        self.blend.extend_from_slice(&other.blend);
    }

    fn is_terrain(&self) -> bool {
        !self.blend.is_empty()
    }
}

/// Key for a material: a texture image or a solid color.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MaterialKey {
    /// `id`: Surface (0x08), SurfaceTexture (0x05) or Texture (0x06) id.
    /// `tex`: replacement SurfaceTexture from an appearance swap, or 0.
    /// `palette`: hash of a composed palette registered by the scene, or 0.
    Texture {
        id: u32,
        tex: u32,
        palette: u64,
    },
    Solid(u32),
    /// Outdoor terrain: batches carry [`TerrainBlend`] data and draw with
    /// the layered terrain textures.
    Terrain,
    /// Not a batch material: asks the image callback for the `n`th terrain
    /// texture layer (None past the end).
    TerrainLayer(u32),
    /// Likewise for the `n`th alpha map layer.
    TerrainAlpha(u32),
    /// A water surface: the Region's water SurfaceTexture (0x05) id, or 0
    /// for a plain tint. Drawn translucently after everything opaque.
    Water(u32),
}

/// How a batch is drawn: opaque geometry writes depth; terrain uses its
/// own layered pipeline; translucent geometry is blended over it
/// afterwards without writing depth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawKind {
    Opaque,
    Terrain,
    Translucent,
    Water,
}

pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct DrawBatch {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    bind_group: std::rc::Rc<wgpu::BindGroup>,
    /// Second vertex buffer of [`TerrainBlend`]; drawn with the terrain pipeline.
    blend_buf: Option<wgpu::Buffer>,
    kind: DrawKind,
    /// World-space box around the batch, for frustum culling.
    bounds: (Vec3, Vec3),
}

/// What one frame cost: draw calls issued, geometry drawn, what the
/// culling skipped, and the CPU time spent encoding it. Read it back
/// with [`Gpu::stats`] after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub draw_calls: u32,
    pub triangles: u64,
    /// Static batches (per material per landblock) drawn and culled.
    pub batches: u32,
    pub batches_culled: u32,
    /// Object instances (one per Setup part) drawn and culled.
    pub instances: u32,
    pub instances_culled: u32,
    pub particles: u32,
    /// Milliseconds spent in `draw`: encoding and submitting.
    pub encode_ms: f32,
}

/// The view frustum as six inward-facing planes (`xyz` normal, `w`
/// offset; a point is inside when `n . p + w >= 0` for all six).
struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Gribb/Hartmann extraction from a column-major view-projection.
    fn from_view_proj(m: Mat4) -> Self {
        let r = |i: usize| Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]);
        let (r0, r1, r2, r3) = (r(0), r(1), r(2), r(3));
        let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        Frustum {
            planes: planes.map(|p| {
                let n = p.truncate().length();
                if n > 0.0 {
                    p / n
                } else {
                    p
                }
            }),
        }
    }

    /// The box has some part inside the frustum (conservative).
    fn sees_aabb(&self, lo: Vec3, hi: Vec3) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            // The box corner farthest along the plane normal.
            let far = Vec3::new(
                if n.x >= 0.0 { hi.x } else { lo.x },
                if n.y >= 0.0 { hi.y } else { lo.y },
                if n.z >= 0.0 { hi.z } else { lo.z },
            );
            n.dot(far) + p.w >= 0.0
        })
    }

    fn sees_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }
}

/// Objects whose bounding sphere spans less than this fraction of the
/// view are not drawn: at 60 degrees on an 800-pixel-tall view that is
/// under a pixel and a half of radius, so nothing visible goes missing.
const MIN_PROJECTED_RADIUS: f32 = 0.002;

/// Callback that draws an overlay onto the frame after the 3D pass.
pub type UiPaint<'a> =
    &'a mut dyn FnMut(&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder, &wgpu::TextureView);

pub struct Gpu {
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    particle_alpha_pipeline: wgpu::RenderPipeline,
    particle_add_pipeline: wgpu::RenderPipeline,
    particle_globals_buf: wgpu::Buffer,
    particle_globals_bg: wgpu::BindGroup,
    /// Billboards drawn after the water, replaced by `set_particles`.
    particles: Vec<ParticleBatch>,
    environment: Environment,
    start: Instant,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    terrain_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Clamp-to-edge, for the per-cell alpha maps.
    sampler_clamp: wgpu::Sampler,
    texture_bytes: std::cell::Cell<u64>,
    /// Vertex/index buffers created so far (count, bytes); a proxy for
    /// graphics memory, which on Metal is paid per allocation.
    buffer_stats: std::cell::Cell<(u64, u64)>,
    batches: Vec<DrawBatch>,
    /// Streamed landblocks, keyed by block id.
    blocks: HashMap<u32, Vec<DrawBatch>>,
    /// Uploaded materials by key: decoded and mip-mapped once, shared by
    /// every batch that uses them, with the texture bytes each holds.
    materials: std::cell::RefCell<HashMap<MaterialKey, (std::rc::Rc<wgpu::BindGroup>, u64)>>,
    /// Per-draw model matrices (dynamic uniform offsets); slot 0 is identity.
    models_buf: wgpu::Buffer,
    models_bg: wgpu::BindGroup,
    dynamic_instances: Vec<Instance>,
    player_instances: Vec<Instance>,
    /// The instance lists changed since their matrices were last uploaded.
    models_dirty: bool,
    /// Anything drawn changed since the last `draw` (instances, blocks,
    /// particles, environment): a frame with the same camera can be skipped.
    dirty: bool,
    /// Instances farther than this from the eye are not drawn (metres;
    /// `f32::INFINITY` draws everything the fog does not hide).
    draw_distance: f32,
    /// Textures wider or taller than this are uploaded from a smaller
    /// mip level (`u32::MAX`: never).
    max_texture: u32,
    stats: FrameStats,
    /// Persistent offscreen target for `render_offscreen`.
    offscreen: Option<wgpu::TextureView>,
}

/// One submesh uploaded in model space with its material.
pub struct GpuSub {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    bind_group: std::rc::Rc<wgpu::BindGroup>,
    translucent: bool,
}

/// A model-space mesh on the GPU, shared by any number of instances.
pub struct GpuMesh {
    pub subs: Vec<GpuSub>,
    /// Bounding sphere in mesh space: center, radius.
    pub bounds: (Vec3, f32),
    /// Triangles in mesh space for picking (all submeshes concatenated).
    pub pick_positions: Vec<Vec3>,
    pub pick_indices: Vec<u32>,
}

/// A drawn copy of a mesh with its own model matrix.
#[derive(Clone)]
pub struct Instance {
    pub mesh: std::rc::Rc<GpuMesh>,
    pub model: Mat4,
    /// Lighting term replacing the sun and ambient for an instance standing
    /// in a lit interior cell; `None` draws it under the sun.
    pub light: Option<Vec3>,
}

/// Bytes of one instance record in the models storage buffer: the model
/// matrix and a light vec4.
const MODEL_SIZE: u64 = 80;
/// Records in the models buffer (the identity in slot 0, then the
/// instances drawn this frame).
const MAX_INSTANCES: u64 = 8192;

impl Gpu {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        Self::create(Some(window), size.width, size.height)
    }

    /// Device without a window, for `render_to_png`.
    pub fn headless(width: u32, height: u32) -> Result<Self> {
        Self::create(None, width, height)
    }

    /// Push pending uploads to the GPU and free dropped resources. A
    /// window does this every frame by presenting; headless sessions must
    /// call it themselves, or every buffer created since the last submit
    /// (particles are re-uploaded each tick) stays alive with its staging
    /// copy until the final screenshot.
    pub fn flush(&self) {
        self.queue.submit(std::iter::empty());
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    fn create(window: Option<Arc<Window>>, width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = match window {
            Some(w) => Some(instance.create_surface(w)?),
            None => None,
        };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface.as_ref(),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter")?;
        tracing::info!("adapter: {:?}", adapter.get_info().name);
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("acswarm"),
                ..Default::default()
            }))?;
        let mut config = match &surface {
            Some(s) => s
                .get_default_config(&adapter, width.max(1), height.max(1))
                .context("surface unsupported")?,
            None => wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: wgpu::TextureFormat::Rgba8Unorm,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
                color_space: Default::default(),
            },
        };
        config.format = config.format.remove_srgb_suffix();
        config.present_mode = wgpu::PresentMode::AutoVsync;
        if let Some(s) = &surface {
            s.configure(&device, &config);
        }
        let depth = Self::make_depth(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let array_tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let terrain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                array_tex(2),
                array_tex(3),
                sampler_entry(4),
                sampler_entry(5),
            ],
        });
        // Instance records live in one storage buffer the vertex shader
        // indexes by instance index, so instances of a mesh draw in one
        // call and static geometry reads slot 0.
        let models_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(MODEL_SIZE),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline"),
            bind_group_layouts: &[
                Some(&globals_layout),
                Some(&material_layout),
                Some(&models_layout),
            ],
            immediate_size: 0,
        });
        let terrain_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrain pipeline"),
                bind_group_layouts: &[
                    Some(&globals_layout),
                    Some(&terrain_layout),
                    Some(&models_layout),
                ],
                immediate_size: 0,
            });
        let depth_state = |write: bool, compare: wgpu::CompareFunction| wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let make_pipeline = |label,
                             layout,
                             entries: (&str, &str),
                             buffers: &[_],
                             depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entries.0),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entries.1),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(
            "main",
            &pipeline_layout,
            ("vs_main", "fs_main"),
            &[Some(Vertex::layout())],
            depth_state(true, wgpu::CompareFunction::Less),
        );
        let terrain_pipeline = make_pipeline(
            "terrain",
            &terrain_pipeline_layout,
            ("vs_terrain", "fs_terrain"),
            &[Some(Vertex::layout()), Some(TerrainBlend::layout())],
            depth_state(true, wgpu::CompareFunction::Less),
        );
        let translucent_pipeline = make_pipeline(
            "translucent",
            &pipeline_layout,
            ("vs_main", "fs_main"),
            &[Some(Vertex::layout())],
            depth_state(false, wgpu::CompareFunction::Less),
        );
        let water_pipeline = make_pipeline(
            "water",
            &pipeline_layout,
            ("vs_main", "fs_water"),
            &[Some(Vertex::layout())],
            depth_state(false, wgpu::CompareFunction::Less),
        );
        // The sky covers the whole frame before anything else is drawn. It
        // only needs the globals, so its layout is a prefix of the main
        // one and the bind group stays set across the pipeline switch.
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Always)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Particles: their own small shader module and uniform (the
        // billboard axes), sharing the material layout for sprites.
        let particle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particles.wgsl").into()),
        });
        let particle_globals_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("particle globals"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let particle_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[Some(&particle_globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let make_particle_pipeline = |label, fs, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&particle_layout),
                vertex: wgpu::VertexState {
                    module: &particle_shader,
                    entry_point: Some("vs_particle"),
                    compilation_options: Default::default(),
                    buffers: &[Some(ParticleInstance::layout())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Less)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &particle_shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let particle_alpha_pipeline = make_particle_pipeline(
            "particles alpha",
            "fs_particle_alpha",
            wgpu::BlendState::ALPHA_BLENDING,
        );
        let particle_add_pipeline = make_particle_pipeline(
            "particles additive",
            "fs_particle_add",
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        );
        let particle_globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle globals"),
            size: std::mem::size_of::<ParticleGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let particle_globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle globals"),
            layout: &particle_globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: particle_globals_buf.as_entire_binding(),
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });
        let models_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("models"),
            size: MODEL_SIZE * MAX_INSTANCES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Slot 0: identity, lit by the sun (static geometry).
        queue.write_buffer(&models_buf, 0, &model_bytes(Mat4::IDENTITY, None));
        let models_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("models"),
            layout: &models_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: models_buf.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sampler_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Ok(Gpu {
            surface,
            device,
            queue,
            config,
            depth,
            pipeline,
            terrain_pipeline,
            translucent_pipeline,
            water_pipeline,
            sky_pipeline,
            particle_alpha_pipeline,
            particle_add_pipeline,
            particle_globals_buf,
            particle_globals_bg,
            particles: Vec::new(),
            environment: Environment::default(),
            start: Instant::now(),
            globals_buf,
            globals_bg,
            material_layout,
            terrain_layout,
            sampler,
            sampler_clamp,
            texture_bytes: std::cell::Cell::new(0),
            buffer_stats: Default::default(),
            batches: Vec::new(),
            blocks: HashMap::new(),
            materials: Default::default(),
            models_buf,
            models_bg,
            dynamic_instances: Vec::new(),
            player_instances: Vec::new(),
            models_dirty: false,
            dirty: true,
            draw_distance: f32::INFINITY,
            max_texture: u32::MAX,
            stats: FrameStats::default(),
            offscreen: None,
        })
    }

    fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width: config.width,
                    height: config.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(s) = &self.surface {
            s.configure(&self.device, &self.config);
        }
        self.depth = Self::make_depth(&self.device, &self.config);
        self.offscreen = None;
        self.dirty = true;
    }

    /// What the last `draw` cost.
    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    /// Something drawn changed since the last frame (the caller adds its
    /// own camera and overlay changes to decide whether to draw at all).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// How far from the eye object instances are still drawn, metres;
    /// 0 or less means no limit.
    pub fn set_draw_distance(&mut self, metres: f32) {
        let d = if metres > 0.0 { metres } else { f32::INFINITY };
        if d != self.draw_distance {
            self.draw_distance = d;
            self.dirty = true;
        }
    }

    /// Cap the size of the textures uploaded from now on (0 = no cap);
    /// materials already uploaded keep their size.
    pub fn set_max_texture(&mut self, size: u32) {
        self.max_texture = if size == 0 { u32::MAX } else { size };
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// Sky, fog and light colours for the frames that follow. The default
    /// is the Region's sunny midday.
    pub fn set_environment(&mut self, env: Environment) {
        self.environment = env;
        self.dirty = true;
    }

    /// Upload a material's texture (with a full mip chain) and return its bind group.
    /// Bytes of texture memory uploaded so far (all mip levels).
    pub fn texture_bytes(&self) -> u64 {
        self.texture_bytes.get()
    }

    /// (count, bytes) of vertex/index buffers created so far.
    pub fn buffer_stats(&self) -> (u64, u64) {
        self.buffer_stats.get()
    }

    fn make_buffer(&self, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let (n, b) = self.buffer_stats.get();
        self.buffer_stats.set((n + 1, b + contents.len() as u64));
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            })
    }

    /// Bytes a mip-mapped RGBA8 texture of this size takes.
    fn texture_size(width: u32, height: u32, layers: u32) -> u64 {
        width as u64 * height as u64 * layers as u64 * 4 * 4 / 3
    }

    fn make_material(&self, img: &Rgba) -> wgpu::BindGroup {
        // Over the size cap, upload from a smaller mip: a quarter of the
        // memory per halving, at the cost of sharpness up close.
        let mut capped;
        let mut img = img;
        while img.width.max(img.height) > self.max_texture && img.width.max(img.height) > 1 {
            let (w, h, px) = downsample(&img.pixels, img.width, img.height);
            capped = Rgba {
                width: w,
                height: h,
                pixels: px,
            };
            img = &capped;
        }
        self.texture_bytes
            .set(self.texture_bytes.get() + Self::texture_size(img.width, img.height, 1));
        let texture = self.make_texture(img.width, img.height, 1);
        self.upload_layer(&texture, 0, img);
        let view = texture.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// The terrain material: one texture array of the terrain textures and
    /// one of the alpha maps. Layers are resampled to the first layer's
    /// size when they differ.
    fn make_terrain_material(&self, layers: &[Rgba], alphas: &[Rgba]) -> wgpu::BindGroup {
        let array = |imgs: &[Rgba]| {
            let fallback = Rgba {
                width: 1,
                height: 1,
                pixels: vec![255, 0, 255, 255],
            };
            let imgs = if imgs.is_empty() {
                std::slice::from_ref(&fallback)
            } else {
                imgs
            };
            let (w, h) = (imgs[0].width, imgs[0].height);
            self.texture_bytes
                .set(self.texture_bytes.get() + Self::texture_size(w, h, imgs.len() as u32));
            let texture = self.make_texture(w, h, imgs.len() as u32);
            for (i, img) in imgs.iter().enumerate() {
                if img.width == w && img.height == h {
                    self.upload_layer(&texture, i as u32, img);
                } else {
                    self.upload_layer(&texture, i as u32, &resample(img, w, h));
                }
            }
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let layers = array(layers);
        let alphas = array(alphas);
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain"),
            layout: &self.terrain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&layers),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&alphas),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_clamp),
                },
            ],
        })
    }

    /// An RGBA8 texture (array) with a full mip chain.
    fn make_texture(&self, width: u32, height: u32, layers: u32) -> wgpu::Texture {
        let mip_levels = (32 - width.max(height).leading_zeros()).max(1);
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// Write an image and its CPU box-filtered mip chain into one layer.
    fn upload_layer(&self, texture: &wgpu::Texture, layer: u32, img: &Rgba) {
        let (mut w, mut h) = (img.width, img.height);
        let mut level_px: Option<Vec<u8>> = None;
        for level in 0..texture.mip_level_count() {
            let px: &[u8] = level_px.as_deref().unwrap_or(&img.pixels);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            if w == 1 && h == 1 {
                break;
            }
            let (nw, nh, next) = downsample(px, w, h);
            w = nw;
            h = nh;
            level_px = Some(next);
        }
    }

    /// Replace the scene with these batches. `materials` maps each key to
    /// its image (solid colors become 1x1 textures).
    pub fn set_scene(
        &mut self,
        batches: HashMap<MaterialKey, Batch>,
        materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) {
        self.batches = self.upload(batches, materials);
        self.dirty = true;
    }

    /// Add (or replace) one streamed landblock's geometry.
    pub fn add_block(
        &mut self,
        id: u32,
        batches: HashMap<MaterialKey, Batch>,
        materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) {
        let uploaded = self.upload(batches, materials);
        self.blocks.insert(id, uploaded);
        self.dirty = true;
    }

    pub fn material_count(&self) -> usize {
        self.materials.borrow().len()
    }

    pub fn instance_count(&self) -> usize {
        self.dynamic_instances.len() + self.player_instances.len()
    }

    /// Static batches held (the scene's plus every streamed block's).
    pub fn batch_count(&self) -> usize {
        self.batches.len() + self.blocks.values().map(Vec::len).sum::<usize>()
    }

    pub fn remove_block(&mut self, id: u32) {
        if self.blocks.remove(&id).is_some() {
            self.dirty = true;
        }
    }

    /// Drop the materials nothing draws any more (their last batch or
    /// mesh went with an unloaded landblock or an evicted mesh) and give
    /// their texture memory back. The terrain array stays: every outdoor
    /// block wants it. Returns how many were dropped.
    pub fn prune_materials(&mut self) -> usize {
        let mut freed = 0u64;
        let mut n = 0usize;
        self.materials.borrow_mut().retain(|k, (bg, bytes)| {
            let keep = matches!(k, MaterialKey::Terrain) || std::rc::Rc::strong_count(bg) > 1;
            if !keep {
                freed += *bytes;
                n += 1;
            }
            keep
        });
        self.texture_bytes
            .set(self.texture_bytes.get().saturating_sub(freed));
        n
    }

    /// Replace the server-object instances drawn each frame.
    pub fn set_dynamic_instances(&mut self, instances: Vec<Instance>) {
        self.dynamic_instances = instances;
        self.models_dirty = true;
        self.dirty = true;
    }

    /// Replace the player's own instances.
    pub fn set_player_instances(&mut self, instances: Vec<Instance>) {
        self.player_instances = instances;
        self.models_dirty = true;
        self.dirty = true;
    }

    /// Replace the particle billboards drawn each frame (grouped by sprite
    /// material and blend mode, e.g. by `particles::draws`). `materials`
    /// decodes any sprite not yet in the material cache.
    pub fn set_particles(
        &mut self,
        draws: Vec<ParticleDraw>,
        mut materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) {
        self.dirty = true;
        self.particles = draws
            .into_iter()
            .filter(|d| !d.instances.is_empty())
            .map(|d| ParticleBatch {
                instance_buf: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("particles"),
                        contents: bytemuck::cast_slice(&d.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                count: d.instances.len() as u32,
                bind_group: self.material(d.material, &mut materials),
                additive: d.additive,
            })
            .collect();
    }

    /// Upload a model-space mesh once; instances reference it by `Rc`.
    pub fn upload_mesh(
        &self,
        mesh: &ac_scene::model::Mesh,
        mut materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) -> std::rc::Rc<GpuMesh> {
        let mut subs = Vec::with_capacity(mesh.submeshes.len());
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for v in mesh.submeshes.iter().flat_map(|s| s.vertices.iter()) {
            lo = lo.min(v.position);
            hi = hi.max(v.position);
        }
        let bounds = if lo.x.is_finite() {
            let c = (lo + hi) * 0.5;
            let r = mesh
                .submeshes
                .iter()
                .flat_map(|s| s.vertices.iter())
                .map(|v| v.position.distance(c))
                .fold(0.0f32, f32::max);
            (c, r)
        } else {
            (Vec3::ZERO, 0.0)
        };
        let mut pick_positions = Vec::new();
        let mut pick_indices = Vec::new();
        for sub in &mesh.submeshes {
            let base = pick_positions.len() as u32;
            pick_positions.extend(sub.vertices.iter().map(|v| v.position));
            pick_indices.extend(sub.indices.iter().map(|i| i + base));
        }
        for sub in &mesh.submeshes {
            if sub.indices.is_empty() {
                continue;
            }
            let key = match sub.solid_color {
                Some(c) => MaterialKey::Solid(c),
                None => MaterialKey::Texture {
                    id: sub.surface_id,
                    tex: sub.texture_override.unwrap_or(0),
                    palette: sub.palette_hash,
                },
            };
            let bind_group = self.material(key, &mut materials);
            let alpha = 1.0 - sub.translucency.clamp(0.0, 1.0);
            let verts: Vec<Vertex> = sub
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position.to_array(),
                    normal: v.normal.to_array(),
                    uv: v.uv.to_array(),
                    color: [1.0, 1.0, 1.0, alpha],
                })
                .collect();
            let vertex_buf =
                self.make_buffer(bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
            let index_buf = self.make_buffer(
                bytemuck::cast_slice(&sub.indices),
                wgpu::BufferUsages::INDEX,
            );
            subs.push(GpuSub {
                vertex_buf,
                index_buf,
                index_count: sub.indices.len() as u32,
                bind_group,
                translucent: alpha < 1.0,
            });
        }
        std::rc::Rc::new(GpuMesh {
            subs,
            bounds,
            pick_positions,
            pick_indices,
        })
    }

    /// Cached material bind group for a key, decoding on first use.
    fn material(
        &self,
        key: MaterialKey,
        materials: &mut impl FnMut(MaterialKey) -> Option<Rgba>,
    ) -> std::rc::Rc<wgpu::BindGroup> {
        if let Some((bg, _)) = self.materials.borrow().get(&key) {
            return bg.clone();
        }
        let before = self.texture_bytes.get();
        let bg = match key {
            MaterialKey::Solid(argb) => self.make_material(&Rgba {
                width: 1,
                height: 1,
                pixels: vec![(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, 255],
            }),
            MaterialKey::Texture { .. }
            | MaterialKey::TerrainLayer(_)
            | MaterialKey::TerrainAlpha(_) => self.make_material(&materials(key).unwrap_or(Rgba {
                width: 1,
                height: 1,
                pixels: vec![255, 0, 255, 255],
            })),
            MaterialKey::Terrain => {
                let t0 = Instant::now();
                let layers: Vec<Rgba> = (0..)
                    .map_while(|i| materials(MaterialKey::TerrainLayer(i)))
                    .collect();
                let alphas: Vec<Rgba> = (0..)
                    .map_while(|i| materials(MaterialKey::TerrainAlpha(i)))
                    .collect();
                let t_decode = t0.elapsed();
                let bg = self.make_terrain_material(&layers, &alphas);
                tracing::debug!(
                    "terrain material: {} texture layers, {} alpha maps; decode {:.1} ms, upload {:.1} ms",
                    layers.len(),
                    alphas.len(),
                    t_decode.as_secs_f64() * 1e3,
                    (t0.elapsed() - t_decode).as_secs_f64() * 1e3
                );
                bg
            }
            // Water ripples come from the Region's water texture; without
            // one the surface is a flat tint.
            MaterialKey::Water(tex) => self.make_material(
                &(tex != 0)
                    .then(|| {
                        materials(MaterialKey::Texture {
                            id: tex,
                            tex: 0,
                            palette: 0,
                        })
                    })
                    .flatten()
                    .unwrap_or(Rgba {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 255, 255, 255],
                    }),
            ),
        };
        let bg = std::rc::Rc::new(bg);
        let bytes = self.texture_bytes.get() - before;
        self.materials.borrow_mut().insert(key, (bg.clone(), bytes));
        bg
    }

    fn upload(
        &self,
        batches: HashMap<MaterialKey, Batch>,
        mut materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) -> Vec<DrawBatch> {
        let mut out = Vec::new();
        let t0 = Instant::now();
        let mut t_materials = std::time::Duration::ZERO;
        let mut keys: Vec<_> = batches.keys().copied().collect();
        keys.sort_by_key(|k| match k {
            MaterialKey::Terrain => (0u8, 0, 0, 0),
            MaterialKey::Texture { id, tex, palette } => (1, *id, *tex, *palette),
            MaterialKey::Solid(c) => (2, *c, 0, 0),
            MaterialKey::TerrainLayer(i) | MaterialKey::TerrainAlpha(i) => (3, *i, 0, 0),
            MaterialKey::Water(t) => (4, *t, 0, 0),
        });
        for key in keys {
            let b = &batches[&key];
            if b.indices.is_empty() {
                continue;
            }
            let tm = Instant::now();
            let bind_group = self.material(key, &mut materials);
            t_materials += tm.elapsed();
            let vertex_buf = self.make_buffer(
                bytemuck::cast_slice(&b.vertices),
                wgpu::BufferUsages::VERTEX,
            );
            let index_buf =
                self.make_buffer(bytemuck::cast_slice(&b.indices), wgpu::BufferUsages::INDEX);
            let blend_buf = b.is_terrain().then(|| {
                self.make_buffer(bytemuck::cast_slice(&b.blend), wgpu::BufferUsages::VERTEX)
            });
            let kind = match key {
                MaterialKey::Water(_) => DrawKind::Water,
                MaterialKey::Terrain => DrawKind::Terrain,
                _ if b.vertices.first().is_some_and(|v| v.opacity() < 1.0) => DrawKind::Translucent,
                _ => DrawKind::Opaque,
            };
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for v in &b.vertices {
                let p = Vec3::from(v.position);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            out.push(DrawBatch {
                vertex_buf,
                index_buf,
                index_count: b.indices.len() as u32,
                bind_group,
                blend_buf,
                kind,
                bounds: (lo, hi),
            });
        }
        tracing::debug!(
            "uploaded {} batches in {:.1} ms ({:.1} ms materials)",
            out.len(),
            t0.elapsed().as_secs_f64() * 1e3,
            t_materials.as_secs_f64() * 1e3
        );
        out
    }

    pub fn render(
        &mut self,
        view_proj: Mat4,
        light_dir: Vec3,
        ui: Option<UiPaint<'_>>,
    ) -> Result<()> {
        let surface = self.surface.as_ref().context("no surface")?;
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                surface.configure(&self.device, &self.config);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.draw(&view, view_proj, light_dir);
        if let Some(ui) = ui {
            let mut enc = self.device.create_command_encoder(&Default::default());
            ui(&self.device, &self.queue, &mut enc, &view);
            self.queue.submit([enc.finish()]);
        }
        self.queue.present(frame);
        Ok(())
    }

    /// Render one frame offscreen and write it as a PNG.
    pub fn render_to_png(
        &mut self,
        view_proj: Mat4,
        light_dir: Vec3,
        path: &std::path::Path,
        ui: Option<UiPaint<'_>>,
    ) -> Result<()> {
        let (w, h) = (self.config.width, self.config.height);
        let t0 = Instant::now();
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        self.draw(&view, view_proj, light_dir);
        if let Some(ui) = ui {
            let mut enc = self.device.create_command_encoder(&Default::default());
            ui(&self.device, &self.queue, &mut enc, &view);
            self.queue.submit([enc.finish()]);
        }
        let bytes_per_row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let t_render = t0.elapsed();
        let data = slice.get_mapped_range()?;
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let r = &data[(row * bytes_per_row) as usize..][..(w * 4) as usize];
            for p in r.as_chunks::<4>().0 {
                if bgra {
                    pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
                } else {
                    pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
        drop(data);
        buf.unmap();
        // A screenshot is written once and read by a person or a test:
        // fast compression is plenty.
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            file,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Adaptive,
        );
        image::ImageEncoder::write_image(encoder, &pixels, w, h, image::ExtendedColorType::Rgba8)?;
        tracing::debug!(
            "render_to_png: draw and readback {:.1} ms, encode {:.1} ms",
            t_render.as_secs_f64() * 1e3,
            (t0.elapsed() - t_render).as_secs_f64() * 1e3
        );
        Ok(())
    }

    fn draw(&mut self, view: &wgpu::TextureView, view_proj: Mat4, light_dir: Vec3) {
        // Camera position and far plane from the view-projection alone:
        // the centre of the near plane is as good as the eye for fog and
        // fresnel, and fog must be opaque by the far plane so nothing pops.
        let inv = view_proj.inverse();
        let near = inv.project_point3(Vec3::ZERO);
        let far = inv.project_point3(Vec3::Z);
        let env = &self.environment;
        let fog_end = env.fog_end.min(near.distance(far) * 0.97).max(1.0);
        // The client's fog is not linear; a linear ramp from the Region's
        // start distance reads far too thick, so hold it off a while.
        let fog_start = env.fog_start.max(fog_end * 0.3).min(fog_end * 0.6);
        let v4 = |v: Vec3, w: f32| Vec4::from((v, w)).to_array();
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: inv.to_cols_array_2d(),
            camera: v4(near, self.start.elapsed().as_secs_f32()),
            light_dir: v4(light_dir.normalize(), 0.0),
            ambient: v4(env.ambient, 1.0),
            sun_color: v4(env.sun_color, 1.0),
            fog_color: v4(env.fog_color, 1.0),
            fog_params: [fog_start, fog_end, 0.0, 0.0],
            sky_zenith: v4(env.sky_zenith, 1.0),
            sky_horizon: v4(env.sky_horizon, 1.0),
            water_color: env.water_color.to_array(),
        };
        let clear = wgpu::Color {
            r: env.fog_color.x as f64,
            g: env.fog_color.y as f64,
            b: env.fog_color.z as f64,
            a: 1.0,
        };
        self.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        if !self.particles.is_empty() {
            // Billboard axes: where the near plane's x and y run in the world.
            let right = (inv.project_point3(Vec3::X) - near).normalize_or_zero();
            let up = (inv.project_point3(Vec3::Y) - near).normalize_or_zero();
            let pg = ParticleGlobals {
                view_proj: globals.view_proj,
                right: v4(right, 0.0),
                up: v4(up, 0.0),
                camera: globals.camera,
                fog_color: globals.fog_color,
                fog_params: globals.fog_params,
            };
            self.queue
                .write_buffer(&self.particle_globals_buf, 0, bytemuck::bytes_of(&pg));
        }
        // Visibility: static batches by their box, instances by their
        // sphere, plus the draw distance and a minimum size on screen.
        // `ACV_NO_CULL` draws everything, for before/after measurements.
        let cull = std::env::var_os("ACV_NO_CULL").is_none();
        let frustum = Frustum::from_view_proj(view_proj);
        let draw_distance = self.draw_distance;
        let visible_instance = |inst: &Instance| -> bool {
            if !cull {
                return true;
            }
            let (c, r) = inst.mesh.bounds;
            let m = inst.model;
            let scale = m
                .x_axis
                .truncate()
                .length()
                .max(m.y_axis.truncate().length())
                .max(m.z_axis.truncate().length());
            let (center, radius) = (m.transform_point3(c), r * scale);
            let dist = center.distance(near);
            if dist - radius > draw_distance {
                return false;
            }
            if dist > 1.0 && radius / dist < MIN_PROJECTED_RADIUS {
                return false;
            }
            frustum.sees_sphere(center, radius)
        };
        let mut stats = FrameStats::default();
        let t0 = Instant::now();
        // The instances drawn this frame, grouped by mesh so that every
        // copy of a mesh is one draw call: sort by mesh pointer, then
        // hand out consecutive record slots (from 1; 0 is the identity).
        let total = (self.dynamic_instances.len() + self.player_instances.len()) as u32;
        let mut visible: Vec<&Instance> = self
            .dynamic_instances
            .iter()
            .chain(self.player_instances.iter())
            .take(MAX_INSTANCES as usize - 1)
            .filter(|inst| visible_instance(inst))
            .collect();
        visible.sort_by_key(|inst| std::rc::Rc::as_ptr(&inst.mesh) as usize);
        stats.instances = visible.len() as u32;
        stats.instances_culled = total - stats.instances;
        // (mesh, first slot, count) runs of the sorted list.
        let mut groups: Vec<(&GpuMesh, u32, u32)> = Vec::new();
        for (i, inst) in visible.iter().enumerate() {
            let slot = i as u32 + 1;
            match groups.last_mut() {
                Some((m, _, n)) if std::ptr::eq(*m, &*inst.mesh) => *n += 1,
                _ => groups.push((&inst.mesh, slot, 1)),
            }
        }
        let records: Vec<u8> = visible
            .iter()
            .flat_map(|inst| model_bytes(inst.model, inst.light))
            .collect();
        if !records.is_empty() {
            self.queue
                .write_buffer(&self.models_buf, MODEL_SIZE, &records);
        }
        self.models_dirty = false;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            // Sky first: a full-screen triangle, no vertex buffer, no depth.
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            stats.draw_calls += 1;
            let hide_static = std::env::var_os("ACV_HIDE_STATIC").is_some();
            pass.set_bind_group(2, &self.models_bg, &[]);
            // Opaque geometry and the layered terrain write depth, then
            // translucent surfaces (glass, water) blend over them.
            for (pipeline, kind) in [
                (&self.pipeline, DrawKind::Opaque),
                (&self.terrain_pipeline, DrawKind::Terrain),
                (&self.translucent_pipeline, DrawKind::Translucent),
                (&self.water_pipeline, DrawKind::Water),
            ] {
                pass.set_pipeline(pipeline);
                // Static geometry is baked in world space: identity model
                // (slot 0, instance 0).
                let streamed = self.blocks.values().flat_map(|v| v.iter());
                let mut last_bg: *const wgpu::BindGroup = std::ptr::null();
                for b in self
                    .batches
                    .iter()
                    .chain(streamed)
                    .filter(|b| !hide_static && b.kind == kind)
                {
                    if cull && !frustum.sees_aabb(b.bounds.0, b.bounds.1) {
                        stats.batches_culled += 1;
                        continue;
                    }
                    stats.batches += 1;
                    // Batches are sorted by material at upload, so a
                    // repeated bind group can be skipped.
                    let bg: *const wgpu::BindGroup = &*b.bind_group;
                    if bg != last_bg {
                        pass.set_bind_group(1, &*b.bind_group, &[]);
                        last_bg = bg;
                    }
                    pass.set_vertex_buffer(0, b.vertex_buf.slice(..));
                    if let Some(blend) = &b.blend_buf {
                        pass.set_vertex_buffer(1, blend.slice(..));
                    }
                    pass.set_index_buffer(b.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..b.index_count, 0, 0..1);
                    stats.draw_calls += 1;
                    stats.triangles += b.index_count as u64 / 3;
                }
                if matches!(kind, DrawKind::Water | DrawKind::Terrain) {
                    continue;
                }
                // Instances: every copy of a mesh in one call, its
                // records at consecutive slots.
                let translucent = kind == DrawKind::Translucent;
                for &(mesh, first, count) in &groups {
                    for sub in mesh.subs.iter().filter(|s| s.translucent == translucent) {
                        let bg: *const wgpu::BindGroup = &*sub.bind_group;
                        if bg != last_bg {
                            pass.set_bind_group(1, &*sub.bind_group, &[]);
                            last_bg = bg;
                        }
                        pass.set_vertex_buffer(0, sub.vertex_buf.slice(..));
                        pass.set_index_buffer(sub.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..sub.index_count, 0, first..first + count);
                        stats.draw_calls += 1;
                        stats.triangles += sub.index_count as u64 / 3 * count as u64;
                    }
                }
            }
            // Particles last: blended over everything, alpha then additive.
            if !self.particles.is_empty() {
                pass.set_bind_group(0, &self.particle_globals_bg, &[]);
                for (pipeline, additive) in [
                    (&self.particle_alpha_pipeline, false),
                    (&self.particle_add_pipeline, true),
                ] {
                    pass.set_pipeline(pipeline);
                    for b in self.particles.iter().filter(|b| b.additive == additive) {
                        pass.set_bind_group(1, &*b.bind_group, &[]);
                        pass.set_vertex_buffer(0, b.instance_buf.slice(..));
                        pass.draw(0..6, 0..b.count);
                        stats.draw_calls += 1;
                        stats.particles += b.count;
                        stats.triangles += b.count as u64 * 2;
                    }
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        stats.encode_ms = t0.elapsed().as_secs_f32() * 1e3;
        self.stats = stats;
        self.dirty = false;
    }

    /// Draw one frame to a persistent offscreen target and wait for the
    /// GPU to finish it: what a windowed frame costs, without a window
    /// (for `--perf` in headless runs). Returns the wall time in ms.
    pub fn render_offscreen(
        &mut self,
        view_proj: Mat4,
        light_dir: Vec3,
        ui: Option<UiPaint<'_>>,
    ) -> Result<f32> {
        let t0 = Instant::now();
        if self.offscreen.is_none() {
            let (w, h) = (self.config.width, self.config.height);
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.offscreen = Some(target.create_view(&Default::default()));
        }
        let view = self.offscreen.clone().unwrap();
        self.draw(&view, view_proj, light_dir);
        if let Some(ui) = ui {
            let mut enc = self.device.create_command_encoder(&Default::default());
            ui(&self.device, &self.queue, &mut enc, &view);
            self.queue.submit([enc.finish()]);
        }
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(t0.elapsed().as_secs_f32() * 1e3)
    }
}

/// One instance's uniform: its model matrix, then the interior lighting
/// term with `w = 1` when it applies (`w = 0` draws under the sun).
fn model_bytes(model: Mat4, light: Option<Vec3>) -> [u8; MODEL_SIZE as usize] {
    let mut out = [0u8; MODEL_SIZE as usize];
    out[..64].copy_from_slice(bytemuck::bytes_of(&model.to_cols_array_2d()));
    let l = match light {
        Some(l) => Vec4::from((l, 1.0)),
        None => Vec4::ZERO,
    };
    out[64..].copy_from_slice(bytemuck::bytes_of(&l.to_array()));
    out
}

/// One RGBA8 mip level down: a 2x2 box filter (odd edges reuse their last
/// row or column). Returns the new width, height and pixels.
fn downsample(px: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let (w, h) = (w as usize, h as usize);
    let row = w * 4;
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    let avg = |a: &[u8], b: &[u8], c: &[u8], d: &[u8]| {
        [0, 1, 2, 3].map(|i| ((a[i] as u32 + b[i] as u32 + c[i] as u32 + d[i] as u32) / 4) as u8)
    };
    if w % 2 == 0 && h % 2 == 0 {
        // The common case: every output texel averages a full 2x2 block.
        for rows in px[..row * h].chunks_exact(row * 2) {
            let (r0, r1) = rows.split_at(row);
            let (r0, r1) = (r0.as_chunks::<8>().0, r1.as_chunks::<8>().0);
            for (a, b) in r0.iter().zip(r1) {
                out.extend_from_slice(&avg(&a[..4], &a[4..], &b[..4], &b[4..]));
            }
        }
    } else {
        for y in 0..nh as usize {
            let r0 = &px[(y * 2).min(h - 1) * row..][..row];
            let r1 = &px[(y * 2 + 1).min(h - 1) * row..][..row];
            for x in 0..nw as usize {
                let x0 = (x * 2).min(w - 1) * 4;
                let x1 = (x * 2 + 1).min(w - 1) * 4;
                out.extend_from_slice(&avg(
                    &r0[x0..x0 + 4],
                    &r0[x1..x1 + 4],
                    &r1[x0..x0 + 4],
                    &r1[x1..x1 + 4],
                ));
            }
        }
    }
    (nw, nh, out)
}

/// Nearest-neighbour resample, for texture-array layers of unequal size.
fn resample(img: &Rgba, width: u32, height: u32) -> Rgba {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let sy = (y as u64 * img.height as u64 / height as u64) as u32;
        for x in 0..width {
            let sx = (x as u64 * img.width as u64 / width as u64) as u32;
            let o = ((sy * img.width + sx) * 4) as usize;
            pixels.extend_from_slice(&img.pixels[o..o + 4]);
        }
    }
    Rgba {
        width,
        height,
        pixels,
    }
}
