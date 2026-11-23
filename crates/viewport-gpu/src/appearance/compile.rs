//! Making material pipelines off the frame (docs/appearance-graph.md §5.2): a cold pipeline compile
//! of a new shape takes a third of a second on some drivers, so where the device makes pipelines
//! from any thread (Vulkan, Metal, Direct3D 12 with FXC: `concurrent::threads_allowed`) one
//! worker thread generates, validates and builds them, and the frame picks up what is finished.
//! Until then a material draws its base response, and before even that pipeline exists it is not
//! drawn over the face the scene draws. Elsewhere (OpenGL and WebGL2, which have one context, and
//! DXC) pipelines are made on the frame's thread within the per-frame budget, as before.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use solidcraft_viewport::appearance_graph::ir::{Ir, ShapeKey};

use super::cache::{Variant, placed_record_layout, vertex_entry, vertex_layout};

/// What a pipeline needs besides its source; cloned onto the worker.
#[derive(Clone)]
pub(crate) struct PipelineSpec {
    pub(crate) device: wgpu::Device,
    pub(crate) layout: wgpu::PipelineLayout,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) samples: u32,
    pub(crate) depth: Option<wgpu::DepthStencilState>,
}

impl PipelineSpec {
    /// The pipeline of `source` for vertices with (`object`) or without the object position.
    pub(crate) fn build(&self, label: &str, source: String, object: bool, variant: Variant) -> wgpu::RenderPipeline {
        let device = &self.device;
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let plain = [Some(vertex_layout(object))];
        let placed = [Some(crate::view::shared_layout()), Some(crate::view::instance_layout()), Some(placed_record_layout())];
        let (entry, buffers): (&str, &[Option<wgpu::VertexBufferLayout<'_>>]) =
            if variant == Variant::Placed { ("vs_mat_placed", &placed) } else { (vertex_entry(object), &plain) };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some(entry), buffers, compilation_options: Default::default() },
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

    /// Generate, validate and build a shape's pipeline; `None` (logged) when its code fails.
    pub(crate) fn shape(&self, ir: &Ir, variant: Variant) -> Option<wgpu::RenderPipeline> {
        let source = match super::shape_source(ir).and_then(|s| super::validate(&s).map(|_| s)) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("appearance graph shape {:02x?}: drawn as its base response: {e}", ir.shape.0.get(..4).unwrap_or(&[]));
                return None;
            }
        };
        Some(self.build("sc_appearance_shape", source, ir.object_space, variant))
    }

    /// The constant-graph (base response) pipeline.
    pub(crate) fn fallback(&self, object: bool, variant: Variant) -> wgpu::RenderPipeline {
        self.build("sc_appearance_base", super::fallback_source(), object, variant)
    }
}

/// A pipeline to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Job {
    Shape(ShapeKey, Variant),
    Fallback(bool, Variant),
}

/// A finished job: its pipeline (`None`: the shape's code failed, or the device refused it) and
/// how long it took (ms).
pub(crate) struct Done {
    pub(crate) job: Job,
    pub(crate) pipeline: Option<wgpu::RenderPipeline>,
    pub(crate) ms: f64,
}

/// The worker thread making pipelines.
pub(crate) struct Compiler {
    jobs: Option<mpsc::Sender<(Job, Option<Arc<Ir>>)>>,
    done: std::sync::Mutex<mpsc::Receiver<Done>>,
    /// Jobs received while waiting ([`Self::wait`]), handed out by the next [`Self::finished`].
    early: std::sync::Mutex<Vec<Done>>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Set when dropped: queued jobs are skipped.
    cancel: Arc<AtomicBool>,
}

impl Compiler {
    /// A worker for `spec`'s device, where it makes pipelines from any thread and a thread starts.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn start(spec: &PipelineSpec) -> Option<Compiler> {
        if !crate::concurrent::threads_allowed(&spec.device) {
            return None;
        }
        let (jobs, inbox) = mpsc::channel::<(Job, Option<Arc<Ir>>)>();
        let (results, done) = mpsc::channel::<Done>();
        let spec = spec.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = std::thread::Builder::new()
            .name("material pipelines".into())
            .spawn(move || {
                for (job, ir) in inbox {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let started = web_time::Instant::now();
                    // Under scopes of its own (scopes belong to the thread that pushed them): a
                    // pipeline the device refuses is a failed shape, not an uncaptured error.
                    let scopes = crate::error_scopes(&spec.device);
                    let pipeline = match (job, ir) {
                        (Job::Shape(_, variant), Some(ir)) => spec.shape(&ir, variant),
                        (Job::Fallback(object, variant), _) => Some(spec.fallback(object, variant)),
                        (Job::Shape(..), None) => None,
                    };
                    let refused = crate::caught(scopes);
                    if let Some(e) = &refused {
                        log::warn!("a material pipeline was refused: {e}");
                    }
                    let pipeline = pipeline.filter(|_| refused.is_none());
                    if results.send(Done { job, pipeline, ms: started.elapsed().as_secs_f64() * 1e3 }).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Compiler { jobs: Some(jobs), done: std::sync::Mutex::new(done), early: std::sync::Mutex::new(Vec::new()), worker: Some(worker), cancel })
    }

    /// The browser has no threads: pipelines are made on the frame's thread.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn start(_spec: &PipelineSpec) -> Option<Compiler> {
        None
    }

    /// Queue a job (`ir` for a shape). False when the worker has stopped.
    pub(crate) fn send(&self, job: Job, ir: Option<Arc<Ir>>) -> bool {
        self.jobs.as_ref().is_some_and(|j| j.send((job, ir)).is_ok())
    }

    /// The jobs finished since the last call.
    pub(crate) fn finished(&self) -> Vec<Done> {
        let mut all = self.early.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default();
        if let Ok(d) = self.done.lock() {
            all.extend(d.try_iter());
        }
        all
    }

    /// Wait until `jobs` jobs have finished since the last [`Self::finished`], or until
    /// `timeout` has passed.
    pub(crate) fn wait(&self, jobs: usize, timeout: std::time::Duration) {
        let deadline = web_time::Instant::now() + timeout;
        let (Ok(done), Ok(mut early)) = (self.done.lock(), self.early.lock()) else { return };
        while early.len() < jobs {
            let left = deadline.saturating_duration_since(web_time::Instant::now());
            match done.recv_timeout(left) {
                Ok(d) => early.push(d),
                Err(_) => return,
            }
        }
    }
}

impl Drop for Compiler {
    /// The worker ends after its current job: dropping the queue stops it, and joining keeps the
    /// app from exiting in the middle of a driver's compile.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
