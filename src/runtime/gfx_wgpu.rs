//! F24 slice 1 — the native window renderer (feature `gfx-wgpu`): a winit window and a wgpu surface (Vulkan or GL, whichever the machine gives;
//! the adapter is reported as a gfx_poll `info` event). The renderer ONLY DRAWS the commands of a frame that runtime::gfx has accepted: every
//! number in a frame was computed by the program. Rules (stale, empty, unknown op) are runtime::gfx's; this file never sees a refused frame.
//! The window lives on its own thread (winit with_any_thread); gfx_submit hands the newest frame over and wakes it (an older frame not yet
//! drawn is replaced: a renderer that falls behind shows the latest frame).

use super::gfx::{push_event, Cmd, Frame, Sink};
use std::sync::{mpsc, Arc, Mutex};
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::keyboard::Key;
use winit::window::WindowBuilder;

#[derive(Clone, Copy, Debug)]
enum Wake {
    Frame,
    Close,
}

struct Win {
    slot: Arc<Mutex<Option<Frame>>>,
    proxy: EventLoopProxy<Wake>,
    size: Arc<Mutex<(u32, u32)>>,
}

impl Sink for Win {
    fn accept(&mut self, frame: &Frame, _raw: &[u8]) -> Option<String> {
        if frame.cmds.is_empty() { return None; }
        *self.slot.lock().unwrap() = Some(frame.clone());
        let _ = self.proxy.send_event(Wake::Frame);
        None
    }
    fn size(&self) -> (u32, u32) { *self.size.lock().unwrap() }
    fn close(&mut self) { let _ = self.proxy.send_event(Wake::Close); }
}

const SHADER: &str = "
struct VOut { @builtin(position) p: vec4<f32>, @location(0) c: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) col: vec4<f32>) -> VOut { var o: VOut; o.p = vec4<f32>(pos, 0.0, 1.0); o.c = col; return o; }
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> { return i.c; }
";

fn rgba(c: u32) -> [f32; 4] {
    [((c >> 24) & 255) as f32 / 255.0, ((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

/// The clear colour and the quads of a frame: a clear wipes everything before it (the frame replaces the surface); no clear means black.
fn vertices(f: &Frame, w: f32, h: f32) -> ([f32; 4], Vec<f32>) {
    let mut clear = [0.0, 0.0, 0.0, 1.0];
    let mut v: Vec<f32> = Vec::new();
    for c in &f.cmds {
        match *c {
            Cmd::Clear(col) => { clear = rgba(col); v.clear(); }
            Cmd::Rect { x, y, w: rw, h: rh, rgba: col } => {
                let (x0, y0, x1, y1) = (x / w * 2.0 - 1.0, 1.0 - y / h * 2.0, (x + rw) / w * 2.0 - 1.0, 1.0 - (y + rh) / h * 2.0);
                let k = rgba(col);
                for (px, py) in [(x0, y0), (x1, y0), (x0, y1), (x1, y0), (x1, y1), (x0, y1)] {
                    v.extend_from_slice(&[px, py, k[0], k[1], k[2], k[3]]);
                }
            }
        }
    }
    (clear, v)
}

/// Present-to-present intervals (F22 acceptance 3: the frame interval AS PRESENTED, not the CPU time to build a frame). Once a second a line goes out as an event:
/// "presented<TAB>frames N<TAB>interval p50 X ms<TAB>p99 Y ms<TAB>missed M" (missed = an interval over 25 ms).
struct Presented {
    last: Option<std::time::Instant>,
    since: std::time::Instant,
    ms: Vec<f64>,
}

impl Presented {
    fn new() -> Self { Presented { last: None, since: std::time::Instant::now(), ms: Vec::new() } }
    fn presented(&mut self) {
        let now = std::time::Instant::now();
        if let Some(l) = self.last { self.ms.push(now.duration_since(l).as_secs_f64() * 1000.0); }
        self.last = Some(now);
        if now.duration_since(self.since).as_secs() >= 1 && !self.ms.is_empty() {
            let mut v = std::mem::take(&mut self.ms);
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let q = |p: f64| v[(((v.len() as f64) * p).ceil() as usize).clamp(1, v.len()) - 1];
            let missed = v.iter().filter(|x| **x > 25.0).count();
            push_event(format!("presented\tframes {}\tinterval p50 {:.1} ms\tp99 {:.1} ms\tmissed {}", v.len() + 1, q(0.5), q(0.99), missed));
            self.since = now;
        }
    }
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
}

impl Gpu {
    fn render(&self, f: &Frame) {
        let tex = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => { self.surface.configure(&self.device, &self.config); return; }
        };
        let view = tex.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (clear, v) = vertices(f, self.config.width as f32, self.config.height as f32);
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&v), usage: wgpu::BufferUsages::VERTEX });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if !v.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..(v.len() / 6) as u32, 0..1);
            }
        }
        self.queue.submit(Some(enc.finish()));
        tex.present();
    }
}

fn init_gpu(window: Arc<winit::window::Window>, w: u32, h: u32) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let surface = instance.create_surface(window).map_err(|e| format!("surface: {}", e))?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: false }))
        .ok_or_else(|| "no adapter".to_string())?;
    let info = adapter.get_info();
    push_event(format!("info\t{:?}\t{}\t{}\t{}\t{:?}\twgpu 0.19", info.backend, info.name, info.driver, info.driver_info, info.device_type));
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: None, required_features: wgpu::Features::empty(), required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()) }, None))
        .map_err(|e| format!("device: {}", e))?;
    let mut config = surface.get_default_config(&adapter, w, h).ok_or_else(|| "surface not supported by the adapter".to_string())?;
    let caps = surface.get_capabilities(&adapter);
    if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) { config.format = *f; }
    config.present_mode = wgpu::PresentMode::Fifo;
    surface.configure(&device, &config);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: "vs", buffers: &[wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs }] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: "fs", targets: &[Some(wgpu::ColorTargetState { format: config.format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
    });
    Ok(Gpu { surface, device, queue, config, pipeline })
}

/// Open the window on its own thread; Err when no display is available (the event loop or the window cannot be created) or no adapter fits.
pub fn open(w: u32, h: u32, title: &str) -> Result<Box<dyn Sink>, String> {
    let slot: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
    let size = Arc::new(Mutex::new((w, h)));
    let (tx, rx) = mpsc::channel::<Result<EventLoopProxy<Wake>, String>>();
    let (slot2, size2, title2) = (slot.clone(), size.clone(), title.to_string());
    std::thread::spawn(move || {
        let mut b = EventLoopBuilder::<Wake>::with_user_event();
        winit::platform::x11::EventLoopBuilderExtX11::with_any_thread(&mut b, true);
        winit::platform::wayland::EventLoopBuilderExtWayland::with_any_thread(&mut b, true);
        let ev = match b.build() { Ok(e) => e, Err(e) => { let _ = tx.send(Err(format!("no display: {}", e))); return; } };
        let window = match WindowBuilder::new().with_title(title2).with_inner_size(PhysicalSize::new(w, h)).build(&ev) {
            Ok(win) => Arc::new(win),
            Err(e) => { let _ = tx.send(Err(format!("window: {}", e))); return; }
        };
        let gpu = match init_gpu(window.clone(), w, h) { Ok(g) => g, Err(e) => { let _ = tx.send(Err(e)); return; } };
        let _ = tx.send(Ok(ev.create_proxy()));
        let mut gpu = gpu;
        let mut last: Option<Frame> = None;
        let mut pres = Presented::new();
        let _ = ev.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(Wake::Close) => elwt.exit(),
                Event::UserEvent(Wake::Frame) => {
                    // take the frame first: the slot lock must not be held while rendering (presented() reports an event, which takes the state lock gfx_submit holds while it fills the slot)
                    let next = slot2.lock().unwrap().take();
                    if let Some(f) = next { gpu.render(&f); pres.presented(); last = Some(f); }
                }
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => push_event("close".to_string()),
                    WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                        gpu.config.width = s.width;
                        gpu.config.height = s.height;
                        gpu.surface.configure(&gpu.device, &gpu.config);
                        *size2.lock().unwrap() = (s.width, s.height);
                        push_event(format!("resize\t{}\t{}", s.width, s.height));
                        if let Some(f) = last.as_ref() { gpu.render(f); }
                    }
                    WindowEvent::KeyboardInput { event: k, .. } if k.state == ElementState::Pressed => {
                        let name = match &k.logical_key { Key::Named(n) => format!("{:?}", n), Key::Character(c) => c.to_string(), _ => "?".to_string() };
                        push_event(format!("key\t{}", name));
                    }
                    _ => {}
                },
                _ => {}
            }
        });
    });
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(proxy)) => Ok(Box::new(Win { slot, proxy, size })),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("timeout opening the window".to_string()),
    }
}
