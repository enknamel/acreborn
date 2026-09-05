//! The window: winit event loop, wgpu surface, and the egui launcher UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::config::{self, Account, Config, Server};
use crate::launch::{self, Options, Process};

/// Redraw at least this often so process exits show up without input.
const IDLE_REDRAW: Duration = Duration::from_millis(500);
const MAX_LOG_LINES: usize = 500;

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("aclauncher"),
                ..Default::default()
            }))?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface unsupported")?;
        config.format = config.format.remove_srgb_suffix();
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        Ok(Gpu {
            surface,
            device,
            queue,
            config,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }
}

/// egui state: context, winit bridge, wgpu renderer.
struct Egui {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

impl Egui {
    fn new(window: &Window, gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.config.format,
            egui_wgpu::RendererOptions::default(),
        );
        Egui {
            ctx,
            state,
            renderer,
        }
    }

    /// Run one frame of `ui` and draw it. Returns egui's requested repaint
    /// delay.
    fn frame(
        &mut self,
        window: &Window,
        gpu: &Gpu,
        ui: impl FnMut(&mut egui::Ui),
    ) -> Result<Duration> {
        let raw = self.state.take_egui_input(window);
        let full = self.ctx.run_ui(raw, ui);
        self.state
            .handle_platform_output(window, full.platform_output);
        let repaint = full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);
        let screen = ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full.pixels_per_point,
        };
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas.iter() {
                self.renderer
                    .update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let frames = self.ctx.tessellate(full.shapes, full.pixels_per_point);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return Ok(Duration::ZERO);
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer
            .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &frames, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.11,
                            g: 0.11,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &frames, &screen);
        }
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(frame);
        for id in &full.textures_delta.free {
            self.renderer.free_texture(id);
        }
        Ok(repaint)
    }
}

/// The server editor's scratch fields.
#[derive(Default)]
struct ServerEdit {
    name: String,
    host: String,
    port: String,
}

impl ServerEdit {
    fn from(s: &Server) -> Self {
        ServerEdit {
            name: s.name.clone(),
            host: s.host.clone(),
            port: s.port.to_string(),
        }
    }
}

/// The launcher application.
pub struct App {
    config_path: PathBuf,
    config: Config,
    dirty: bool,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    egui: Option<Egui>,
    selected_server: usize,
    server_edit: ServerEdit,
    server_edit_for: Option<usize>,
    new_account: String,
    new_password: String,
    processes: Vec<Process>,
    log: Vec<String>,
    error: Option<String>,
    next_redraw: Instant,
}

impl App {
    pub fn new(config_path: PathBuf, config: Config) -> Self {
        App {
            config_path,
            config,
            dirty: false,
            window: None,
            gpu: None,
            egui: None,
            selected_server: 0,
            server_edit: ServerEdit::default(),
            server_edit_for: None,
            new_account: String::new(),
            new_password: String::new(),
            processes: Vec::new(),
            log: Vec::new(),
            error: None,
            next_redraw: Instant::now(),
        }
    }

    /// Open the window and run until it closes.
    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Wait);
        event_loop.run_app(&mut self)?;
        self.save();
        Ok(())
    }

    fn log(&mut self, line: String) {
        tracing::info!("{line}");
        self.log.push(line);
        if self.log.len() > MAX_LOG_LINES {
            let drop = self.log.len() - MAX_LOG_LINES;
            self.log.drain(..drop);
        }
    }

    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        match self.config.save(&self.config_path) {
            Ok(()) => self.dirty = false,
            Err(e) => self.error = Some(format!("save config: {e:#}")),
        }
    }

    /// Poll every child; log exits.
    fn poll_processes(&mut self) {
        let mut ended = Vec::new();
        for p in &mut self.processes {
            if p.poll() {
                ended.push(format!("pid {} ({}) {}", p.pid, p.account, p.status_text()));
            }
        }
        for line in ended {
            self.log(line);
        }
    }

    fn launch(&mut self, index: usize, headless: bool) {
        let account = self.config.accounts[index].clone();
        let Some(server) = self.config.server(&account.server).cloned() else {
            self.error = Some(format!(
                "account {} has no server {}",
                account.account, account.server
            ));
            return;
        };
        let opts = Options {
            character: account.last_character.clone(),
            headless,
        };
        let launch = launch::build_launch(
            &launch::client_binary(&self.config),
            &self.config.data_dir,
            &server,
            &account,
            &opts,
        );
        match launch::spawn(&launch, &config::logs_dir(), &account) {
            Ok(p) => {
                let line = format!(
                    "pid {} launched {} on {}{} -> {}",
                    p.pid,
                    account.account,
                    server.name,
                    opts.character
                        .as_deref()
                        .filter(|c| !c.trim().is_empty())
                        .map(|c| format!(" as {c}"))
                        .unwrap_or_default(),
                    p.log.display()
                );
                self.processes.push(p);
                self.log(line);
                self.config
                    .record_launch(index, account.last_character.as_deref());
                self.dirty = true;
            }
            Err(e) => {
                let line = format!("launch {} failed: {e:#}", account.account);
                self.log(line.clone());
                self.error = Some(line);
            }
        }
    }

    fn add_account(&mut self) {
        let name = self.new_account.trim().to_string();
        if name.is_empty() {
            self.error = Some("account name is empty".into());
            return;
        }
        let Some(server) = self.config.servers.get(self.selected_server) else {
            return;
        };
        let server_name = server.name.clone();
        if self
            .config
            .find_account(&name, Some(&server_name))
            .is_some()
        {
            self.error = Some(format!("{name} already exists on {server_name}"));
            return;
        }
        self.config.accounts.push(Account {
            server: server_name,
            account: name,
            password: self.new_password.clone(),
            characters: Vec::new(),
            last_character: None,
            last_used: None,
        });
        self.new_account.clear();
        self.new_password.clear();
        self.dirty = true;
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.servers_panel(ui);
        self.log_panel(ui);
        self.accounts_panel(ui);
        if let Some(err) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Error")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(err);
                    if ui.button("OK").clicked() {
                        self.error = None;
                    }
                });
            if !open {
                self.error = None;
            }
        }
    }

    fn servers_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("servers")
            .default_size(220.0)
            .show(ui, |ui| {
                ui.heading("Servers");
                ui.add_space(4.0);
                let mut select = None;
                for (i, s) in self.config.servers.iter().enumerate() {
                    let label = format!("{}\n{}:{}", s.name, s.host, s.port);
                    if ui
                        .selectable_label(i == self.selected_server, label)
                        .clicked()
                    {
                        select = Some(i);
                    }
                }
                if let Some(i) = select {
                    self.selected_server = i;
                    self.server_edit_for = None;
                }
                ui.add_space(6.0);
                if ui.button("Add server").clicked() {
                    let mut n = 1;
                    let mut name = "New server".to_string();
                    while self.config.server(&name).is_some() {
                        n += 1;
                        name = format!("New server {n}");
                    }
                    self.config.servers.push(Server {
                        name,
                        host: "127.0.0.1".into(),
                        port: 9000,
                    });
                    self.selected_server = self.config.servers.len() - 1;
                    self.server_edit_for = None;
                    self.dirty = true;
                }
                ui.separator();
                if self.selected_server >= self.config.servers.len() {
                    self.selected_server = 0;
                }
                if self.config.servers.is_empty() {
                    ui.label("No servers.");
                    return;
                }
                let i = self.selected_server;
                if self.server_edit_for != Some(i) {
                    self.server_edit = ServerEdit::from(&self.config.servers[i]);
                    self.server_edit_for = Some(i);
                }
                ui.label("Edit");
                egui::Grid::new("server_edit")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut self.server_edit.name);
                        ui.end_row();
                        ui.label("Host");
                        ui.text_edit_singleline(&mut self.server_edit.host);
                        ui.end_row();
                        ui.label("Port");
                        ui.text_edit_singleline(&mut self.server_edit.port);
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.save_server_edit(i);
                    }
                    if ui.button("Remove").clicked() {
                        let s = self.config.servers.remove(i);
                        let n = self.config.accounts.len();
                        self.config.accounts.retain(|a| a.server != s.name);
                        let removed = n - self.config.accounts.len();
                        self.log(format!(
                            "removed server {} and {removed} account(s)",
                            s.name
                        ));
                        self.selected_server = 0;
                        self.server_edit_for = None;
                        self.dirty = true;
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                ui.label("Data dir");
                let mut data_dir = self.config.data_dir.to_string_lossy().into_owned();
                if ui.text_edit_singleline(&mut data_dir).changed() {
                    self.config.data_dir = PathBuf::from(data_dir);
                    self.dirty = true;
                }
                ui.label("Client (blank = auto)");
                let mut client = self.config.client_binary.join(" ");
                if ui.text_edit_singleline(&mut client).changed() {
                    self.config.client_binary =
                        client.split_whitespace().map(String::from).collect();
                    self.dirty = true;
                }
                if self.config.client_binary.is_empty() {
                    ui.small(launch::client_binary(&self.config).join(" "));
                }
            });
    }

    fn save_server_edit(&mut self, i: usize) {
        let name = self.server_edit.name.trim().to_string();
        let host = self.server_edit.host.trim().to_string();
        let port: u16 = match self.server_edit.port.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                self.error = Some(format!("bad port {:?}", self.server_edit.port));
                return;
            }
        };
        if name.is_empty() || host.is_empty() {
            self.error = Some("name and host are required".into());
            return;
        }
        if self
            .config
            .servers
            .iter()
            .enumerate()
            .any(|(j, s)| j != i && s.name == name)
        {
            self.error = Some(format!("a server named {name} already exists"));
            return;
        }
        let old = self.config.servers[i].name.clone();
        if old != name {
            for a in &mut self.config.accounts {
                if a.server == old {
                    a.server = name.clone();
                }
            }
        }
        self.config.servers[i] = Server { name, host, port };
        self.server_edit_for = None;
        self.dirty = true;
    }

    fn accounts_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(server) = self.config.servers.get(self.selected_server).cloned() else {
                ui.label("Add a server first.");
                return;
            };
            ui.heading(format!("Accounts on {}", server.name));
            if !self.config.password_notice_dismissed {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 180, 60),
                        format!(
                            "Passwords are stored in plain text in {}.",
                            self.config_path.display()
                        ),
                    );
                    if ui.small_button("Got it").clicked() {
                        self.config.password_notice_dismissed = true;
                        self.dirty = true;
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Account");
                ui.add(egui::TextEdit::singleline(&mut self.new_account).desired_width(140.0));
                ui.label("Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_password)
                        .password(true)
                        .desired_width(140.0),
                );
                if ui
                    .button("Add / create")
                    .on_hover_text("ACE creates the account on first login")
                    .clicked()
                {
                    self.add_account();
                }
            });
            ui.separator();

            let indices = self.config.accounts_on(&server.name);
            let mut launch_all = false;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!indices.is_empty(), egui::Button::new("Launch all"))
                    .clicked()
                {
                    launch_all = true;
                }
                ui.label(format!("{} account(s)", indices.len()));
            });
            ui.add_space(4.0);

            let mut to_launch: Vec<(usize, bool)> = Vec::new();
            let mut to_remove: Option<usize> = None;
            egui::ScrollArea::vertical()
                .id_salt("accounts")
                .show(ui, |ui| {
                    egui::Grid::new("accounts_grid")
                        .num_columns(4)
                        .striped(true)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.strong("Account");
                            ui.strong("Character");
                            ui.strong("Last used");
                            ui.strong("");
                            ui.end_row();
                            for &i in &indices {
                                let running = self
                                    .processes
                                    .iter()
                                    .filter(|p| {
                                        p.running() && p.account == self.config.accounts[i].account
                                    })
                                    .count();
                                let a = &mut self.config.accounts[i];
                                ui.label(if running > 0 {
                                    format!("{} ({running} running)", a.account)
                                } else {
                                    a.account.clone()
                                });
                                ui.horizontal(|ui| {
                                    let mut c = a.last_character.clone().unwrap_or_default();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut c)
                                                .hint_text("(first)")
                                                .desired_width(140.0),
                                        )
                                        .changed()
                                    {
                                        a.last_character = Some(c).filter(|c| !c.is_empty());
                                        self.dirty = true;
                                    }
                                    if !a.characters.is_empty() {
                                        let id = ui.make_persistent_id(("chars", i));
                                        egui::ComboBox::from_id_salt(id)
                                            .selected_text("")
                                            .width(20.0)
                                            .show_ui(ui, |ui| {
                                                for name in a.characters.clone().iter().rev() {
                                                    if ui.selectable_label(false, name).clicked() {
                                                        a.last_character = Some(name.clone());
                                                        self.dirty = true;
                                                    }
                                                }
                                            });
                                    }
                                });
                                ui.label(a.last_used.clone().unwrap_or_else(|| "never".into()));
                                ui.horizontal(|ui| {
                                    if ui.button("Launch").clicked() {
                                        to_launch.push((i, false));
                                    }
                                    if ui
                                        .button("Launch headless")
                                        .on_hover_text("adds --mute")
                                        .clicked()
                                    {
                                        to_launch.push((i, true));
                                    }
                                    if ui.button("Remove").clicked() {
                                        to_remove = Some(i);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if launch_all {
                to_launch.extend(indices.iter().map(|&i| (i, false)));
            }
            for (i, headless) in to_launch {
                self.launch(i, headless);
            }
            if let Some(i) = to_remove {
                let a = self.config.accounts.remove(i);
                self.log(format!(
                    "removed account {} (processes left running)",
                    a.account
                ));
                self.dirty = true;
            }
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Processes");
                    let running = self.processes.iter().filter(|p| p.running()).count();
                    ui.label(format!("{running} running"));
                    if ui
                        .add_enabled(running > 0, egui::Button::new("Kill all"))
                        .clicked()
                    {
                        for p in &mut self.processes {
                            p.kill();
                        }
                        self.log(format!("killed {running} process(es)"));
                    }
                    if ui.button("Clear finished").clicked() {
                        self.processes.retain(|p| p.running());
                    }
                });
                egui::ScrollArea::horizontal()
                    .id_salt("procs")
                    .max_height(80.0)
                    .show(ui, |ui| {
                        egui::Grid::new("procs_grid")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for p in &self.processes {
                                    ui.monospace(p.pid.to_string());
                                    ui.label(&p.account);
                                    ui.label(&p.server);
                                    ui.label(p.status_text());
                                    ui.end_row();
                                }
                            });
                    });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("log")
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("acreborn launcher")
            .with_inner_size(winit::dpi::LogicalSize::new(960, 640));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let gpu = match Gpu::new(window.clone()) {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("gpu: {e:#}");
                event_loop.exit();
                return;
            }
        };
        self.egui = Some(Egui::new(&window, &gpu));
        self.gpu = Some(gpu);
        self.window = Some(window);
        let bin = launch::client_binary(&self.config).join(" ");
        self.log(format!("config {}", self.config_path.display()));
        self.log(format!("client: {bin}"));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(e) = &mut self.egui {
            if !matches!(event, WindowEvent::RedrawRequested) {
                let r = e.state.on_window_event(&window, &event);
                if r.repaint {
                    window.request_redraw();
                }
                if r.consumed {
                    return;
                }
            }
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.poll_processes();
                let (Some(gpu), Some(mut egui)) = (self.gpu.take(), self.egui.take()) else {
                    return;
                };
                let result = egui.frame(&window, &gpu, |ui| self.ui(ui));
                self.gpu = Some(gpu);
                self.egui = Some(egui);
                self.save();
                match result {
                    Ok(delay) if delay.is_zero() => window.request_redraw(),
                    Ok(delay) => {
                        self.next_redraw = Instant::now() + delay.min(IDLE_REDRAW);
                    }
                    Err(e) => tracing::error!("render: {e:#}"),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_redraw {
            self.next_redraw = now + IDLE_REDRAW;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_redraw));
    }
}
