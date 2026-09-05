//! egui overlay: the host's own widgets, a chat log with an input line and
//! a status line. Everything else on screen (vitals, radar, inventory...)
//! is a plugin, see `ac_plugin::panels`.

use std::sync::Arc;

use ac_plugin::icons::{IconCache, IconLayers, IconLoader};
use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

pub struct ChatLine {
    pub text: String,
    /// ChatMessageType from the server (0 broadcast, 2 speech, ...).
    pub kind: u32,
}

pub struct Ui {
    pub ctx: egui::Context,
    state: Option<egui_winit::State>,
    renderer: egui_wgpu::Renderer,
    pub chat: Vec<ChatLine>,
    pub input: String,
    /// The chat box has keyboard focus; game keys are suppressed.
    pub chat_focus: bool,
    /// Set when the user submits a chat line; drained by the caller.
    pub outgoing: Vec<String>,
    pub status: String,
    /// Icon of the selected object, drawn after the status text.
    pub status_icon: IconLayers,
    frames: Vec<egui::ClippedPrimitive>,
    free: Vec<egui::TextureId>,
    screen: ScreenDescriptor,
    /// For the status icon; plugins draw through the host's own cache.
    icons: IconCache,
}

impl Ui {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: Option<&Arc<Window>>,
        width: u32,
        height: u32,
    ) -> Self {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|st| {
            for f in st.text_styles.values_mut() {
                f.size *= 1.3;
            }
        });
        let state = window.map(|w| {
            egui_winit::State::new(
                ctx.clone(),
                egui::ViewportId::ROOT,
                w.as_ref(),
                Some(w.scale_factor() as f32),
                None,
                None,
            )
        });
        let renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Ui {
            ctx,
            state,
            renderer,
            chat: Vec::new(),
            input: String::new(),
            chat_focus: false,
            outgoing: Vec::new(),
            status: String::new(),
            status_icon: IconLayers::default(),
            frames: Vec::new(),
            free: Vec::new(),
            screen: ScreenDescriptor {
                size_in_pixels: [width.max(1), height.max(1)],
                pixels_per_point: 1.0,
            },
            icons: IconCache::default(),
        }
    }

    /// Install the callback that decodes icon RenderSurfaces to RGBA for
    /// the status icon. Icons are loaded lazily the first time they are
    /// drawn.
    pub fn set_icon_loader(&mut self, loader: IconLoader) {
        self.icons.set_loader(loader);
    }

    /// Feed a window event. Returns true if egui consumed it.
    pub fn on_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        match &mut self.state {
            Some(s) => s.on_window_event(window, event).consumed,
            None => false,
        }
    }

    pub fn push_chat(&mut self, text: String, kind: u32) {
        self.chat.push(ChatLine { text, kind });
        if self.chat.len() > 200 {
            self.chat.drain(..self.chat.len() - 200);
        }
    }

    /// Run the UI for this frame and prepare paint jobs.
    pub fn begin(
        &mut self,
        window: Option<&Window>,
        extra: &mut dyn FnMut(&egui::Context),
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) {
        let raw = match (&mut self.state, window) {
            (Some(s), Some(w)) => s.take_egui_input(w),
            _ => egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                ..Default::default()
            },
        };
        let ppp = match (&self.state, window) {
            (Some(_), Some(w)) => w.scale_factor() as f32,
            _ => 1.0,
        };
        self.screen = ScreenDescriptor {
            size_in_pixels: [width.max(1), height.max(1)],
            pixels_per_point: ppp,
        };
        let mut submit: Option<String> = None;
        let mut want_focus = false;
        let mut full = self.ctx.run_ui(raw, |ctx| {
            extra(ctx);
            egui::Area::new(egui::Id::new("status"))
                .fade_in(false)
                .fixed_pos(egui::pos2(8.0, 8.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&self.status)
                                .color(egui::Color32::WHITE)
                                .background_color(egui::Color32::from_black_alpha(140)),
                        );
                        if !self.status_icon.is_empty() {
                            self.icons.draw(ui, self.status_icon, egui::Sense::hover());
                        }
                    });
                });
            let h = height as f32 / ppp;
            egui::Window::new("chat")
                .fade_in(false)
                .title_bar(false)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(190))
                        .inner_margin(6),
                )
                .fixed_pos(egui::pos2(8.0, h - 250.0))
                .fixed_size(egui::vec2(560.0, 240.0))
                .show(ctx, |ui| {
                    ui.set_min_size(egui::vec2(548.0, 228.0));
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(190.0)
                        .min_scrolled_height(190.0)
                        .show(ui, |ui| {
                            ui.set_min_height(190.0);
                            ui.set_min_width(540.0);
                            for line in &self.chat {
                                let color = match line.kind {
                                    2 => egui::Color32::from_rgb(230, 230, 230),
                                    0 => egui::Color32::from_rgb(255, 220, 120),
                                    _ => egui::Color32::from_rgb(180, 210, 255),
                                };
                                ui.label(egui::RichText::new(&line.text).color(color));
                            }
                        });
                    let edit = egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Enter to chat, @command for server commands")
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    if self.chat_focus && !r.has_focus() {
                        r.request_focus();
                        want_focus = true;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let t = self.input.trim().to_string();
                        if !t.is_empty() {
                            submit = Some(t);
                        }
                        self.input.clear();
                        self.chat_focus = false;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && r.has_focus() {
                        self.chat_focus = false;
                        r.surrender_focus();
                    }
                });
        });
        let _ = want_focus;
        if let Some(t) = submit {
            self.outgoing.push(t);
        }
        if let (Some(s), Some(w)) = (&mut self.state, window) {
            s.handle_platform_output(w, full.platform_output);
        }
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas.iter() {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        self.free = full.textures_delta.free.iter().copied().collect();
        full.textures_delta.clear();
        let n_shapes = full.shapes.len();
        self.frames = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        tracing::trace!("ui: {n_shapes} shapes, {} primitives", self.frames.len());
    }

    /// Draw the prepared UI onto `view`.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        self.renderer
            .update_buffers(device, queue, encoder, &self.frames, &self.screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &self.frames, &self.screen);
        }
        for id in self.free.drain(..) {
            self.renderer.free_texture(&id);
        }
    }

    pub fn wants_keyboard(&self) -> bool {
        self.chat_focus || self.ctx.egui_wants_keyboard_input()
    }
}
