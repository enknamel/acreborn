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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
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
}

impl Batch {
    pub fn push(&mut self, verts: &[Vertex], indices: &[u32]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(verts);
        self.indices.extend(indices.iter().map(|i| i + base));
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
    /// A water surface: the Region's water SurfaceTexture (0x05) id, or 0
    /// for a plain tint. Drawn translucently after everything opaque.
    Water(u32),
}

/// How a batch is drawn: opaque geometry writes depth; translucent
/// geometry is blended over it afterwards without writing depth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawKind {
    Opaque,
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
    kind: DrawKind,
}

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
    translucent_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    environment: Environment,
    start: Instant,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    batches: Vec<DrawBatch>,
    /// Streamed landblocks, keyed by block id.
    blocks: HashMap<u32, Vec<DrawBatch>>,
    /// Uploaded materials by key: decoded and mip-mapped once, shared by
    /// every batch that uses them.
    materials: std::cell::RefCell<HashMap<MaterialKey, std::rc::Rc<wgpu::BindGroup>>>,
    /// Per-draw model matrices (dynamic uniform offsets); slot 0 is identity.
    models_buf: wgpu::Buffer,
    models_bg: wgpu::BindGroup,
    dynamic_instances: Vec<Instance>,
    player_instances: Vec<Instance>,
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
}

const MODEL_STRIDE: u64 = 256;
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
                label: Some("acviewer"),
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
        let models_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
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
        let depth_state = |write: bool, compare: wgpu::CompareFunction| wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let make_pipeline = |label: &str, fs: &str, depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(Vertex::layout())],
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
                    entry_point: Some(fs),
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
            "fs_main",
            depth_state(true, wgpu::CompareFunction::Less),
        );
        let translucent_pipeline = make_pipeline(
            "translucent",
            "fs_main",
            depth_state(false, wgpu::CompareFunction::Less),
        );
        let water_pipeline = make_pipeline(
            "water",
            "fs_water",
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
            size: MODEL_STRIDE * MAX_INSTANCES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &models_buf,
            0,
            bytemuck::bytes_of(&Mat4::IDENTITY.to_cols_array_2d()),
        );
        let models_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("models"),
            layout: &models_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &models_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
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
        Ok(Gpu {
            surface,
            device,
            queue,
            config,
            depth,
            pipeline,
            translucent_pipeline,
            water_pipeline,
            sky_pipeline,
            environment: Environment::default(),
            start: Instant::now(),
            globals_buf,
            globals_bg,
            material_layout,
            sampler,
            batches: Vec::new(),
            blocks: HashMap::new(),
            materials: Default::default(),
            models_buf,
            models_bg,
            dynamic_instances: Vec::new(),
            player_instances: Vec::new(),
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
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// Sky, fog and light colours for the frames that follow. The default
    /// is the Region's sunny midday.
    pub fn set_environment(&mut self, env: Environment) {
        self.environment = env;
    }

    /// Upload a material's texture (with a full mip chain) and return its bind group.
    fn make_material(&self, img: &Rgba) -> wgpu::BindGroup {
        let mip_levels = (32 - img.width.max(img.height).leading_zeros()).max(1);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: img.width,
                height: img.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // CPU box-filter mip chain.
        let (mut w, mut h, mut px) = (img.width, img.height, img.pixels.clone());
        for level in 0..mip_levels {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
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
            let nw = (w / 2).max(1);
            let nh = (h / 2).max(1);
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0u32; 4];
                    let mut n = 0;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(w - 1);
                            let sy = (y * 2 + dy).min(h - 1);
                            let o = ((sy * w + sx) * 4) as usize;
                            for c in 0..4 {
                                acc[c] += px[o + c] as u32;
                            }
                            n += 1;
                        }
                    }
                    let o = ((y * nw + x) * 4) as usize;
                    for c in 0..4 {
                        next[o + c] = (acc[c] / n) as u8;
                    }
                }
            }
            w = nw;
            h = nh;
            px = next;
        }
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

    /// Replace the scene with these batches. `materials` maps each key to
    /// its image (solid colors become 1x1 textures).
    pub fn set_scene(
        &mut self,
        batches: HashMap<MaterialKey, Batch>,
        materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) {
        self.batches = self.upload(batches, materials);
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
    }

    pub fn remove_block(&mut self, id: u32) {
        self.blocks.remove(&id);
    }

    /// Replace the server-object instances drawn each frame.
    pub fn set_dynamic_instances(&mut self, instances: Vec<Instance>) {
        self.dynamic_instances = instances;
    }

    /// Replace the player's own instances.
    pub fn set_player_instances(&mut self, instances: Vec<Instance>) {
        self.player_instances = instances;
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
            let vertex_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let index_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&sub.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
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
        if let Some(bg) = self.materials.borrow().get(&key).cloned() {
            return bg;
        }
        let img = match key {
            MaterialKey::Solid(argb) => Rgba {
                width: 1,
                height: 1,
                pixels: vec![(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, 255],
            },
            MaterialKey::Texture { .. } => match materials(key) {
                Some(i) => i,
                None => Rgba {
                    width: 1,
                    height: 1,
                    pixels: vec![255, 0, 255, 255],
                },
            },
            // Water ripples come from the Region's water texture; without
            // one the surface is a flat tint.
            MaterialKey::Water(tex) => (tex != 0)
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
        };
        let bg = std::rc::Rc::new(self.make_material(&img));
        self.materials.borrow_mut().insert(key, bg.clone());
        bg
    }

    fn upload(
        &self,
        batches: HashMap<MaterialKey, Batch>,
        mut materials: impl FnMut(MaterialKey) -> Option<Rgba>,
    ) -> Vec<DrawBatch> {
        let mut out = Vec::new();
        let mut keys: Vec<_> = batches.keys().copied().collect();
        keys.sort_by_key(|k| match k {
            MaterialKey::Texture { id, tex, palette } => (0u8, *id, *tex, *palette),
            MaterialKey::Solid(c) => (1, *c, 0, 0),
            MaterialKey::Water(t) => (2, *t, 0, 0),
        });
        for key in keys {
            let b = &batches[&key];
            if b.indices.is_empty() {
                continue;
            }
            let bind_group = self.material(key, &mut materials);
            let vertex_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&b.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let index_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&b.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
            let kind = match key {
                MaterialKey::Water(_) => DrawKind::Water,
                _ if b.vertices.iter().any(|v| v.color[3] < 1.0) => DrawKind::Translucent,
                _ => DrawKind::Opaque,
            };
            out.push(DrawBatch {
                vertex_buf,
                index_buf,
                index_count: b.indices.len() as u32,
                bind_group,
                kind,
            });
        }
        tracing::debug!("uploaded {} batches", out.len());
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
        let data = slice.get_mapped_range()?;
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let r = &data[(row * bytes_per_row) as usize..][..(w * 4) as usize];
            for p in r.chunks_exact(4) {
                if bgra {
                    pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
                } else {
                    pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
        drop(data);
        buf.unmap();
        image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8)?;
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
        // Model matrices for this frame's instances (slot 0 stays identity).
        let count = self.dynamic_instances.len() + self.player_instances.len();
        let mut mats = Vec::with_capacity(MODEL_STRIDE as usize * count);
        for inst in self
            .dynamic_instances
            .iter()
            .chain(self.player_instances.iter())
            .take(MAX_INSTANCES as usize - 1)
        {
            let m = inst.model.to_cols_array_2d();
            mats.extend_from_slice(bytemuck::bytes_of(&m));
            mats.resize(mats.len() + (MODEL_STRIDE as usize - 64), 0);
        }
        if !mats.is_empty() {
            self.queue
                .write_buffer(&self.models_buf, MODEL_STRIDE, &mats);
        }
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
            let hide_static = std::env::var_os("ACV_HIDE_STATIC").is_some();
            let instances = || {
                self.dynamic_instances
                    .iter()
                    .chain(self.player_instances.iter())
                    .enumerate()
            };
            // Opaque geometry writes depth, then translucent surfaces
            // (glass, water) blend over it in the order they were built.
            for (pipeline, kind) in [
                (&self.pipeline, DrawKind::Opaque),
                (&self.translucent_pipeline, DrawKind::Translucent),
                (&self.water_pipeline, DrawKind::Water),
            ] {
                pass.set_pipeline(pipeline);
                // Static geometry is baked in world space: identity model (slot 0).
                pass.set_bind_group(2, &self.models_bg, &[0]);
                let streamed = self.blocks.values().flat_map(|v| v.iter());
                for b in self
                    .batches
                    .iter()
                    .chain(streamed)
                    .filter(|b| !hide_static && b.kind == kind)
                {
                    pass.set_bind_group(1, &*b.bind_group, &[]);
                    pass.set_vertex_buffer(0, b.vertex_buf.slice(..));
                    pass.set_index_buffer(b.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..b.index_count, 0, 0..1);
                }
                if kind == DrawKind::Water {
                    continue;
                }
                // Instances: one model matrix each, via dynamic offset.
                for (i, inst) in instances() {
                    let slot = (i as u64 + 1).min(MAX_INSTANCES - 1) as u32;
                    let subs = inst
                        .mesh
                        .subs
                        .iter()
                        .filter(|s| s.translucent == (kind == DrawKind::Translucent));
                    let mut bound = false;
                    for sub in subs {
                        if !bound {
                            pass.set_bind_group(2, &self.models_bg, &[slot * MODEL_STRIDE as u32]);
                            bound = true;
                        }
                        pass.set_bind_group(1, &*sub.bind_group, &[]);
                        pass.set_vertex_buffer(0, sub.vertex_buf.slice(..));
                        pass.set_index_buffer(sub.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..sub.index_count, 0, 0..1);
                    }
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}
