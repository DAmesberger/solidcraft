//! Shape pipelines and the per-scene GPU state of appearance graphs (docs/appearance-graph.md §5.2).
//!
//! A pipeline per shape, made from naga-validated generated WGSL: on a worker thread where the
//! device makes pipelines from any thread (`compile`), else on the frame's thread, at most
//! [`COMPILES_PER_FRAME`] new ones a frame and none once the frame has taken [`FRAME_BUDGET_MS`];
//! at most [`CACHE_SHAPES`] kept, least recently drawn out first, never one drawn this frame. Until its
//! shape's pipeline exists (or when it failed, or the cache is full of shapes in use), a material
//! draws its base response through the constant-graph pipeline: a new material never stalls a
//! frame. A value edit rewrites only the page words that changed; nothing recompiles.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use solidcraft_viewport::appearance_graph::ir::{Ir, ShapeKey};

use super::compile::{Compiler, Done, Job, PipelineSpec};
use super::scene::{GraphAppearance, GraphScene, TextureSet};
use super::textures::ResidentTextures;
use super::{CACHE_SHAPES, COMPILES_PER_FRAME, FRAME_BUDGET_MS, OBJECT_VERTEX_SIZE, PAGE_BYTES, VERTEX_SIZE};
use crate::view::ibl::{StudioPreset, Studios};

/// Failed shapes remembered (so each logs once); forgotten all at once past this.
const FAILED_SHAPES: usize = 4096;

/// What the appearance-graph drawing did in the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AppearanceStats {
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
    /// The longest pipeline compile that finished this frame (ms; made on the worker where the
    /// device allows, else on the frame's thread).
    pub compile_ms: f64,
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

/// A placed mesh's material record, per placement (location 7, beside the placement's rows).
pub(crate) fn placed_record_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![7 => Uint32];
    wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Instance, attributes: &A }
}

pub(crate) fn vertex_entry(object: bool) -> &'static str {
    if object { "vs_mat_object" } else { "vs_mat" }
}

/// How a draw is made: over opaque faces, or blended (see-through: premultiplied over, far
/// sides first, then near sides, as the viewport's glass is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Variant {
    Opaque,
    Far,
    Near,
    /// An instanced mesh's placements, over its opaque faces.
    Placed,
}

impl Variant {
    fn of(blended: bool) -> &'static [Variant] {
        if blended { &[Variant::Far, Variant::Near] } else { &[Variant::Opaque] }
    }
}

enum Pipeline {
    Ready { pipeline: wgpu::RenderPipeline, used: u64 },
}

/// The renderer's material pipelines.
pub(crate) struct AppearancePipelines {
    /// The device, layout and target the pipelines are made for.
    spec: PipelineSpec,
    group3: wgpu::BindGroupLayout,
    /// Makes pipelines on a worker thread where the device allows (`compile`); started on first
    /// use, unless `threaded` is false.
    compiler: Option<Compiler>,
    /// Jobs sent to the worker and not finished.
    in_flight: HashSet<Job>,
    /// Use the worker where the device allows (tests that count compiles a frame turn it off).
    pub(crate) threaded: bool,
    shapes: HashMap<(ShapeKey, Variant), Pipeline>,
    failed: HashSet<ShapeKey>,
    /// The constant-graph pipelines (plain and object vertices), made when a scene first has
    /// material surfaces.
    fallback: HashMap<(bool, Variant), wgpu::RenderPipeline>,
    frame: u64,
    stats: AppearanceStats,
    /// Draws this frame that used the fallback (counted while drawing, which borrows `self`).
    fallback_draws: std::sync::atomic::AtomicU32,
    /// A stand-in for the four material textures (1 × 1 white, made with the fallback) and the
    /// samplers by wrap mode.
    blank: Option<wgpu::TextureView>,
    samplers: [wgpu::Sampler; 3],
    /// The resident material textures (`textures`).
    pub(crate) textures: ResidentTextures,
    /// Decodes the textures a scene gives bytes for, on a worker (made on first use).
    decoder: Option<std::sync::Mutex<super::TextureDecoder>>,
    /// Textures still decoding, as of the last frame.
    decoding: u32,
    /// Shapes kept and new pipelines a frame ([`CACHE_SHAPES`], [`COMPILES_PER_FRAME`]); tests
    /// lower them.
    pub(crate) capacity: usize,
    pub(crate) compiles_per_frame: usize,
    #[cfg(test)]
    pub(crate) no_time_budget: bool,
    /// Tests of the shading itself read `pbr_shade`'s output as it is: see-through draws are made
    /// as opaque ones, without blending.
    #[cfg(test)]
    pub(crate) unblended: bool,
}

impl AppearancePipelines {
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
        let group3 = crate::view::ibl::appearance_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_appearance"),
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
            sampler("sc_appearance_repeat", wgpu::AddressMode::Repeat),
            sampler("sc_appearance_clamp", wgpu::AddressMode::ClampToEdge),
            sampler("sc_appearance_mirror", wgpu::AddressMode::MirrorRepeat),
        ];
        AppearancePipelines {
            spec: PipelineSpec { device: device.clone(), layout, format, samples, depth },
            group3,
            compiler: None,
            in_flight: HashSet::new(),
            threaded: !cfg!(test),
            shapes: HashMap::new(),
            failed: HashSet::new(),
            fallback: HashMap::new(),
            frame: 0,
            stats: AppearanceStats::default(),
            fallback_draws: std::sync::atomic::AtomicU32::new(0),
            blank: None,
            capacity: CACHE_SHAPES,
            compiles_per_frame: COMPILES_PER_FRAME,
            samplers,
            textures: ResidentTextures::new(device),
            decoder: None,
            decoding: 0,
            #[cfg(test)]
            no_time_budget: false,
            #[cfg(test)]
            unblended: false,
        }
    }

    pub(crate) fn stats(&self) -> AppearanceStats {
        AppearanceStats { fallback_draws: self.fallback_draws.load(std::sync::atomic::Ordering::Relaxed), ..self.stats }
    }

    /// Shapes of the last frame's scene are still compiling: a frame drawn now would differ.
    pub(crate) fn wants_redraw(&self) -> bool {
        self.stats.pending > 0 || self.decoding > 0 || !self.in_flight.is_empty()
    }

    /// Ready this frame's pipelines for `scene`: the fallback when it first has material surfaces,
    /// then new shapes within the budget (`started`: when the frame began).
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &GraphBatches, started: web_time::Instant) {
        self.fallback_draws.store(0, std::sync::atomic::Ordering::Relaxed);
        self.frame += 1;
        let frame = self.frame;
        self.textures.next_frame();
        for set in scene.draws.iter().map(|d| d.set).chain(scene.placed.values().map(|p| p.set)) {
            for key in scene.set(set).iter().flatten() {
                self.textures.touch(key);
            }
        }
        self.stats = AppearanceStats { failed: u32::try_from(self.failed.len()).unwrap_or(u32::MAX), ..AppearanceStats::default() };
        self.decode(device, queue, scene);
        if scene.draws.is_empty() && scene.placed.is_empty() {
            self.stats.ready = u32::try_from(self.shapes.len()).unwrap_or(u32::MAX);
            return;
        }
        if self.blank.is_none() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sc_appearance_blank"),
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
        if self.threaded && self.compiler.is_none() {
            self.compiler = Compiler::start(&self.spec);
            self.threaded = self.compiler.is_some();
        }
        self.take_finished(frame);
        // The constant-graph pipelines a draw may fall back to, made the first time one is needed.
        let fallbacks: Vec<(bool, Variant)> = scene
            .draws
            .iter()
            .flat_map(|d| Variant::of(d.blended && !self.blends_off()).iter().map(move |v| (d.object, *v)))
            .chain(scene.placed.values().map(|p| (p.object, Variant::Placed)))
            .collect();
        for (object, variant) in fallbacks {
            let job = Job::Fallback(object, variant);
            if self.fallback.contains_key(&(object, variant)) || self.in_flight.contains(&job) {
                continue;
            }
            match &self.compiler {
                Some(c) if c.send(job, None) => {
                    self.in_flight.insert(job);
                }
                _ => {
                    let pipeline = self.spec.fallback(object, variant);
                    self.fallback.insert((object, variant), pipeline);
                }
            }
        }
        let mut wanted: Vec<((ShapeKey, Variant), &Arc<Ir>)> = Vec::new();
        for p in scene.placed.values() {
            let key = (p.shape, Variant::Placed);
            match self.shapes.get_mut(&key) {
                Some(Pipeline::Ready { used, .. }) => *used = frame,
                None if !self.failed.contains(&p.shape) && !wanted.iter().any(|(k, _)| *k == key) => wanted.push((key, &p.ir)),
                None => {}
            }
        }
        for draw in &scene.draws {
            for variant in Variant::of(draw.blended && !self.blends_off()) {
                let key = (draw.shape, *variant);
                match self.shapes.get_mut(&key) {
                    Some(Pipeline::Ready { used, .. }) => *used = frame,
                    None if !self.failed.contains(&draw.shape) && !wanted.iter().any(|(k, _)| *k == key) => wanted.push((key, &draw.ir)),
                    None => {}
                }
            }
        }
        let mut pending = 0u32;
        for (key, ir) in wanted {
            if self.failed.contains(&key.0) {
                continue;
            }
            let job = Job::Shape(key.0, key.1);
            if self.in_flight.contains(&job) {
                pending += 1;
                continue;
            }
            if self.compiler.is_some() {
                // Off the frame: queued while the cache has room for it (counting those queued).
                let queued = self.in_flight.iter().filter(|j| matches!(j, Job::Shape(..))).count();
                if self.shapes.len() + queued >= self.capacity.max(1) && !self.make_room(frame) {
                    pending += 1;
                    continue;
                }
                if self.compiler.as_ref().is_some_and(|c| c.send(job, Some(ir.clone()))) {
                    self.in_flight.insert(job);
                }
                pending += 1;
                continue;
            }
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
            let t = web_time::Instant::now();
            match self.spec.shape(ir, key.1) {
                Some(pipeline) => {
                    self.shapes.insert(key, Pipeline::Ready { pipeline, used: frame });
                }
                None => {
                    if self.failed.len() >= FAILED_SHAPES {
                        self.failed.clear();
                    }
                    self.failed.insert(key.0);
                }
            }
            self.stats.compile_ms = self.stats.compile_ms.max(t.elapsed().as_secs_f64() * 1e3);
            self.stats.compiled += 1;
        }
        self.stats.pending = pending;
        self.stats.ready = u32::try_from(self.shapes.len()).unwrap_or(u32::MAX);
        self.stats.failed = u32::try_from(self.failed.len()).unwrap_or(u32::MAX);
    }

    /// Keep one finished job of the worker: a shape (drawn from frame `frame` on), a failed
    /// shape or a fallback.
    fn keep(&mut self, done: Done, frame: u64) {
        self.in_flight.remove(&done.job);
        self.stats.compile_ms = self.stats.compile_ms.max(done.ms);
        match (done.job, done.pipeline) {
            (Job::Shape(shape, variant), Some(pipeline)) => {
                self.shapes.insert((shape, variant), Pipeline::Ready { pipeline, used: frame });
                self.stats.compiled += 1;
            }
            (Job::Shape(shape, _), None) => {
                if self.failed.len() >= FAILED_SHAPES {
                    self.failed.clear();
                }
                self.failed.insert(shape);
                self.stats.compiled += 1;
            }
            (Job::Fallback(object, variant), Some(pipeline)) => {
                self.fallback.insert((object, variant), pipeline);
            }
            (Job::Fallback(..), None) => {}
        }
    }

    /// Take the worker's finished pipelines.
    fn take_finished(&mut self, frame: u64) {
        let finished = self.compiler.as_ref().map(Compiler::finished).unwrap_or_default();
        for done in finished {
            self.keep(done, frame);
        }
    }

    /// Wait until the worker has finished every pipeline queued so far, or until `timeout` has
    /// passed: the next frame draws what it finished. For renders that must not show a base
    /// response, such as an offscreen image.
    pub(crate) fn wait(&self, timeout: std::time::Duration) {
        if let Some(c) = &self.compiler {
            c.wait(self.in_flight.len(), timeout);
        }
    }

    /// Wait for the worker's pipelines and keep them now (tests).
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        self.wait(std::time::Duration::from_secs(60));
        self.take_finished(self.frame);
    }

    /// Ask the decoder for the scene's textures that are neither resident nor decoding, and make
    /// resident what it has finished. A texture that fails to decode stays its `image` nodes'
    /// `default` (logged once; the decoder does not try it again).
    fn decode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &GraphBatches) {
        let wanted: Vec<_> = scene.sources.iter().filter(|(key, _)| self.textures.view(key).is_none()).collect();
        if wanted.is_empty() && self.decoder.as_mut().is_none_or(|d| d.get_mut().map_or(0, |d| d.pending()) == 0) {
            self.decoding = 0;
            return;
        }
        let (policy, caps) = (self.textures.policy, self.textures.caps);
        let slot = self.decoder.get_or_insert_with(|| std::sync::Mutex::new(super::TextureDecoder::new(policy, caps)));
        let decoder = match slot.get_mut() {
            Ok(d) => d,
            Err(e) => e.into_inner(),
        };
        for (key, bytes) in wanted {
            if !decoder.is_pending(key)
                && let Err(e) = decoder.request(key.clone(), bytes.as_ref().clone())
            {
                log::warn!("material texture {}: drawn as its default colour: {e}", key.sha256.get(..12).unwrap_or(""));
            }
        }
        let finished = decoder.poll();
        self.decoding = u32::try_from(decoder.pending()).unwrap_or(u32::MAX);
        for done in finished {
            match done {
                Ok(texture) => {
                    if let Err(e) = self.textures.install(device, queue, texture) {
                        log::warn!("material texture not made resident: {e}");
                    }
                }
                Err((key, e)) => log::warn!("material texture {}: drawn as its default colour: {e}", key.sha256.get(..12).unwrap_or("")),
            }
        }
    }

    fn blends_off(&self) -> bool {
        #[cfg(test)]
        return self.unblended;
        #[cfg(not(test))]
        false
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
    fn for_draw(&self, shape: &ShapeKey, object: bool, variant: Variant) -> Option<(&wgpu::RenderPipeline, bool)> {
        match self.shapes.get(&(*shape, variant)) {
            Some(Pipeline::Ready { pipeline, .. }) => Some((pipeline, false)),
            None => self.fallback.get(&(object, variant)).map(|f| (f, true)),
        }
    }

    /// Whether `shape` has its own pipeline.
    #[cfg(test)]
    pub(crate) fn is_ready(&self, shape: &ShapeKey) -> bool {
        self.shapes.keys().any(|(k, _)| k == shape)
    }
}

#[derive(Clone)]
struct PageGpu {
    buffer: wgpu::Buffer,
    words: Vec<u32>,
}

#[derive(Clone)]
struct DrawGpu {
    shape: ShapeKey,
    /// See-through: drawn blended, after the opaque draws.
    blended: bool,
    /// What its vertices hash to: an unchanged draw keeps its buffer.
    hash: u64,
    ir: Arc<Ir>,
    object: bool,
    page: usize,
    /// Its texture set in `GraphBatches::sets`.
    set: u32,
    buffer: wgpu::Buffer,
    vertices: u32,
}

/// A material that instanced meshes are shaded with ([`GraphBatches::place`]).
#[derive(Clone)]
struct PlacedGpu {
    shape: ShapeKey,
    ir: Arc<Ir>,
    object: bool,
    page: usize,
    set: u32,
}

/// One scene's material surfaces on the GPU: its pages, its draws and their group-3 binds.
#[derive(Clone, Default)]
pub(crate) struct GraphBatches {
    pages: Vec<PageGpu>,
    draws: Vec<DrawGpu>,
    /// The materials of the scene's instanced meshes.
    placed: HashMap<GraphAppearance, PlacedGpu>,
    /// The scene's texture sets (index 0 binds none).
    sets: Vec<TextureSet>,
    /// The latest scene's texture bytes, by key.
    sources: HashMap<super::TextureKey, Arc<Vec<u8>>>,
    /// Group-3 binds by (page, texture set).
    binds: HashMap<(usize, u32), wgpu::BindGroup>,
    /// The studio (preset, installation), pages and texture generation the binds were made for.
    bound: Option<(Option<StudioPreset>, u64, usize, u64)>,
}

impl GraphBatches {
    /// Upload `scene`: pages whose words changed are rewritten, the others kept, and only draws
    /// whose vertices changed are uploaded (a value edit writes pages alone). Returns the page and
    /// vertex bytes written.
    pub(crate) fn take(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &GraphScene) -> (u64, u64) {
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
                        label: Some("sc_appearance_page"),
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
        self.sources.clone_from(scene.texture_sources());
        let sets: Vec<TextureSet> = (0..=scene.draws().iter().map(|d| d.textures).max().unwrap_or(0)).map(|i| scene.texture_set(i)).collect();
        if sets != self.sets {
            self.sets = sets;
            self.bound = None;
        }
        let pages_written = written;
        written = 0;
        let mut old: HashMap<(ShapeKey, usize, u32, u64), DrawGpu> =
            std::mem::take(&mut self.draws).into_iter().map(|d| ((d.shape, d.page, d.set, d.hash), d)).collect();
        for draw in scene.draws() {
            let stride = GraphScene::stride(draw.object);
            let vertices = u32::try_from(draw.vertices.len() / stride).unwrap_or(0);
            let (Some(ir), Ok(page)) = (scene.shape(&draw.shape), usize::try_from(draw.page)) else { continue };
            if vertices < 3 || draw.vertices.len() as u64 > device.limits().max_buffer_size {
                continue;
            }
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                draw.vertices.hash(&mut h);
                h.finish()
            };
            if let Some(kept) = old.remove(&(draw.shape, page, draw.textures, hash)) {
                self.draws.push(DrawGpu { ir: ir.clone(), object: draw.object, blended: draw.blended, ..kept });
                continue;
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sc_appearance_vertices"),
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
                blended: draw.blended,
                hash,
                ir: ir.clone(),
                object: draw.object,
                page,
                set: draw.textures,
                buffer,
                vertices: vertices / 3 * 3,
            });
        }
        (pages_written, written)
    }

    /// The materials instanced meshes are shaded with ([`Self::take`]'s scene's): their shapes
    /// join the pipelines to make and their pages the binds. What a material needs per placement
    /// is its record, which comes back; a see-through or unknown material draws no overlay.
    pub(crate) fn place(&mut self, scene: &GraphScene, material: GraphAppearance) -> Option<u32> {
        let m = scene.placed_appearance(material).filter(|m| !m.blended)?;
        let ir = scene.shape(&m.shape)?.clone();
        let page = usize::try_from(m.page).ok()?;
        self.placed.insert(material, PlacedGpu { shape: m.shape, ir, object: m.object, page, set: m.textures });
        Some(m.record)
    }

    /// Forget the placed materials before a new scene's ([`Self::place`]).
    pub(crate) fn clear_placed(&mut self) {
        self.placed.clear();
    }

    /// Set the pipeline and bind group of `material`'s placements. False until they can draw.
    pub(crate) fn begin_placed(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &AppearancePipelines, material: GraphAppearance) -> bool {
        let Some(p) = self.placed.get(&material) else { return false };
        let (Some((pipeline, fallback)), Some(bind)) = (pipelines.for_draw(&p.shape, p.object, Variant::Placed), self.binds.get(&(p.page, p.set)))
        else {
            return false;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(3, bind, &[]);
        if fallback {
            pipelines.fallback_draws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        true
    }

    /// Texture set `i` (none past the scene's).
    fn set(&self, i: u32) -> &TextureSet {
        static NONE: TextureSet = [None, None, None, None];
        usize::try_from(i).ok().and_then(|i| self.sets.get(i)).unwrap_or(&NONE)
    }

    /// Make the group-3 binds (studio + page + textures) of every (page, texture set) drawn, when
    /// the studio, the pages or the resident textures changed. A texture not resident yet binds
    /// the 1 × 1 stand-in, so its `image` nodes draw their `default`.
    pub(crate) fn bind(&mut self, device: &wgpu::Device, materials: &AppearancePipelines, studios: &Studios, preset: Option<StudioPreset>) {
        let Some(blank) = materials.blank.as_ref().filter(|_| !self.draws.is_empty() || !self.placed.is_empty()) else { return };
        let want = (preset, studios.generation(), self.pages.len(), materials.textures.generation());
        if self.bound != Some(want) {
            self.binds.clear();
        }
        let samplers = [&materials.samplers[0], &materials.samplers[1], &materials.samplers[2]];
        let wanted: Vec<(usize, u32)> = self.draws.iter().map(|d| (d.page, d.set)).chain(self.placed.values().map(|p| (p.page, p.set))).collect();
        for (page_index, set_index) in wanted {
            if self.binds.contains_key(&(page_index, set_index)) {
                continue;
            }
            let Some(page) = self.pages.get(page_index) else { continue };
            let set = self.set(set_index);
            let view = |i: usize| set.get(i).and_then(Option::as_ref).and_then(|k| materials.textures.view(k)).unwrap_or(blank);
            let textures = [view(0), view(1), view(2), view(3)];
            let bind = studios.appearance_bind(device, &materials.group3, preset, &page.buffer, textures, samplers);
            self.binds.insert((page_index, set_index), bind);
        }
        self.bound = Some(want);
    }

    /// Draw every material surface: one draw per (shape, page, texture set); the opaque ones,
    /// then the see-through ones' far sides, then their near sides.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'static>, pipelines: &AppearancePipelines, tally: &crate::view::Tally) {
        let off = pipelines.blends_off();
        let order = [(false, Variant::Opaque), (true, Variant::Far), (true, Variant::Near)];
        let draws = order.iter().flat_map(|(blended, v)| self.draws.iter().filter(move |d| (d.blended && !off) == *blended).map(move |d| (d, *v)));
        for (draw, variant) in draws {
            let (Some((pipeline, fallback)), Some(bind)) =
                (pipelines.for_draw(&draw.shape, draw.object, variant), self.binds.get(&(draw.page, draw.set)))
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
}

fn words_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
