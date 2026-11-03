//! Material textures on the GPU (docs/material-graph.md §4.3, §5.3): the decode worker, the
//! resident size, the upload and the mip chain built by render passes.
//!
//! - **Decoding** goes through `solidcraft_render::texture_input`, the one decoder, on a
//!   [`TextureDecoder`] worker thread (natively; the web has no threads, so it decodes one
//!   texture per [`TextureDecoder::poll`] on the window thread). Each design's textures share one
//!   decoded budget ([`DecodeBudget`]), charged from the headers before anything decodes.
//! - **Resident size** ([`ResidentPolicy`]): each texture is made resident at the largest level
//!   that fits the device's `max_texture_dimension_2d` and a GPU memory budget (native: the full
//!   size within 2 GiB; a downlevel device such as WebGL2: 2048 px within 256 MiB). An image above
//!   the device limit is box-filtered by powers of two at decode time (data preparation, once per
//!   load); over the budget, the least recently drawn textures drop a level first, on the GPU.
//! - **Mips** are drawn level by level by render passes (no compute, so WebGL2 builds them too):
//!   each level is the area average of the one above ([`MIP_SOURCE`]), sRGB colour averaged in
//!   linear light by the `Rgba8UnormSrgb` target itself. Rendering into its levels is
//!   colour-renderable in ES 3.0 (`SRGB8_ALPHA8`); a device that cannot ([`TextureCaps`]) builds
//!   the chain in a linear texture with the decode in the shader and copies it into the sRGB
//!   texture. Never CPU mips.
//! - **Sampling** ([`sampling_source`]): the `image` node's planar sample, at the level its
//!   footprint selects. A binding whose texture is not resident reads the 1 × 1 stand-in, which a
//!   resident texture never is (a 1 × 1 image is uploaded as 2 × 2, identical under every filter
//!   and wrap mode), and draws the node's `default` colour with alpha 0 until it is.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use solidcraft_render::material_graph::Constant;
use solidcraft_render::material_graph::ir::Ir;
use solidcraft_render::texture_input::{DecodeBudget, DecodedTexture, MAX_TEXTURE_EDGE, PixelLayout, TextureError, TextureSource};

use super::scene::TEXTURE_SLOTS;

/// The GPU memory budget for material textures on a native device.
pub const NATIVE_BUDGET: u64 = 2 << 30;
/// The edge on a downlevel device (WebGL2's limit).
pub const DOWNLEVEL_EDGE: u32 = 2048;
/// The GPU memory budget on a downlevel device.
pub const DOWNLEVEL_BUDGET: u64 = 256 << 20;
/// Dropping levels for the budget stops at this edge: smaller textures cost next to nothing.
pub const MIN_DROP_EDGE: u32 = 16;

/// A texture as one `image` binding reads it: the asset's SHA-256 and the node's colour space.
/// The same asset read in both spaces is two GPU textures.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureKey {
    pub sha256: String,
    pub srgb: bool,
}

/// The textures of a material of graph `ir`, by binding: a constant's asset, or the interface
/// input's texture in `values` (the caller merges a look's values over the interface defaults);
/// `None` where nothing supplies one (the `image` node draws its `default`).
pub fn texture_keys(ir: &Ir, values: &BTreeMap<String, Constant>) -> [Option<TextureKey>; TEXTURE_SLOTS] {
    let mut keys: [Option<TextureKey>; TEXTURE_SLOTS] = Default::default();
    for (slot, binding) in keys.iter_mut().zip(&ir.textures) {
        let sha = match (binding.sha256(), binding.interface()) {
            (Some(sha), _) => Some(sha.to_owned()),
            (None, Some(name)) => match values.get(name) {
                Some(Constant::Texture(t)) => Some(t.texture.clone()),
                _ => None,
            },
            (None, None) => None,
        };
        *slot = sha.filter(|s| is_sha256(s)).map(|sha256| TextureKey { sha256, srgb: binding.srgb });
    }
    keys
}

/// 64 lowercase hex digits: an asset key.
fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What a device holds of material textures: the largest edge and the GPU memory budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentPolicy {
    pub max_edge: u32,
    pub budget_bytes: u64,
}

impl ResidentPolicy {
    /// A native device with `max_texture_dimension_2d` = `max_edge`.
    pub fn native(max_edge: u32) -> Self {
        ResidentPolicy { max_edge: max_edge.clamp(1, MAX_TEXTURE_EDGE), budget_bytes: NATIVE_BUDGET }
    }
    /// A downlevel device (WebGL2).
    pub fn downlevel(max_edge: u32) -> Self {
        ResidentPolicy { max_edge: max_edge.clamp(1, DOWNLEVEL_EDGE), budget_bytes: DOWNLEVEL_BUDGET }
    }
    /// The policy of a device with `limits`: downlevel when it has no storage buffers (WebGL2).
    pub fn for_limits(limits: &wgpu::Limits) -> Self {
        if limits.max_storage_buffers_per_shader_stage == 0 {
            Self::downlevel(limits.max_texture_dimension_2d)
        } else {
            Self::native(limits.max_texture_dimension_2d)
        }
    }
    /// The path tracer's clamp (§4.3): its pixels are linear RGBA `f32` (16 B/px) without mips in
    /// one storage buffer, so the budget is the smaller of its binding size and the job's
    /// texture-pixel budget (`job::textures::MAX_TEXTURE_PIXELS_BYTES`).
    pub fn path(max_storage_buffer_binding_size: u64) -> Self {
        let budget = max_storage_buffer_binding_size.min(solidcraft_render::job::textures::MAX_TEXTURE_PIXELS_BYTES);
        let edge = (budget / PATH_BYTES_PER_PIXEL).isqrt();
        ResidentPolicy { max_edge: u32::try_from(edge).unwrap_or(MAX_TEXTURE_EDGE).clamp(1, MAX_TEXTURE_EDGE), budget_bytes: budget }
    }
    /// Levels to drop from a `width` × `height` image of `bytes_per_pixel` so it fits the edge and
    /// (with its mip chain when `mips`) the whole budget.
    pub fn levels_to_drop(&self, width: u32, height: u32, bytes_per_pixel: u64, mips: bool) -> u32 {
        let (mut w, mut h, mut dropped) = (width.max(1), height.max(1), 0);
        loop {
            let bytes = if mips { chain_bytes(w, h, bytes_per_pixel) } else { (u64::from(w) * u64::from(h)).saturating_mul(bytes_per_pixel) };
            if (w <= self.max_edge && h <= self.max_edge && bytes <= self.budget_bytes) || (w == 1 && h == 1) {
                return dropped;
            }
            (w, h, dropped) = ((w / 2).max(1), (h / 2).max(1), dropped + 1);
        }
    }
}

/// Bytes per pixel of the path tracer's texture words.
pub const PATH_BYTES_PER_PIXEL: u64 = 16;

/// Levels of a full chain from `width` × `height` through 1 × 1 (floor-halving).
pub fn mip_levels(width: u32, height: u32) -> u32 {
    width.max(height).max(1).ilog2() + 1
}

/// Bytes of a `width` × `height` texture with its full mip chain.
pub fn chain_bytes(width: u32, height: u32, bytes_per_pixel: u64) -> u64 {
    let (mut w, mut h, mut total) = (width.max(1), height.max(1), 0u64);
    loop {
        total = total.saturating_add((u64::from(w) * u64::from(h)).saturating_mul(bytes_per_pixel));
        if w == 1 && h == 1 {
            return total;
        }
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
}

/// Which formats a device renders into (and filters); checked per adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureCaps {
    /// `Rgba8UnormSrgb` levels can be drawn into (ES 3.0 core; WebGPU guarantees it).
    pub srgb_renderable: bool,
    /// `Rgba16Float` filters and can be drawn into (WebGL2 needs `EXT_color_buffer_float`).
    pub half_float_renderable: bool,
}

impl Default for TextureCaps {
    /// What every device can do: sRGB render targets; no half-float ones.
    fn default() -> Self {
        TextureCaps { srgb_renderable: true, half_float_renderable: false }
    }
}

impl TextureCaps {
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        let renders = |format| {
            let f = adapter.get_texture_format_features(format);
            f.allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
                && f.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
        };
        TextureCaps {
            srgb_renderable: renders(wgpu::TextureFormat::Rgba8UnormSrgb),
            half_float_renderable: renders(wgpu::TextureFormat::Rgba16Float),
        }
    }
}

/// The GPU format of a texture's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFormat {
    /// sRGB colour, decoded by the sampler (and encoded by the mip passes' target).
    Srgb8,
    /// Linear 8-bit data.
    Linear8,
    /// 16-bit height data as half floats, grey in r, g and b (so every channel mode reads it).
    Half,
}

impl GpuFormat {
    pub fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            GpuFormat::Srgb8 => wgpu::TextureFormat::Rgba8UnormSrgb,
            GpuFormat::Linear8 => wgpu::TextureFormat::Rgba8Unorm,
            GpuFormat::Half => wgpu::TextureFormat::Rgba16Float,
        }
    }
    pub fn bytes_per_pixel(self) -> u64 {
        if self == GpuFormat::Half { 8 } else { 4 }
    }
}

/// A texture ready to upload: level 0 at its resident size, in its GPU format. Data preparation,
/// made by [`prepare`] on the decode worker.
#[derive(Debug)]
pub struct PreparedTexture {
    pub key: TextureKey,
    pub width: u32,
    pub height: u32,
    pub format: GpuFormat,
    /// Level 0, rows top first, tightly packed.
    pub pixels: Vec<u8>,
    /// Levels dropped from the source to fit the policy.
    pub dropped: u32,
    /// The source's size.
    pub source: [u32; 2],
}

/// The role an `image` binding reads its bytes as: `solidcraft_render::texture_input::role_of`,
/// shared with the path tracer's graph textures.
pub use solidcraft_render::texture_input::role_of;

/// Decode `source` and make it resident-sized for `policy` on a device with `caps`: the
/// power-of-two box filter above the device limit and the budget, the GPU format's pixels, and a
/// 1 × 1 image widened to 2 × 2 (see the module notes).
pub fn prepare(
    key: TextureKey,
    source: &TextureSource,
    policy: ResidentPolicy,
    caps: TextureCaps,
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedTexture, TextureError> {
    let info = source.info().clone();
    let format = match (info.layout, key.srgb) {
        (PixelLayout::Gray16, _) if caps.half_float_renderable => GpuFormat::Half,
        (_, true) => GpuFormat::Srgb8,
        _ => GpuFormat::Linear8,
    };
    let drop = policy.levels_to_drop(info.width, info.height, format.bytes_per_pixel(), true);
    let mut texture = source.decode(cancelled)?;
    for _ in 0..drop {
        match texture.halve(cancelled)? {
            Some(next) => texture = next,
            None => break,
        }
    }
    let (width, height) = (texture.info().width, texture.info().height);
    let pixels = gpu_pixels(texture, format)?;
    let (width, height, pixels) = if width == 1 && height == 1 { (2, 2, pixels.repeat(4)) } else { (width, height, pixels) };
    Ok(PreparedTexture { key, width, height, format, pixels, dropped: drop, source: [info.width, info.height] })
}

fn gpu_pixels(texture: DecodedTexture, format: GpuFormat) -> Result<Vec<u8>, TextureError> {
    let layout = texture.info().layout;
    let bytes = texture.into_pixels();
    let grey = || bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b));
    Ok(match (layout, format) {
        (PixelLayout::Rgba8, _) => bytes,
        (PixelLayout::Gray16, GpuFormat::Half) => {
            let one = half_bits(1.0).to_le_bytes();
            let mut out = Vec::new();
            out.try_reserve_exact(bytes.len() * 4).map_err(|_| TextureError("cannot allocate texture pixels".into()))?;
            for v in grey() {
                let h = half_bits(f32::from(v) / 65535.0).to_le_bytes();
                out.extend_from_slice(&[h[0], h[1], h[0], h[1], h[0], h[1], one[0], one[1]]);
            }
            out
        }
        (PixelLayout::Gray16, _) => {
            // No half-float target: 8 bits of the height (documented precision loss).
            let mut out = Vec::new();
            out.try_reserve_exact(bytes.len() * 2).map_err(|_| TextureError("cannot allocate texture pixels".into()))?;
            for v in grey() {
                let c = ((f32::from(v) / 257.0).round()).clamp(0.0, 255.0) as u8;
                out.extend_from_slice(&[c, c, c, 255]);
            }
            out
        }
    })
}

/// IEEE half-float bits of `x` in [0, 1] (rounded to nearest even; subnormals kept).
pub fn half_bits(x: f32) -> u16 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 65504.0) };
    if x == 0.0 {
        return 0;
    }
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if exp <= 0 {
        // Subnormal: shift the implicit one in.
        let shift = (14 - exp) as u32;
        if shift > 24 {
            return 0;
        }
        let m = mant | 0x80_0000;
        let half = m >> shift;
        let rest = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let round = u32::from(rest > mid || (rest == mid && half & 1 == 1));
        return (half + round) as u16;
    }
    let half = ((exp as u32) << 10) | (mant >> 13);
    let rest = mant & 0x1fff;
    let round = u32::from(rest > 0x1000 || (rest == 0x1000 && half & 1 == 1));
    (half + round) as u16
}

/// A prepared texture, or why its key has none.
pub type Decoded = Result<PreparedTexture, (TextureKey, TextureError)>;

struct Job {
    key: TextureKey,
    source: TextureSource,
}

/// Decodes a design's textures off the window thread, one at a time (so at most one decoded image
/// is in flight), and hands back [`PreparedTexture`]s for [`crate::Renderer::install_material_texture`].
/// Dropping it cancels what is left.
pub struct TextureDecoder {
    policy: ResidentPolicy,
    caps: TextureCaps,
    budget: DecodeBudget,
    /// Keys requested and not yet returned, with what they charged.
    pending: HashMap<TextureKey, solidcraft_render::texture_input::TextureInfo>,
    /// Keys that failed (not retried until [`Self::forget`]).
    failed: HashSet<TextureKey>,
    cancel: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<(std::sync::mpsc::Sender<Job>, std::sync::mpsc::Receiver<Decoded>)>,
    #[cfg(target_arch = "wasm32")]
    queue: std::collections::VecDeque<Job>,
}

impl Drop for TextureDecoder {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl TextureDecoder {
    /// A decoder for one design on a device with `policy` and `caps`.
    pub fn new(policy: ResidentPolicy, caps: TextureCaps) -> Self {
        TextureDecoder {
            policy,
            caps,
            budget: DecodeBudget::design(),
            pending: HashMap::new(),
            failed: HashSet::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            #[cfg(not(target_arch = "wasm32"))]
            worker: None,
            #[cfg(target_arch = "wasm32")]
            queue: std::collections::VecDeque::new(),
        }
    }

    /// [`Self::new`] with a smaller decoded budget than a design's.
    pub fn with_budget(policy: ResidentPolicy, caps: TextureCaps, budget: DecodeBudget) -> Self {
        let mut decoder = Self::new(policy, caps);
        decoder.budget = budget;
        decoder
    }

    /// The design's decoded-pixel budget left.
    pub fn budget(&self) -> DecodeBudget {
        self.budget
    }

    /// Whether `key` is waiting to decode.
    pub fn is_pending(&self, key: &TextureKey) -> bool {
        self.pending.contains_key(key)
    }

    /// Textures still waiting.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Queue `key`'s encoded `bytes` (a design texture's, already checked against its key).
    /// The header is admitted here, and its decoded size charged to the design's budget, before
    /// anything decodes; a refusal is returned at once and the key is not retried.
    pub fn request(&mut self, key: TextureKey, bytes: Vec<u8>) -> Result<(), TextureError> {
        if self.pending.contains_key(&key) || self.failed.contains(&key) {
            return Ok(());
        }
        let role = role_of(&bytes, key.srgb);
        let admitted = TextureSource::new(bytes, role).and_then(|source| {
            self.budget.admit(source.info())?;
            Ok(source)
        });
        let source = match admitted {
            Ok(source) => source,
            Err(e) => {
                self.failed.insert(key);
                return Err(e);
            }
        };
        self.pending.insert(key.clone(), source.info().clone());
        self.enqueue(Job { key, source })
    }

    /// Let a failed `key` be requested again (its asset was replaced).
    pub fn forget(&mut self, key: &TextureKey) {
        self.failed.remove(key);
    }

    /// Release a texture's share of the budget once its design no longer uses it.
    pub fn release(&mut self, info: &solidcraft_render::texture_input::TextureInfo) {
        self.budget.release(info);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn enqueue(&mut self, job: Job) -> Result<(), TextureError> {
        if self.worker.is_none() {
            let (jobs, inbox) = std::sync::mpsc::channel::<Job>();
            let (results, outbox) = std::sync::mpsc::channel::<Decoded>();
            let (policy, caps, cancel) = (self.policy, self.caps, self.cancel.clone());
            std::thread::Builder::new()
                .name("solidcraft-texture-decode".into())
                .spawn(move || {
                    for job in inbox {
                        let cancelled = || cancel.load(Ordering::Relaxed);
                        if cancelled() {
                            return;
                        }
                        let done = prepare(job.key.clone(), &job.source, policy, caps, &cancelled).map_err(|e| (job.key, e));
                        if results.send(done).is_err() {
                            return;
                        }
                    }
                })
                .map_err(|e| TextureError(format!("the texture decode worker did not start: {e}")))?;
            self.worker = Some((jobs, outbox));
        }
        let sent = self.worker.as_ref().is_some_and(|(jobs, _)| jobs.send(job).is_ok());
        if sent { Ok(()) } else { Err(TextureError("the texture decode worker stopped".into())) }
    }

    #[cfg(target_arch = "wasm32")]
    fn enqueue(&mut self, job: Job) -> Result<(), TextureError> {
        self.queue.push_back(job);
        Ok(())
    }

    /// Textures finished since the last call (natively without waiting; on the web, decoding one).
    pub fn poll(&mut self) -> Vec<Decoded> {
        let mut done = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some((_, outbox)) = &self.worker {
            done.extend(outbox.try_iter());
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(job) = self.queue.pop_front() {
            let cancel = self.cancel.clone();
            let cancelled = move || cancel.load(Ordering::Relaxed);
            done.push(prepare(job.key.clone(), &job.source, self.policy, self.caps, &cancelled).map_err(|e| (job.key, e)));
        }
        for result in &done {
            let key = match result {
                Ok(t) => &t.key,
                Err((key, _)) => key,
            };
            if let Some(info) = self.pending.remove(key)
                && result.is_err()
            {
                self.budget.release(&info);
                self.failed.insert(key.clone());
            }
        }
        done
    }

    /// Wait for every queued texture (tests and headless captures; never the window thread).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn finish(&mut self, timeout: std::time::Duration) -> Vec<Decoded> {
        let deadline = web_time::Instant::now() + timeout;
        let mut done = Vec::new();
        while !self.pending.is_empty() && web_time::Instant::now() < deadline {
            let Some((_, outbox)) = &self.worker else { break };
            match outbox.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(result) => {
                    let key = match &result {
                        Ok(t) => t.key.clone(),
                        Err((key, _)) => key.clone(),
                    };
                    if let Some(info) = self.pending.remove(&key)
                        && result.is_err()
                    {
                        self.budget.release(&info);
                        self.failed.insert(key);
                    }
                    done.push(result);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        done
    }
}

/// The mip passes' shader: a full-screen triangle and the area average of the level above, read
/// texel by texel (`textureLoad`, so no sampler and no filtering rule is involved). Odd sides
/// weigh the source cells by their overlap, as `texture_sampling::mips` does. `fs_mip_srgb` is
/// the fallback for a device that cannot draw into sRGB levels: it decodes, averages and encodes
/// sRGB codes held in a linear texture.
pub const MIP_SOURCE: &str = include_str!("mips.wgsl");

/// What one resident texture is.
struct Entry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: GpuFormat,
    width: u32,
    height: u32,
    levels: u32,
    bytes: u64,
    used: u64,
}

/// What the resident textures are now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureStats {
    pub resident: u32,
    pub bytes: u64,
    /// Levels dropped since the store was made, for the budget.
    pub dropped_levels: u32,
}

/// The renderer's material textures.
pub(crate) struct ResidentTextures {
    pub(crate) policy: ResidentPolicy,
    pub(crate) caps: TextureCaps,
    entries: HashMap<TextureKey, Entry>,
    total: u64,
    frame: u64,
    generation: u64,
    dropped: u32,
    mips: Option<MipPasses>,
}

struct MipPasses {
    layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<(wgpu::TextureFormat, bool), wgpu::RenderPipeline>,
}

impl MipPasses {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_material_mip"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sc_material_mip"),
            source: wgpu::ShaderSource::Wgsl(MIP_SOURCE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_material_mip"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        MipPasses { layout, module, pipeline_layout, pipelines: HashMap::new() }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, srgb_codes: bool) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipeline_layout);
        self.pipelines.entry((format, srgb_codes)).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sc_material_mip"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs_mip"), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(if srgb_codes { "fs_mip_srgb" } else { "fs_mip" }),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Draw levels 1.. of `texture` (`levels` in all) from level 0.
    fn build(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, levels: u32, srgb_codes: bool) {
        let format = texture.format();
        let level = |i| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: i, mip_level_count: Some(1), ..Default::default() });
        for i in 1..levels {
            let (source, target) = (level(i - 1), level(i));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sc_material_mip"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source) }],
            });
            let pipeline = self.pipeline(device, format, srgb_codes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sc_material_mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::RENDER_ATTACHMENT)
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }
}

fn level_extent(width: u32, height: u32, level: u32) -> wgpu::Extent3d {
    extent((width >> level).max(1), (height >> level).max(1))
}

impl ResidentTextures {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        ResidentTextures {
            policy: ResidentPolicy::for_limits(&device.limits()),
            caps: TextureCaps::default(),
            entries: HashMap::new(),
            total: 0,
            frame: 0,
            generation: 0,
            dropped: 0,
            mips: None,
        }
    }

    /// Counts changes, so binds made before one are known stale.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn stats(&self) -> TextureStats {
        TextureStats { resident: u32::try_from(self.entries.len()).unwrap_or(u32::MAX), bytes: self.total, dropped_levels: self.dropped }
    }

    pub(crate) fn view(&self, key: &TextureKey) -> Option<&wgpu::TextureView> {
        self.entries.get(key).map(|e| &e.view)
    }

    /// The resident size of `key` ([width, height, levels]).
    #[cfg(test)]
    pub(crate) fn size(&self, key: &TextureKey) -> Option<[u32; 3]> {
        self.entries.get(key).map(|e| [e.width, e.height, e.levels])
    }

    /// A new frame: textures touched from now count as drawn in it.
    pub(crate) fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// `key` is drawn this frame.
    pub(crate) fn touch(&mut self, key: &TextureKey) {
        if let Some(e) = self.entries.get_mut(key) {
            e.used = self.frame;
        }
    }

    /// Remove `key`'s texture.
    pub(crate) fn remove(&mut self, key: &TextureKey) {
        if let Some(e) = self.entries.remove(key) {
            self.total = self.total.saturating_sub(e.bytes);
            self.generation += 1;
        }
    }

    /// Upload `t` (replacing its key's texture), build its mips on the GPU and keep the budget.
    pub(crate) fn install(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, t: PreparedTexture) -> Result<(), TextureError> {
        let limit = device.limits().max_texture_dimension_2d;
        if t.width == 0 || t.height == 0 || t.width > limit || t.height > limit {
            return Err(TextureError(format!("a {}×{} texture does not fit this device ({limit} px a side)", t.width, t.height)));
        }
        let bpp = t.format.bytes_per_pixel();
        if t.pixels.len() as u64 != u64::from(t.width) * u64::from(t.height) * bpp {
            return Err(TextureError("prepared pixels do not match their size".into()));
        }
        let levels = mip_levels(t.width, t.height);
        // A device that cannot draw into sRGB levels builds them in a linear twin (sRGB codes,
        // decoded in the shader) and copies them over.
        let twin = t.format == GpuFormat::Srgb8 && !self.caps.srgb_renderable;
        let format = t.format.wgpu();
        let make = |format: wgpu::TextureFormat, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sc_material_texture"),
                size: extent(t.width, t.height),
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let texture = make(format, if twin { USAGE - wgpu::TextureUsages::RENDER_ATTACHMENT } else { USAGE });
        let drawn = if twin { make(wgpu::TextureFormat::Rgba8Unorm, USAGE) } else { texture.clone() };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &drawn, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &t.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::try_from(u64::from(t.width) * bpp).unwrap_or(u32::MAX)),
                rows_per_image: Some(t.height),
            },
            extent(t.width, t.height),
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sc_material_mips") });
        self.mips.get_or_insert_with(|| MipPasses::new(device)).build(device, &mut encoder, &drawn, levels, twin);
        if twin {
            for level in 0..levels {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: &drawn, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    level_extent(t.width, t.height, level),
                );
            }
        }
        queue.submit([encoder.finish()]);
        let bytes = chain_bytes(t.width, t.height, bpp);
        let view = texture.create_view(&Default::default());
        self.remove(&t.key);
        let key = t.key.clone();
        self.entries.insert(t.key, Entry { texture, view, format: t.format, width: t.width, height: t.height, levels, bytes, used: self.frame });
        self.total = self.total.saturating_add(bytes);
        self.generation += 1;
        self.keep_budget(device, queue, &key);
        Ok(())
    }

    /// Drop a level from the least recently drawn textures (`newest` last) until the total fits.
    fn keep_budget(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, newest: &TextureKey) {
        while self.total > self.policy.budget_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| e.width.max(e.height) > MIN_DROP_EDGE)
                .min_by_key(|(k, e)| (*k == newest, e.used, std::cmp::Reverse(e.bytes)))
                .map(|(k, _)| k.clone());
            let Some(key) = victim else { return };
            if !self.drop_level(device, queue, &key) {
                return;
            }
        }
    }

    /// Replace `key`'s texture by its chain from level 1, copied on the GPU.
    fn drop_level(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: &TextureKey) -> bool {
        let Some(old) = self.entries.get(key) else { return false };
        if old.levels < 2 {
            return false;
        }
        let (width, height, levels) = ((old.width / 2).max(1), (old.height / 2).max(1), old.levels - 1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sc_material_texture"),
            size: extent(width, height),
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: old.texture.format(),
            usage: old.texture.usage(),
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sc_material_drop_level") });
        for level in 0..levels {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &old.texture,
                    mip_level: level + 1,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                level_extent(width, height, level),
            );
        }
        queue.submit([encoder.finish()]);
        let bytes = chain_bytes(width, height, old.format.bytes_per_pixel());
        let (format, used, old_bytes) = (old.format, old.used, old.bytes);
        let view = texture.create_view(&Default::default());
        self.entries.insert(key.clone(), Entry { texture, view, format, width, height, levels, bytes, used });
        self.total = self.total.saturating_sub(old_bytes).saturating_add(bytes);
        self.dropped += 1;
        self.generation += 1;
        true
    }

    /// Read level `level` of `key` back as tightly packed rows (tests).
    #[cfg(test)]
    pub(crate) fn read_level(&self, device: &wgpu::Device, queue: &wgpu::Queue, key: &TextureKey, level: u32) -> Option<(u32, u32, Vec<u8>)> {
        let e = self.entries.get(key)?;
        let size = level_extent(e.width, e.height, level);
        let bpp = u32::try_from(e.format.bytes_per_pixel()).ok()?;
        let row = (size.width * bpp).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sc_material_readback"),
            size: u64::from(row * size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &e.texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size.height) },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::new();
        for y in 0..size.height as usize {
            let start = y * row as usize;
            out.extend_from_slice(data.get(start..start + (size.width * bpp) as usize)?);
        }
        Some((size.width, size.height, out))
    }
}

/// The raster target's sampling functions (§5.3): for each binding (group 3, textures 4–7) and
/// wrap mode (samplers 8–10), `mat_image_<b>_<wrap>(t, default, channel)`, the planar sample at
/// the level the footprint selects, and `mat_slope_<b>_<wrap>(t, channel)`, the world gradient of
/// its `value` by one-texel forward differences (the reference's slope).
pub fn sampling_source() -> String {
    let mut s = String::from(
        "// Material textures (crates/gpu/src/material/textures.rs). A binding without a resident texture\n\
         // holds the 1 × 1 stand-in, which a resident texture never is.\n",
    );
    for b in 0..TEXTURE_SLOTS {
        s.push_str(&format!("@group(3) @binding({}) var mat_tex{b}: texture_2d<f32>;\n", 4 + b));
    }
    for (i, wrap) in WRAPS.iter().enumerate() {
        s.push_str(&format!("@group(3) @binding({}) var mat_{wrap}: sampler;\n", 8 + i));
    }
    s.push_str(
        "\n// The `value` of an image sample `c` for `channel` (0 the Rec. 709 luminance, 1–3 r, g, b, 4 alpha).\n\
         fn mat_pick(c: vec4<f32>, channel: u32) -> f32 {\n\
         \x20   return select(select(select(select(sc_luminance(c.rgb), c.r, channel == 1u), c.g, channel == 2u), c.b, channel == 3u), c.a, channel == 4u);\n\
         }\n\
         // The level a footprint of `fp` (uv units) selects in a texture of `size` texels.\n\
         fn mat_lod(fp: f32, size: vec2<f32>) -> f32 {\n\
         \x20   return log2(max(fp * max(size.x, size.y), 1.0));\n\
         }\n",
    );
    for b in 0..TEXTURE_SLOTS {
        for wrap in WRAPS {
            s.push_str(&format!(
                "\nfn mat_image_{b}_{wrap}(t: sc_Texcoord, default_color: vec3<f32>, channel: u32) -> sc_Image {{\n\
                 \x20   let size = vec2<f32>(textureDimensions(mat_tex{b}));\n\
                 \x20   let ready = size.x * size.y > 1.0;\n\
                 \x20   let c = textureSampleLevel(mat_tex{b}, mat_{wrap}, vec2<f32>(t.uv.x, 1.0 - t.uv.y), mat_lod(sc_tc_footprint(t), size));\n\
                 \x20   let d = sc_image_default(default_color, channel);\n\
                 \x20   return sc_Image(select(d.color, c.rgb, ready), select(d.value, mat_pick(c, channel), ready), select(d.alpha, c.a, ready));\n\
                 }}\n\
                 fn mat_slope_{b}_{wrap}(t: sc_Texcoord, channel: u32) -> vec3<f32> {{\n\
                 \x20   let size = vec2<f32>(textureDimensions(mat_tex{b}));\n\
                 \x20   let st = vec2<f32>(t.uv.x, 1.0 - t.uv.y);\n\
                 \x20   let lod = mat_lod(sc_tc_footprint(t), size);\n\
                 \x20   let h = 1.0 / size;\n\
                 \x20   let c = mat_pick(textureSampleLevel(mat_tex{b}, mat_{wrap}, st, lod), channel);\n\
                 \x20   let cu = mat_pick(textureSampleLevel(mat_tex{b}, mat_{wrap}, st + vec2<f32>(h.x, 0.0), lod), channel);\n\
                 \x20   let cv = mat_pick(textureSampleLevel(mat_tex{b}, mat_{wrap}, st - vec2<f32>(0.0, h.y), lod), channel);\n\
                 \x20   let g = (cu - c) * size.x * t.du + (cv - c) * size.y * t.dv;\n\
                 \x20   return select(vec3<f32>(0.0), g, size.x * size.y > 1.0);\n\
                 }}\n"
            ));
        }
    }
    s
}

/// The samplers by wrap mode, in the `image` node's `wrap` order (bindings 8–10).
pub const WRAPS: [&str; 3] = ["repeat", "clamp", "mirror"];

#[cfg(test)]
#[path = "textures_tests.rs"]
mod tests;
