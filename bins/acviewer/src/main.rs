//! acviewer: fly around a landblock or inspect a model.
//!
//!   acviewer --landblock A9B4 [--radius 1]
//!   acviewer --model 02000001
//!
//! Controls: right mouse drag to look, WASD to move, Q/E down/up,
//! Shift to go faster, Escape to quit.

mod camera;
mod gpu;
mod scene;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Directory with client_portal.dat and client_cell_1.dat (default: $AC_DATA_DIR)
    #[arg(long, env = "AC_DATA_DIR")]
    data_dir: PathBuf,
    /// Landblock to show, hex (e.g. A9B4 for Holtburg)
    #[arg(long)]
    landblock: Option<String>,
    /// Extra rings of landblocks around the center
    #[arg(long, default_value_t = 1)]
    radius: u32,
    /// Model (GfxObj 01xxxxxx or Setup 02xxxxxx) to show, hex
    #[arg(long)]
    model: Option<String>,
    /// Render one frame to this PNG and exit (no window)
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Camera override for --screenshot: x,y,z,yaw_deg,pitch_deg
    #[arg(long)]
    camera: Option<String>,
}

struct App {
    cli: Cli,
    window: Option<Arc<Window>>,
    gpu: Option<gpu::Gpu>,
    camera: camera::Camera,
    keys: HashSet<KeyCode>,
    looking: bool,
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
}

impl App {
    fn load_scene(&mut self, gpu: &mut gpu::Gpu) -> Result<()> {
        let assets = ac_scene::Assets::open(&self.cli.data_dir).context("opening DAT archives")?;
        let built = if let Some(m) = &self.cli.model {
            let id = u32::from_str_radix(m.trim_start_matches("0x"), 16)?;
            scene::build_model(&assets, id)?
        } else {
            let lb = self.cli.landblock.as_deref().unwrap_or("A9B4");
            let id = u32::from_str_radix(lb.trim_start_matches("0x"), 16)? << 16;
            scene::build_landblocks(&assets, id, self.cli.radius)?
        };
        let tris: usize = built.batches.values().map(|b| b.indices.len() / 3).sum();
        tracing::info!(
            "{} materials, {tris} triangles, center {:?} radius {:.1}",
            built.batches.len(),
            built.center,
            built.radius
        );
        gpu.set_scene(built.batches, |k| scene::material_image(&assets, k));
        let d = built.radius.max(5.0);
        self.camera.position = built.center + Vec3::new(0.0, -d * 0.8, d * 0.5);
        self.camera.yaw = 0.0;
        self.camera.pitch = -0.45;
        self.camera.speed = (d * 0.25).max(2.0);
        self.camera.far = (d * 10.0).max(500.0);
        Ok(())
    }

    fn update(&mut self, dt: f32) {
        let mut v = Vec3::ZERO;
        let f = self.camera.forward();
        let r = self.camera.right();
        if self.keys.contains(&KeyCode::KeyW) {
            v += f;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            v -= f;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            v += r;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            v -= r;
        }
        if self.keys.contains(&KeyCode::KeyE) || self.keys.contains(&KeyCode::Space) {
            v += Vec3::Z;
        }
        if self.keys.contains(&KeyCode::KeyQ) {
            v -= Vec3::Z;
        }
        let boost = if self.keys.contains(&KeyCode::ShiftLeft) {
            4.0
        } else {
            1.0
        };
        self.camera.position += v.normalize_or_zero() * self.camera.speed * boost * dt;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("acviewer")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut gpu = gpu::Gpu::new(window.clone()).expect("gpu");
        if let Err(e) = self.load_scene(&mut gpu) {
            tracing::error!("{e:#}");
            event_loop.exit();
            return;
        }
        self.gpu = Some(gpu);
        self.window = Some(window);
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    }
                    match event.state {
                        ElementState::Pressed => {
                            self.keys.insert(code);
                        }
                        ElementState::Released => {
                            self.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                self.looking = state == ElementState::Pressed;
                self.last_cursor = None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.looking {
                    if let Some((lx, ly)) = self.last_cursor {
                        self.camera
                            .look((position.x - lx) as f32, (position.y - ly) as f32);
                    }
                    self.last_cursor = Some((position.x, position.y));
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                self.update(dt);
                if let Some(g) = &mut self.gpu {
                    let vp = self.camera.view_proj(g.aspect());
                    if let Err(e) = g.render(vp, Vec3::new(0.4, 0.3, 1.0)) {
                        tracing::error!("render: {e:#}");
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("acviewer=info".parse()?),
        )
        .init();
    let cli = Cli::parse();
    if let Some(path) = cli.screenshot.clone() {
        let mut gpu = gpu::Gpu::headless(1280, 800)?;
        let mut app = App {
            cli,
            window: None,
            gpu: None,
            camera: camera::Camera {
                position: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                fov_y: 60f32.to_radians(),
                near: 0.5,
                far: 2000.0,
                speed: 20.0,
            },
            keys: HashSet::new(),
            looking: false,
            last_cursor: None,
            last_frame: Instant::now(),
        };
        app.load_scene(&mut gpu)?;
        if let Some(c) = &app.cli.camera {
            let v: Vec<f32> = c
                .split(',')
                .map(|x| x.trim().parse())
                .collect::<std::result::Result<_, _>>()?;
            anyhow::ensure!(v.len() == 5, "--camera wants x,y,z,yaw,pitch");
            app.camera.position = Vec3::new(v[0], v[1], v[2]);
            app.camera.yaw = v[3].to_radians();
            app.camera.pitch = v[4].to_radians();
        }
        let vp = app.camera.view_proj(gpu.aspect());
        gpu.render_to_png(vp, Vec3::new(0.4, 0.3, 1.0), &path)?;
        tracing::info!("wrote {}", path.display());
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cli,
        window: None,
        gpu: None,
        camera: camera::Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60f32.to_radians(),
            near: 0.5,
            far: 2000.0,
            speed: 20.0,
        },
        keys: HashSet::new(),
        looking: false,
        last_cursor: None,
        last_frame: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
