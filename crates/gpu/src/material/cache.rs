//! Shape pipelines and the per-scene GPU state of material graphs (docs/material-graph.md §5.2).
//!
//! A pipeline per shape, made from naga-validated generated WGSL, at most [`COMPILES_PER_FRAME`]
//! new ones a frame and none once the frame has taken [`FRAME_BUDGET_MS`]; at most
//! [`CACHE_SHAPES`] kept, least recently drawn out first, never one drawn this frame. Until its
//! shape's pipeline exists (or when it failed, or the cache is full of shapes in use), a material
//! draws its base response through the constant-graph pipeline: a new material never stalls a
//! frame. A value edit rewrites only the page words that changed; nothing recompiles.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use solidcraft_render::material_graph::ir::{Ir, ShapeKey};

use super::scene::{GraphScene, TextureSet};
use super::textures::ResidentTextures;
use super::{CACHE_SHAPES, COMPILES_PER_FRAME, FRAME_BUDGET_MS, OBJECT_VERTEX_SIZE, PAGE_BYTES, VERTEX_SIZE};
use crate::view::ibl::{StudioPreset, Studios};

/// Failed shapes remembered (so each logs once); forgotten all at once past this.
const FAILED_SHAPES: usize = 4096;

/// What the material-graph drawing did in the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialStats {
    /// Shape pipelines created this frame.
    pub compiled: u32,
    /// Shape pipelines kept.
    pub ready: u32,
    /// Shapes whose code failed to generate or validate (drawn as their base response).
    pub failed: u32,
    /// Shapes in the scene still waiting for a pipeline.
    pub pending: u32,
    /// Draws this frame that used the base-response fallback.
    pub fallback_draws: u32,
}

/// The material vertex layouts: [`VERTEX_SIZE`] and, for object-space shapes, [`OBJECT_VERTEX_SIZE`].
pub(crate) fn vertex_layout(object: bool) -> wgpu::VertexBufferLayout<'static> {
    const PLAIN: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
    const OBJECT: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Float32x3];
    if object {
        wgpu::VertexBufferLayout { array_stride: OBJECT_VERTEX_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &OBJECT }
    } else {
        wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &PLAIN }
    }
}

pub(crate) fn vertex_entry(object: bool) -> &'static str {
    if object { "vs_mat_object" } else { "vs_mat" }
}

enum Pipeline {
    Ready { pipeline: wgpu::RenderPipeline, used: u64 },
}

/// The renderer's material pipelines.
pub(crate) struct Materials {
    layout: wgpu::PipelineLayout,
    group3: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    samples: u32,
    depth: Option<wgpu::DepthStencilState>,
    shapes: HashMap<ShapeKey, Pipeline>,
    failed: HashSet<ShapeKey>,
    /// The constant-graph pipelines (plain and object vertices), made when a scene first has
    /// material surfaces.
    fallback: Option<[wgpu::RenderPipeline; 2]>,
    frame: u64,
    stats: MaterialStats,
    /// Draws this frame that used the fallback (counted while drawing, which borrows `self`).
    fallback_draws: std::sync::atomic::AtomicU32,
    /// A stand-in for the four material textures (1 × 1 white, made with the fallback) and the
    /// samplers by wrap mode.
    blank: Option<wgpu::TextureView>,
    samplers: [wgpu::Sampler; 3],
    /// The resident material textures (`textures`).
    pub(crate) textures: ResidentTextures,
    /// Shapes kept and new pipelines a frame ([`CACHE_SHAPES`], [`COMPILES_PER_FRAME`]); tests
    /// lower them.
    pub(crate) capacity: usize,
    pub(crate) compiles_per_frame: usize,
    #[cfg(test)]
    pub(crate) no_time_budget: bool,
}

impl Materials {
    /// `groups` are the viewport's groups 0–2 (frame uniform, accessibility, occlusion); group 3
    /// is "studio + material". The pipelines draw into `format` with `samples` and `depth`, as the
    /// PBR pipeline does.
    pub(crate) fn new(
        device: &wgpu::Device,
        groups: [&wgpu::BindGroupLayout; 3],
        format: wgpu::TextureFormat,
        samples: u32,
        depth: Option<wgpu::DepthStencilState>,
    ) -> Self {
        let group3 = crate::view::ibl::material_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_material"),
            bind_group_layouts: &[Some(groups[0]), Some(groups[1]), Some(groups[2]), Some(&group3)],
            immediate_size: 0,
        });
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let samplers = [
            sampler("sc_material_repeat", wgpu::AddressMode::Repeat),
            sampler("sc_material_clamp", wgpu::AddressMode::ClampToEdge),
            sampler("sc_material_mirror", wgpu::AddressMode::MirrorRepeat),
        ];
        Materials {
            layout,
            group3,
            format,
            samples,
            depth,
            shapes: HashMap::new(),
            failed: HashSet::new(),
            fallback: None,
            frame: 0,
            stats: MaterialStats::default(),
            fallback_draws: std::sync::atomic::AtomicU32::new(0),
            blank: None,
            capacity: CACHE_SHAPES,
            compiles_per_frame: COMPILES_PER_FRAME,
            samplers,
            textures: ResidentTextures::new(device),
            #[cfg(test)]
            no_time_budget: false,
        }
    }

    pub(crate) fn stats(&self) -> MaterialStats {
        MaterialStats { fallback_draws: self.fallback_draws.load(std::sync::atomic::Ordering::Relaxed), ..self.stats }
    }

    /// Shapes of the last frame's scene are still compiling: a frame drawn now would differ.
    pub(crate) fn wants_redraw(&self) -> bool {
        self.stats.pending > 0
    }

    fn pipeline(&self, device: &wgpu::Device, label: &str, source: String, object: bool) -> wgpu::RenderPipeline {
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(vertex_entry(object)),
                buffers: &[Some(vertex_layout(object))],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: self.depth.clone(),
            multisample: wgpu::MultisampleState { count: self.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_mat"),
                targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Generate, validate and build a shape's pipeline; `None` (logged once) when it fails.
    fn compile(&self, device: &wgpu::Device, ir: &Ir) -> Option<wgpu::RenderPipeline> {
        let source = match super::shape_source(ir).and_then(|s| super::validate(&s).map(|_| s)) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("material graph shape {:02x?}: drawn as its base response: {e}", &ir.shape.0[..4]);
                return None;
            }
        };
        Some(self.pipeline(device, "sc_material_shape", source, ir.object_space))
    }

    /// Ready this frame's pipelines for `scene`: the fallback when it first has material surfaces,
    /// then new shapes within the budget (`started`: when the frame began).
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &GraphBatches, started: web_time::Instant) {
        self.fallback_draws.store(0, std::sync::atomic::Ordering::Relaxed);
        self.frame += 1;
        let frame = self.frame;
        self.textures.next_frame();
        for draw in &scene.draws {
            for key in scene.set(draw.set).iter().flatten() {
                self.textures.touch(key);
            }
        }
        self.stats = MaterialStats { failed: u32::try_from(self.failed.len()).unwrap_or(u32::MAX), ..MaterialStats::default() };
        if scene.draws.is_empty() {
            self.stats.ready = u32::try_from(self.shapes.len()).unwrap_or(u32::MAX);
            return;
        }
        if self.blank.is_none() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sc_material_blank"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &[255; 4],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            self.blank = Some(texture.create_view(&Default::default()));
        }
        if self.fallback.is_none() {
            let source = super::fallback_source();
            self.fallback = Some([false, true].map(|object| self.pipeline(device, "sc_material_base", source.clone(), object)));
        }
        let mut wanted: Vec<(&ShapeKey, &Arc<Ir>)> = Vec::new();
        for draw in &scene.draws {
            match self.shapes.get_mut(&draw.shape) {
                Some(Pipeline::Ready { used, .. }) => *used = frame,
                None if !self.failed.contains(&draw.shape) && !wanted.iter().any(|(k, _)| **k == draw.shape) => wanted.push((&draw.shape, &draw.ir)),
                None => {}
            }
        }
        let mut pending = 0u32;
        for (key, ir) in wanted {
            let over_time = {
                #[cfg(test)]
                let skip = self.no_time_budget;
                #[cfg(not(test))]
                let skip = false;
                !skip && started.elapsed().as_secs_f64() * 1e3 > FRAME_BUDGET_MS
            };
            if self.stats.compiled as usize >= self.compiles_per_frame || over_time || !self.make_room(frame) {
                pending += 1;
                continue;
            }
            match self.compile(device, ir) {
                Some(pipeline) => {
                    self.shapes.insert(*key, Pipeline::Ready { pipeline, used: frame });
                }
                None => {
                    if self.failed.len() >= FAILED_SHAPES {
                        self.failed.clear();
                    }
                    self.failed.insert(*key);
                }
            }
            self.stats.compiled += 1;
        }
        self.stats.pending = pending;
        self.stats.ready = u32::try_from(self.shapes.len()).unwrap_or(u32::MAX);
        self.stats.failed = u32::try_from(self.failed.len()).unwrap_or(u32::MAX);
    }

    /// Free a cache slot if one is needed, dropping the least recently drawn shape not drawn this
    /// frame; false when every kept shape is in use.
    fn make_room(&mut self, frame: u64) -> bool {
        if self.shapes.len() < self.capacity.max(1) {
            return true;
        }
        let oldest = self.shapes.iter().map(|(k, Pipeline::Ready { used, .. })| (*used, *k)).filter(|(used, _)| *used < frame).min();
        match oldest {
            Some((_, key)) => {
                self.shapes.remove(&key);
                true
            }
            None => false,
        }
    }

    /// The pipeline a draw of `shape` uses now: its own, else the fallback.
    fn for_draw(&self, shape: &ShapeKey, object: bool) -> Option<(&wgpu::RenderPipeline, bool)> {
        match self.shapes.get(shape) {
            Some(Pipeline::Ready { pipeline, .. }) => Some((pipeline, false)),
            None => self.fallback.as_ref().map(|f| (if object { &f[1] } else { &f[0] }, true)),
        }
    }

    /// Whether `shape` has its own pipeline.
    #[cfg(test)]
    pub(crate) fn is_ready(&self, shape: &ShapeKey) -> bool {
        self.shapes.contains_key(shape)
    }
}

struct PageGpu {
    buffer: wgpu::Buffer,
    words: Vec<u32>,
}

struct DrawGpu {
    shape: ShapeKey,
    ir: Arc<Ir>,
    object: bool,
    page: usize,
    /// Its texture set in `GraphBatches::sets`.
    set: u32,
    buffer: wgpu::Buffer,
    vertices: u32,
}

/// One scene's material surfaces on the GPU: its pages, its draws and their group-3 binds.
#[derive(Default)]
pub(crate) struct GraphBatches {
    pages: Vec<PageGpu>,
    draws: Vec<DrawGpu>,
    /// The scene's texture sets (index 0 binds none).
    sets: Vec<TextureSet>,
    /// Group-3 binds by (page, texture set).
    binds: HashMap<(usize, u32), wgpu::BindGroup>,
    /// The studio (preset, installation), pages and texture generation the binds were made for.
    bound: Option<(Option<StudioPreset>, u64, usize, u64)>,
}

impl GraphBatches {
    /// Upload `scene`: pages whose words changed are rewritten, the others kept; the vertices are
    /// uploaded anew. Returns the bytes written.
    pub(crate) fn take(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &GraphScene) -> u64 {
        let mut written = 0u64;
        let mut pages = std::mem::take(&mut self.pages);
        pages.truncate(scene.pages().len());
        let rebind = pages.len() != scene.pages().len();
        for (i, page) in scene.pages().iter().enumerate() {
            match pages.get_mut(i) {
                Some(old) if old.words == page.words => {}
                Some(old) => {
                    queue.write_buffer(&old.buffer, 0, &words_bytes(&page.words));
                    old.words.clone_from(&page.words);
                    written += PAGE_BYTES;
                }
                None => {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("sc_material_page"),
                        size: PAGE_BYTES,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    queue.write_buffer(&buffer, 0, &words_bytes(&page.words));
                    written += PAGE_BYTES;
                    pages.push(PageGpu { buffer, words: page.words.clone() });
                }
            }
        }
        self.pages = pages;
        if rebind {
            self.bound = None;
        }
        let sets: Vec<TextureSet> = (0..=scene.draws().iter().map(|d| d.textures).max().unwrap_or(0)).map(|i| scene.texture_set(i)).collect();
        if sets != self.sets {
            self.sets = sets;
            self.bound = None;
        }
        self.draws.clear();
        for draw in scene.draws() {
            let stride = GraphScene::stride(draw.object);
            let vertices = u32::try_from(draw.vertices.len() / stride).unwrap_or(0);
            let (Some(ir), Ok(page)) = (scene.shape(&draw.shape), usize::try_from(draw.page)) else { continue };
            if vertices < 3 || draw.vertices.len() as u64 > device.limits().max_buffer_size {
                continue;
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sc_material_vertices"),
                size: (draw.vertices.len() as u64).div_ceil(wgpu::COPY_BUFFER_ALIGNMENT) * wgpu::COPY_BUFFER_ALIGNMENT,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut bytes = draw.vertices.clone();
            bytes.resize(bytes.len().div_ceil(4) * 4, 0);
            queue.write_buffer(&buffer, 0, &bytes);
            written += bytes.len() as u64;
            self.draws.push(DrawGpu {
                shape: draw.shape,
                ir: ir.clone(),
                object: draw.object,
                page,
                set: draw.textures,
                buffer,
                vertices: vertices / 3 * 3,
            });
        }
        written
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Texture set `i` (none past the scene's).
    fn set(&self, i: u32) -> &TextureSet {
        static NONE: TextureSet = [None, None, None, None];
        usize::try_from(i).ok().and_then(|i| self.sets.get(i)).unwrap_or(&NONE)
    }

    /// Make the group-3 binds (studio + page + textures) of every (page, texture set) drawn, when
    /// the studio, the pages or the resident textures changed. A texture not resident yet binds
    /// the 1 × 1 stand-in, so its `image` nodes draw their `default`.
    pub(crate) fn bind(&mut self, device: &wgpu::Device, materials: &Materials, studios: &Studios, preset: Option<StudioPreset>) {
        let Some(blank) = materials.blank.as_ref().filter(|_| !self.draws.is_empty()) else { return };
        let want = (preset, studios.generation(), self.pages.len(), materials.textures.generation());
        if self.bound != Some(want) {
            self.binds.clear();
        }
        let samplers = [&materials.samplers[0], &materials.samplers[1], &materials.samplers[2]];
        for draw in &self.draws {
            if self.binds.contains_key(&(draw.page, draw.set)) {
                continue;
            }
            let Some(page) = self.pages.get(draw.page) else { continue };
            let set = self.set(draw.set);
            let view = |i: usize| set.get(i).and_then(Option::as_ref).and_then(|k| materials.textures.view(k)).unwrap_or(blank);
            let textures = [view(0), view(1), view(2), view(3)];
            let bind = studios.material_bind(device, &materials.group3, preset, &page.buffer, textures, samplers);
            self.binds.insert((draw.page, draw.set), bind);
        }
        self.bound = Some(want);
    }

    /// Draw every material surface: one draw per (shape, page, texture set).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'static>, pipelines: &Materials, tally: &crate::view::Tally) {
        for draw in &self.draws {
            let (Some((pipeline, fallback)), Some(bind)) = (pipelines.for_draw(&draw.shape, draw.object), self.binds.get(&(draw.page, draw.set)))
            else {
                continue;
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(3, bind, &[]);
            pass.set_vertex_buffer(0, draw.buffer.slice(..));
            pass.draw(0..draw.vertices, 0..1);
            tally.triangles(draw.vertices, 1);
            if fallback {
                pipelines.fallback_draws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// The surfaces' depth or occlusion term in an AO pass, with that pass's pipelines for plain
    /// and object vertices.
    pub(crate) fn draw_ao(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &[wgpu::RenderPipeline; 2]) {
        for draw in &self.draws {
            pass.set_pipeline(if draw.object { &pipelines[1] } else { &pipelines[0] });
            pass.set_vertex_buffer(0, draw.buffer.slice(..));
            pass.draw(0..draw.vertices, 0..1);
        }
    }
}

fn words_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
