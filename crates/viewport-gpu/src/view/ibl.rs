//! The studio lighting for Realistic views. The selected rig supplies world-space radiance to
//! the shared environment baker. Its area lights are reflected in the specular levels and light
//! diffuse surfaces directly. The ambient map contains the cyclorama alone.

use std::sync::Arc;

use solidcraft_viewport::studio::{self as rig, Room, StudioLight, StudioRig};

use wgpu::util::DeviceExt;

use crate::environment::{self, BaseLevel, Environment, Equirect, MapBake};

/// Two room parameters followed by four vectors per light.
const ENV_UNIFORM_SIZE: u64 = (2 + 4 * rig::MAX_LIGHTS as u64) * 16;

/// A studio the product look lights with: a handle to a studio's rig (`solidcraft_viewport::studio`,
/// an entry of the app's studio library, data the user can manage), registered with
/// [`StudioPreset::register`]. The handle is the rig's content key, so the same rig registered
/// twice is the same studio, and an edited rig is a new one. The default handle names no rig: its
/// bake is a neutral grey surround without lights, until an app registers the studio it uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StudioPreset(u64);

/// Most rigs kept registered; past it the oldest is forgotten (a studio baked from it stays
/// installed).
const MAX_REGISTERED: usize = 64;

fn registry() -> &'static std::sync::Mutex<Vec<(u64, Arc<StudioRig>)>> {
    static REGISTRY: std::sync::OnceLock<std::sync::Mutex<Vec<(u64, Arc<StudioRig>)>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

impl StudioPreset {
    /// The handle that names no rig (the default).
    pub const NONE: StudioPreset = StudioPreset(0);

    /// The handle of `rig`, kept so a bake on any thread can read it.
    pub fn register(rig: &StudioRig) -> Self {
        let key = rig.content_key().max(1);
        if let Ok(mut rigs) = registry().lock()
            && !rigs.iter().any(|(k, _)| *k == key)
        {
            if rigs.len() >= MAX_REGISTERED {
                rigs.remove(0);
            }
            rigs.push((key, Arc::new(rig.clone())));
        }
        StudioPreset(key)
    }

    /// The rig this handle names, while it is registered.
    pub fn rig(self) -> Option<Arc<StudioRig>> {
        registry().lock().ok().and_then(|rigs| rigs.iter().find(|(k, _)| *k == self.0).map(|(_, rig)| rig.clone()))
    }

    /// The rig's content key (0: no rig).
    pub fn key(self) -> u64 {
        self.0
    }
}

/// The built-in studios' rigs for tests: a copy of the app's `assets/render/catalog/studios.json`
/// (the app's GPU crate checks the copy stays identical).
#[cfg(test)]
pub(crate) fn test_rig(name: &str) -> StudioRig {
    let text = include_str!("../../tests/fixtures/studios.json");
    let file: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let studios: Vec<solidcraft_viewport::studio::Studio> = serde_json::from_value(file["items"].clone()).unwrap_or_default();
    studios.into_iter().find(|s| s.name == name).and_then(|s| s.realistic).unwrap_or_else(|| panic!("no built-in rig {name}"))
}

#[cfg(test)]
impl ProductLook {
    /// The look lit by the built-in Product studio.
    pub(crate) fn product() -> Self {
        ProductLook { studio: StudioPreset::product(), ..ProductLook::default() }
    }
}

#[cfg(test)]
impl StudioPreset {
    pub(crate) fn product() -> Self {
        Self::register(&test_rig("Product"))
    }
    pub(crate) fn soft() -> Self {
        Self::register(&test_rig("Soft"))
    }
    pub(crate) fn contrast() -> Self {
        Self::register(&test_rig("Contrast"))
    }
    pub(crate) fn all() -> [Self; 3] {
        [Self::product(), Self::soft(), Self::contrast()]
    }
}

/// Where the studio stands: the scene's ground and extent, so the rig's lights sit in multiples of
/// the subject's extent B about it and reflections are projected against a room sized to it
/// ([`rig`]). Fit it once from the scene's bounds ([`StudioRoom::around`]), not per view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StudioRoom {
    /// The subject's centre across (x, y), and the ground's height (z), in world units.
    pub origin: [f32; 3],
    /// The subject's extent B (its bounding box's largest side), in world units.
    pub extent: f32,
    /// The radii along x and y of the contact darkening the floor shows under the subject in
    /// reflections, in world units: the viewport's ground-shadow ellipse ([`StudioRoom::around`]).
    pub footprint: [f32; 2],
}

impl StudioRoom {
    /// The room around a subject's bounding box (`None` for an empty, non-finite or flat-to-a-point
    /// box). The ground is the box's bottom; the footprint is the ellipse the viewport's ground
    /// shadow draws (0.62 of each side plus 0.15 of the height).
    pub fn around(min: [f32; 3], max: [f32; 3]) -> Option<Self> {
        if !min.iter().chain(&max).all(|v| v.is_finite()) {
            return None;
        }
        let size: [f32; 3] = std::array::from_fn(|k| max[k] - min[k]);
        let extent = size[0].max(size[1]).max(size[2]);
        if !(extent > 0.0) || size.iter().any(|s| *s < 0.0) || !extent.is_finite() {
            return None;
        }
        StudioRoom {
            origin: [0.5 * (min[0] + max[0]), 0.5 * (min[1] + max[1]), min[2]],
            extent,
            footprint: [0.62 * size[0] + 0.15 * size[2], 0.62 * size[1] + 0.15 * size[2]],
        }
        .sanitized()
    }

    /// Largest coordinate, extent and footprint a room keeps (world units); the smallest extent.
    pub const MAX_SIZE: f32 = 1e12;
    pub const MIN_EXTENT: f32 = 1e-6;

    /// The room as drawn: `None` unless every value is finite and within [`StudioRoom::MAX_SIZE`],
    /// the extent at least [`StudioRoom::MIN_EXTENT`] and the footprint not negative.
    pub fn sanitized(self) -> Option<Self> {
        let bounded = self.origin.iter().chain(&self.footprint).all(|v| v.is_finite() && v.abs() <= Self::MAX_SIZE);
        let extent = (Self::MIN_EXTENT..=Self::MAX_SIZE).contains(&self.extent);
        (bounded && extent && self.footprint.iter().all(|f| *f >= 0.0)).then_some(self)
    }
}

/// The opt-in product look of a frame (`Frame::product_look`). The frame's product lights, if any,
/// still light the scene, and the look's studio lights it from all around (once installed:
/// `Renderer::set_studio_environment`); the fixed three-light studio of the default preview does
/// not. Without a look a frame keeps the default preview, unchanged.
///
/// With the look, PBR surfaces also show ambient occlusion (from a frame's `ao_radius`) on the
/// studio's and the ambient lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProductLook {
    /// Scale of the scene-referred radiance (product lights and studio) before tone mapping.
    pub exposure: f32,
    /// The studio reflected and lighting from all around.
    pub studio: StudioPreset,
    /// Turn of the studio about world +Z, radians (counter-clockwise seen from above).
    pub rotation: f32,
    /// Scale of the studio's radiance (0: only the product lights).
    pub intensity: f32,
    /// Where the studio stands. `None`: the studio at infinity, its lights as seen from the
    /// subject's centre, and reflections without box projection.
    pub room: Option<StudioRoom>,
}

impl Default for ProductLook {
    fn default() -> Self {
        ProductLook { exposure: 1.0, studio: StudioPreset::default(), rotation: 0.0, intensity: 1.0, room: None }
    }
}

impl ProductLook {
    /// Largest exposure and intensity a record keeps.
    pub const MAX_SCALE: f32 = 1024.0;

    /// The uniform the shaders read: (exposure, studio intensity, studio rotation, 1), of
    /// [`ProductLook::sanitized`].
    pub(crate) fn uniform(&self) -> [f32; 4] {
        let look = self.sanitized();
        [look.exposure, look.intensity, look.rotation, 1.0]
    }

    /// The room's uniform: (origin x, y, ground z, extent B), then (footprint radii x, y, 0, 0);
    /// all zero without a room.
    pub(crate) fn room_uniform(&self) -> [f32; 8] {
        match self.room.and_then(StudioRoom::sanitized) {
            Some(r) => [r.origin[0], r.origin[1], r.origin[2], r.extent, r.footprint[0], r.footprint[1], 0.0, 0.0],
            None => [0.0; 8],
        }
    }

    /// The look the renderer draws: out-of-range values clamped, non-finite ones replaced by the
    /// defaults, the rotation within [0, 2π).
    pub fn sanitized(self) -> Self {
        let scale = |x: f32| if x.is_finite() { x.clamp(0.0, Self::MAX_SCALE) } else { 1.0 };
        let rotation = if self.rotation.is_finite() { self.rotation.rem_euclid(std::f32::consts::TAU) } else { 0.0 };
        ProductLook {
            exposure: scale(self.exposure),
            studio: self.studio,
            rotation,
            intensity: scale(self.intensity),
            room: self.room.and_then(StudioRoom::sanitized),
        }
    }
}

type Rgb = [f64; 3];

#[cfg(test)]
fn dot(a: Rgb, b: Rgb) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
fn unit(a: Rgb) -> Rgb {
    let l = dot(a, a).sqrt();
    if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// `acc += c × w`.
#[cfg(test)]
fn add(acc: &mut Rgb, c: Rgb, w: f64) {
    for (a, c) in acc.iter_mut().zip(c) {
        *a += c * w;
    }
}

#[cfg(test)]
fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn studio(preset: StudioPreset) -> Option<Arc<StudioRig>> {
    preset.rig()
}

/// A calibrated rig and its environment maps, ready to upload.
#[derive(Clone)]
pub struct StudioEnvironment {
    preset: StudioPreset,
    levels: Vec<Equirect>,
    diffuse: Equirect,
    base: BaseLevel,
    lights: Vec<StudioLight>,
    room: Room,
    calibration: f64,
}

/// Where a bake's radiance comes from.
#[derive(Clone)]
enum Radiance {
    /// A rig at its calibrated scale, with its key white ([`StudioRig::key_white`]).
    Studio(Arc<StudioRig>, f64, f64),
    /// The same radiance everywhere: the white furnace of the tests, and the neutral surround of
    /// a handle that names no registered rig.
    Constant(f64),
}

/// The neutral surround's radiance (a mid grey) for a handle without a rig.
const SURROUND: f64 = 0.18;

impl Radiance {
    /// Everything seen from the studio's centre along `d`: what reflections show.
    fn at(&self, d: Rgb) -> Rgb {
        match self {
            Radiance::Studio(r, scale, white) => r.radiance_in(d, *white).map(|c| c * scale),
            Radiance::Constant(c) => [*c; 3],
        }
    }

    /// The radiance without the rig's lights (which diffuse shading takes directly).
    fn ambient(&self, d: Rgb) -> Rgb {
        match self {
            Radiance::Studio(r, scale, white) => r.ambient_in(d, *white).map(|c| c * scale),
            Radiance::Constant(c) => [*c; 3],
        }
    }

    fn calibration(&self) -> f64 {
        match self {
            Radiance::Studio(_, scale, _) => *scale,
            Radiance::Constant(_) => 1.0,
        }
    }

    fn lights(&self) -> Vec<StudioLight> {
        match self {
            Radiance::Studio(r, _, _) => r.lights.iter().take(rig::MAX_LIGHTS).cloned().collect(),
            Radiance::Constant(_) => Vec::new(),
        }
    }

    fn room(&self) -> Room {
        match self {
            Radiance::Studio(r, _, _) => r.room,
            Radiance::Constant(_) => Room::default(),
        }
    }
}

/// Bake a studio off-thread, or advance by a bounded number of samples per frame on the web.
pub struct StudioBake {
    preset: StudioPreset,
    radiance: Radiance,
    base: BaseLevel,
    maps: MapBake,
}

impl StudioBake {
    /// Approximate number of samples in one web frame's share of the bake.
    pub const FRAME_WORK: usize = 16_384;

    pub fn new(preset: StudioPreset) -> Self {
        Self::with_base(preset, BaseLevel::Preblurred)
    }

    pub fn with_base(preset: StudioPreset, base: BaseLevel) -> Self {
        let radiance = match studio(preset) {
            Some(rig) => {
                let calibration = rig.calibration();
                let white = rig.key_white();
                Radiance::Studio(rig, calibration, white)
            }
            None => Radiance::Constant(SURROUND),
        };
        Self { preset, radiance, base, maps: MapBake::new(base) }
    }

    /// The studio being baked.
    pub fn preset(&self) -> StudioPreset {
        self.preset
    }

    pub fn is_done(&self) -> bool {
        self.maps.is_done()
    }

    /// Bake whole rows until approximately `work` samples have been evaluated.
    pub fn step(&mut self, work: usize) -> bool {
        self.maps.step(work, |d| self.radiance.specular(d), |d| self.radiance.diffuse(d))
    }

    /// Take the result once all rows have been baked.
    pub fn finish(self) -> Option<StudioEnvironment> {
        let (levels, diffuse) = self.maps.finish()?;
        Some(StudioEnvironment::from_maps(self.preset, &self.radiance, self.base, levels, diffuse))
    }

    fn bake(self) -> StudioEnvironment {
        let (levels, diffuse) = self.maps.bake(|d| self.radiance.specular(d), |d| self.radiance.diffuse(d));
        StudioEnvironment::from_maps(self.preset, &self.radiance, self.base, levels, diffuse)
    }
}

impl Radiance {
    fn specular(&self, d: [f32; 3]) -> [f32; 3] {
        self.at(d.map(f64::from)).map(|v| v as f32)
    }

    fn diffuse(&self, d: [f32; 3]) -> [f32; 3] {
        self.ambient(d.map(f64::from)).map(|v| v as f32)
    }
}

impl StudioEnvironment {
    /// Bake a studio in one pass on a worker thread.
    pub fn bake(preset: StudioPreset) -> Self {
        StudioBake::new(preset).bake()
    }

    /// Bake a studio using the chosen reflection roughness at level zero.
    pub fn bake_with_base(preset: StudioPreset, base: BaseLevel) -> Self {
        StudioBake::with_base(preset, base).bake()
    }

    fn from_maps(preset: StudioPreset, radiance: &Radiance, base: BaseLevel, levels: Vec<Equirect>, diffuse: Equirect) -> Self {
        Self { preset, levels, diffuse, base, lights: radiance.lights(), room: radiance.room(), calibration: radiance.calibration() }
    }

    pub fn preset(&self) -> StudioPreset {
        self.preset
    }

    /// The scale the studio's radiance is baked at: its exposure calibration
    /// ([`StudioRig::calibration`]; 1 for a surround without a rig).
    pub fn calibration(&self) -> f64 {
        self.calibration
    }

    /// The level range, light count, base roughness, room bounds and calibrated light rectangles.
    fn uniform_bytes(&self) -> Vec<u8> {
        let lights = &self.lights[..self.lights.len().min(rig::MAX_LIGHTS)];
        let mut v = vec![self.levels.len().saturating_sub(1) as f32, lights.len() as f32, self.base.roughness(), 0.0];
        v.extend([self.room.half, self.room.floor, self.room.ceiling, 0.0].map(|x| x as f32));
        for slot in 0..rig::MAX_LIGHTS {
            match lights.get(slot) {
                Some(l) => {
                    let c = l.radiance.map(|x| x * self.calibration);
                    v.extend(
                        [
                            l.centre[0],
                            l.centre[1],
                            l.centre[2],
                            l.half_width,
                            l.width_axis[0],
                            l.width_axis[1],
                            l.width_axis[2],
                            l.half_height,
                            l.height_axis[0],
                            l.height_axis[1],
                            l.height_axis[2],
                            0.0,
                            c[0],
                            c[1],
                            c[2],
                            0.0,
                        ]
                        .map(|x| x as f32),
                    );
                }
                None => v.extend([0.0; 16]),
            }
        }
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
}

fn texture_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}

fn studio_layout(visibility: wgpu::ShaderStages) -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        texture_entry(0, visibility),
        sampler_entry(1, visibility),
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        },
        texture_entry(11, visibility),
        texture_entry(12, visibility),
    ]
}

pub(crate) fn layout(device: &wgpu::Device, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("sc_studio"), entries: &studio_layout(visibility) })
}

/// The images and rig uniform of one studio. Every studio uses the same BRDF table and sampler.
struct StudioResources {
    specular: wgpu::TextureView,
    diffuse: wgpu::TextureView,
    uniform: wgpu::Buffer,
}

impl StudioResources {
    fn entries<'a>(&'a self, sampler: &'a wgpu::Sampler, dfg: &'a wgpu::TextureView) -> [wgpu::BindGroupEntry<'a>; 5] {
        [
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.specular) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(&self.diffuse) },
            wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::TextureView(dfg) },
        ]
    }

    fn bind(self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, dfg: &wgpu::TextureView) -> EnvGpu {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("sc_studio"), layout, entries: &self.entries(sampler, dfg) });
        EnvGpu { resources: self, bind }
    }
}

struct EnvGpu {
    bind: wgpu::BindGroup,
    resources: StudioResources,
}

/// Empty textures for a view awaiting its chosen studio.
fn placeholder(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, environment: &Environment) -> EnvGpu {
    let blank = environment::map_texture(device, "sc_empty_studio", 1, 1, 1).create_view(&Default::default());
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sc_studio"),
        contents: &[0; ENV_UNIFORM_SIZE as usize],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    StudioResources { specular: blank.clone(), diffuse: blank, uniform }.bind(device, layout, &environment.sampler, &environment.dfg)
}

/// Upload the selected rig's two maps. The GGX lookup table is independent of the studio.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    dfg: &wgpu::TextureView,
    env: &StudioEnvironment,
) -> EnvGpu {
    let mip_count = env.levels.len() as u32;
    let width = env.levels.first().map_or(2, |l| l.width);
    let specular = environment::map_texture(device, "sc_realistic_specular", width, width / 2, mip_count);
    for (mip, level) in env.levels.iter().enumerate() {
        environment::write_map(queue, &specular, mip as u32, level);
    }
    let width = env.diffuse.width;
    let diffuse = environment::map_texture(device, "sc_realistic_diffuse", width, width / 2, 1);
    environment::write_map(queue, &diffuse, 0, &env.diffuse);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sc_studio"),
        contents: &env.uniform_bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    StudioResources { specular: specular.create_view(&Default::default()), diffuse: diffuse.create_view(&Default::default()), uniform }
        .bind(device, layout, sampler, dfg)
}

/// Appearance graphs share the studio group and add a page, four textures and three wrap modes.
pub(crate) fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let visibility = wgpu::ShaderStages::FRAGMENT;
    let mut entries = studio_layout(visibility).to_vec();
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 3,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(crate::material::PAGE_BYTES),
        },
        count: None,
    });
    entries.extend((4..8).map(|binding| texture_entry(binding, visibility)));
    entries.extend((8..11).map(|binding| sampler_entry(binding, visibility)));
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("sc_studio_material"), entries: &entries })
}

/// Most studios the renderer keeps on the GPU at once.
const MAX_INSTALLED: usize = 8;

/// The renderer's studios: the stand-in and the studios uploaded so far (one per rig, at most
/// [`MAX_INSTALLED`]).
pub(crate) struct Studios {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    dfg: wgpu::TextureView,
    placeholder: EnvGpu,
    baked: Vec<(StudioPreset, EnvGpu)>,
    /// Counts installations, so binds made from an older studio are known stale.
    generation: u64,
}

impl Studios {
    pub(crate) fn new(device: &wgpu::Device, environment: &Environment) -> Self {
        let layout = layout(device, wgpu::ShaderStages::FRAGMENT);
        let placeholder = placeholder(device, &layout, environment);
        Studios { layout, sampler: environment.sampler.clone(), dfg: environment.dfg.clone(), placeholder, baked: Vec::new(), generation: 0 }
    }

    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Whether `preset` is on the GPU.
    pub(crate) fn has(&self, preset: StudioPreset) -> bool {
        self.baked.iter().any(|(p, _)| *p == preset)
    }

    /// Upload `env`, replacing the same preset's studio.
    pub(crate) fn install(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, env: &StudioEnvironment) {
        let gpu = upload(device, queue, &self.layout, &self.sampler, &self.dfg, env);
        self.baked.retain(|(p, _)| *p != env.preset());
        // At most a few studios stay on the GPU; the oldest goes first.
        if self.baked.len() >= MAX_INSTALLED {
            self.baked.remove(0);
        }
        self.baked.push((env.preset(), gpu));
        self.generation += 1;
    }

    /// How many studios have been installed: binds of an older generation are stale.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// An appearance-graph group 3 ([`material_layout`]): the frame's studio (as [`Studios::bind`]
    /// picks it), a uniform `page`, the material `textures` and their `samplers`.
    pub(crate) fn material_bind(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        preset: Option<StudioPreset>,
        page: &wgpu::Buffer,
        textures: [&wgpu::TextureView; 4],
        samplers: [&wgpu::Sampler; 3],
    ) -> wgpu::BindGroup {
        let env = preset.and_then(|preset| self.baked.iter().find(|(p, _)| *p == preset)).map_or(&self.placeholder, |(_, gpu)| gpu);
        let [t0, t1, t2, t3] = textures;
        let [s0, s1, s2] = samplers;
        let [specular, studio_sampler, rig, diffuse, dfg] = env.resources.entries(&self.sampler, &self.dfg);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sc_studio_material"),
            layout,
            entries: &[
                specular,
                studio_sampler,
                rig,
                diffuse,
                dfg,
                wgpu::BindGroupEntry { binding: 3, resource: page.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(t0) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(t1) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(t2) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(t3) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(s0) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(s1) },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::Sampler(s2) },
            ],
        })
    }

    /// The bind group for a frame's look (the stand-in without one, or before its studio is up).
    pub(crate) fn bind(&self, preset: Option<StudioPreset>) -> &wgpu::BindGroup {
        preset.and_then(|preset| self.baked.iter().find(|(p, _)| *p == preset)).map_or(&self.placeholder.bind, |(_, gpu)| &gpu.bind)
    }
}

/// Khronos PBR Neutral tone mapping on the CPU for the lighting tests.
#[cfg(test)]
pub(crate) fn pbr_neutral(c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    let c = c.map(|v| v * new_peak / peak);
    let g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    c.map(|v| v + (new_peak - v) * g)
}

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;
