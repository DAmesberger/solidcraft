//! The material editor's live swatch: a sphere and a tilted flat coupon, drawn on the GPU by
//! the viewport's own pipelines with the product look (its studio and tone mapping) in an egui
//! paint callback. No pixel is shaded on the CPU; the CPU only builds the two small meshes.
//!
//! The swatch has a renderer of its own beside the window's, so drawing it never evicts the
//! viewport's or the Render preview's uploads. It is created on the first swatch frame with the
//! window renderer's targets, and its studio is baked off the window thread like the viewport's.
//!
//! A graph is drawn as itself: its lowered IR, the look's uniform values and its base response
//! go into the scene's material-graph surfaces (`GpuScene::graphs`), whose generated pipeline
//! compiles beside the window; until it has, the base response is drawn. A look without a graph
//! is drawn as its `OpaqueSurface`. Image textures draw as their node's default until the GPU
//! textures land (track C3).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use solidcore_record::Vec3;
use solidcraft_engine::render::Camera;
use solidcraft_engine::render::surface::OpaqueSurface;

use solidcraft_engine::render::material_graph::{self as mg, Constant, Ir, MaterialGraph, SurfaceSample};

use crate::gpu::{Frame, GpuScene, GpuTarget, ProductLook, Renderer};

/// A graph ready to draw: its IR, the look's uniform words and its base response.
#[derive(Debug)]
pub(crate) struct Material {
    ir: Arc<Ir>,
    words: Vec<u32>,
    base: SurfaceSample,
    key: u64,
}

impl Material {
    /// `graph` with the look's `values`; `None` when it does not lower (the swatch then shows
    /// the look's surface).
    pub(crate) fn new(graph: &MaterialGraph, values: &std::collections::BTreeMap<String, Constant>) -> Option<Material> {
        let valid = mg::validate(graph).ok()?;
        let ir = mg::lower(&valid).ok()?;
        let base = mg::base_sample(&valid).ok()?;
        let words = mg::uniform_words(&ir, values).ok()?;
        let record = solidcraft_gpu::material::base_record(&base);
        let bytes =
            ir.shape.0.into_iter().chain(words.iter().flat_map(|w| w.to_le_bytes())).chain(record.iter().flat_map(|x| x.to_bits().to_le_bytes()));
        let key = fnv(bytes);
        Some(Material { ir: Arc::new(ir), words, base, key })
    }
}

/// What the swatch draws.
#[derive(Clone, Debug)]
pub(crate) enum Sample {
    Surface(OpaqueSurface),
    Graph(Arc<Material>),
}

fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h | 1
}

/// The targets the window's renderer draws with; [`crate::gpu::install`] publishes them so the
/// swatch renderer draws into the same kind of pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SwatchTargets {
    pub scene: GpuTarget,
    pub window: GpuTarget,
}

/// The swatch's own renderer and its studio bake.
struct SwatchRenderer {
    renderer: Renderer,
    baking: crate::gpu::StudioBaking,
}

/// Whether the swatch renderer's studio is installed (the window repaints until it is).
static STUDIO_READY: AtomicBool = AtomicBool::new(false);

pub(crate) fn studio_ready() -> bool {
    STUDIO_READY.load(Ordering::Relaxed)
}

/// Sphere segments around and rings from pole to pole: bounded work and memory.
const SECTORS: u32 = 48;
const RINGS: u32 = 24;
const SPHERE_CENTRE: [f64; 3] = [-1.15, 0.0, 1.0];
const COUPON_CENTRE: [f64; 3] = [1.2, 0.0, 0.85];
/// The coupon leans back from the camera by this angle from vertical, radians.
const COUPON_LEAN: f64 = 0.7;
const COUPON_HALF: f64 = 0.9;
const COUPON_THICKNESS: f64 = 0.08;

fn v(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

/// The swatch's geometry with `sample` on every vertex.
pub(crate) fn scene(sample: &Sample) -> GpuScene {
    let mut scene = GpuScene::default();
    match sample {
        Sample::Surface(surface) => geometry(|p, n| scene.pbr_tri(p.to_f32(), n.to_f32(), *surface)),
        Sample::Graph(m) => match scene.graphs.material(&m.ir, &m.words, &m.base) {
            Ok(material) => {
                let graphs = &mut scene.graphs;
                // The samples sit at the origin, so object space is world space.
                geometry(|p, n| graphs.vertex(material, p.to_f32(), n.to_f32(), p.to_f32()));
            }
            Err(e) => log::warn!("material swatch: {e}"),
        },
    }
    scene
}

/// The sphere's and the coupon's triangles, three vertices (position, unit normal) at a time.
fn geometry(mut vertex: impl FnMut(Vec3, Vec3)) {
    let centre = v(SPHERE_CENTRE);
    let point = |ring: u32, sector: u32| {
        let theta = std::f64::consts::PI * f64::from(ring) / f64::from(RINGS);
        let phi = std::f64::consts::TAU * f64::from(sector) / f64::from(SECTORS);
        Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos())
    };
    for ring in 0..RINGS {
        for sector in 0..SECTORS {
            let [a, b, c, d] = [point(ring, sector), point(ring + 1, sector), point(ring + 1, sector + 1), point(ring, sector + 1)];
            for tri in [[a, b, c], [a, c, d]] {
                for n in tri {
                    vertex(centre + n, n);
                }
            }
        }
    }
    // The coupon: a thin slab whose face looks toward the camera and up.
    let (s, c) = COUPON_LEAN.sin_cos();
    let normal = Vec3::new(0.0, -s, c);
    let u = Vec3::X;
    let w = Vec3::new(0.0, c, s);
    let centre = v(COUPON_CENTRE);
    let corner = |a: f64, b: f64, depth: f64| centre + u * (a * COUPON_HALF) + w * (b * COUPON_HALF) - normal * depth;
    let mut quad = |p: [Vec3; 4], n: Vec3| {
        for tri in [[p[0], p[1], p[2]], [p[0], p[2], p[3]]] {
            for q in tri {
                vertex(q, n);
            }
        }
    };
    let t = COUPON_THICKNESS;
    quad([corner(-1.0, -1.0, 0.0), corner(1.0, -1.0, 0.0), corner(1.0, 1.0, 0.0), corner(-1.0, 1.0, 0.0)], normal);
    quad([corner(-1.0, -1.0, t), corner(1.0, -1.0, t), corner(1.0, -1.0, 0.0), corner(-1.0, -1.0, 0.0)], -w);
    quad([corner(1.0, -1.0, t), corner(1.0, 1.0, t), corner(1.0, 1.0, 0.0), corner(1.0, -1.0, 0.0)], u);
    quad([corner(-1.0, 1.0, t), corner(-1.0, -1.0, t), corner(-1.0, -1.0, 0.0), corner(-1.0, 1.0, 0.0)], -u);
    quad([corner(1.0, 1.0, t), corner(-1.0, 1.0, t), corner(-1.0, 1.0, 0.0), corner(1.0, 1.0, 0.0)], w);
}

/// The swatch camera: from the front, a little above, framing both samples.
pub(crate) fn camera() -> Camera {
    Camera { target: Vec3::new(0.0, 0.0, 0.85), yaw: 0.0, pitch: 0.32, roll: 0.0, distance: 9.0, fov: 0.42, up: Default::default() }
}

/// Scene radius for the clipping planes.
pub(crate) const RADIUS: f64 = 3.0;

/// A key for the renderer's upload cache: equal samples share an upload.
pub(crate) fn key(sample: &Sample) -> u64 {
    match sample {
        Sample::Graph(m) => m.key,
        Sample::Surface(surface) => {
            let coat = surface.clear_coat().map_or([0.0; 3], |c| [c.weight(), c.roughness(), c.ior()]);
            let parts = surface
                .base_color()
                .to_f32()
                .into_iter()
                .chain([surface.metallic() as f32, surface.roughness() as f32, surface.dielectric_ior() as f32])
                .chain(coat.map(|x| x as f32));
            // Distinct from every graph key's low bit pattern only by chance; both are FNV.
            fnv(parts.flat_map(|x| x.to_bits().to_le_bytes()).chain([0xff]))
        }
    }
}

/// What the swatch shows: the slot it uploads from and its key.
#[derive(Clone, Default)]
struct Slot {
    key: u64,
    slot: Arc<Mutex<Option<GpuScene>>>,
}

/// One frame of the swatch in `rect` of size `size_px` (physical pixels).
pub(crate) fn frame(slot: Arc<Mutex<Option<GpuScene>>>, key: u64, size_px: [f32; 2]) -> Frame {
    let camera = camera();
    let aspect = if size_px[1] > 0.0 { f64::from(size_px[0] / size_px[1]) } else { 1.0 };
    let eye = camera.eye();
    let none = || Arc::new(Mutex::new(None));
    Frame {
        hatch: Default::default(),
        product_lights: None,
        product_look: Some(ProductLook::default()),
        // The swatch's sphere is modelled in millimetres.
        units_per_mm: 1.0,
        product: true,
        upload_error: None,
        key,
        slot,
        sketch_key: 0,
        sketch_slot: none(),
        hl_key: 0,
        hl_slot: none(),
        pv_key: 0,
        pv_slot: none(),
        pv_hide: false,
        view_proj: camera.view_proj(aspect, RADIUS).to_f32(),
        back: camera.back().to_f32(),
        size_px,
        clip: [0.0; 4],
        cap: [0.0; 4],
        images: Vec::new(),
        clouds: Vec::new(),
        analysis: [0.0; 8],
        analysis_colors: [0.0; 36],
        access: None,
        outline: false,
        eye: [eye.x as f32, eye.y as f32, eye.z as f32, 1.0],
        ao_radius: 0.0,
        ao_samples: solidcraft_gpu::view::AO_SAMPLES,
        ssaa: 2.0,
        hover_tint: [0.0; 3],
        selection_tint: [0.0; 3],
        sky: [0.0; 4],
    }
}

/// Paint the swatch of `surface` into `rect`. The scene is rebuilt only when the surface changes.
pub(crate) fn paint(ui: &egui::Ui, id: egui::Id, rect: egui::Rect, sample: Sample) {
    let key = key(&sample);
    let slot = ui.ctx().data_mut(|d| {
        let state = d.get_temp_mut_or_default::<Slot>(id);
        if state.key != key {
            if let Ok(mut s) = state.slot.lock() {
                *s = Some(scene(&sample));
            }
            state.key = key;
        }
        state.slot.clone()
    });
    let ppp = ui.ctx().pixels_per_point();
    let frame = frame(slot, key, [rect.width() * ppp, rect.height() * ppp]);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, SwatchCallback(frame)));
    // Repaint until the studio is baked and the material's pipeline has compiled.
    if !studio_ready() || MATERIAL_PENDING.load(Ordering::Relaxed) {
        ui.ctx().request_repaint();
    }
}

/// Whether the swatch's material pipeline was still compiling in the last frame.
static MATERIAL_PENDING: AtomicBool = AtomicBool::new(false);

struct SwatchCallback(Frame);

impl egui_wgpu::CallbackTrait for SwatchCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<SwatchRenderer>().is_none() {
            let Some(targets) = resources.get::<SwatchTargets>().copied() else { return Vec::new() };
            let scene = GpuTarget { tess: false, ..targets.scene };
            let window = GpuTarget { tess: false, ..targets.window };
            resources.insert(SwatchRenderer { renderer: Renderer::with_window_target(device, scene, window, None), baking: Default::default() });
        }
        let Some(swatch) = resources.get_mut::<SwatchRenderer>() else { return Vec::new() };
        if let Some(look) = self.0.product_look {
            let installed = swatch.renderer.has_studio_environment(look.studio);
            if !installed && let Some(studio) = swatch.baking.advance(look.studio) {
                swatch.renderer.set_studio_environment(device, queue, &studio);
            }
            STUDIO_READY.store(swatch.renderer.has_studio_environment(look.studio), Ordering::Relaxed);
        }
        swatch.renderer.prepare(device, queue, encoder, &self.0);
        MATERIAL_PENDING.store(swatch.renderer.material_stats().pending > 0, Ordering::Relaxed);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(swatch) = resources.get::<SwatchRenderer>() else { return };
        let vp = info.viewport_in_pixels();
        swatch.renderer.paint(&self.0, pass, [vp.left_px as f32, vp.top_px as f32, vp.width_px as f32, vp.height_px as f32]);
    }
}
