//! egui overlay: chat log with an input line, and a status line.

use std::sync::Arc;

use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

pub struct ChatLine {
    pub text: String,
    /// ChatMessageType from the server (0 broadcast, 2 speech, ...).
    pub kind: u32,
}

/// One vital bar: label, current, maximum.
pub struct VitalBar {
    pub name: &'static str,
    pub current: u32,
    pub max: u32,
}

pub struct Sheet {
    pub name: String,
    pub level: i32,
    pub vitals: Vec<VitalBar>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlipKind {
    Player,
    Creature,
    Other,
}

/// A radar blip in radar space: x right, y forward, metres.
pub struct Blip {
    pub x: f32,
    pub y: f32,
    pub kind: BlipKind,
}

pub struct Item {
    pub guid: u32,
    pub name: String,
    pub stack: u32,
    pub wielded: bool,
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
    pub sheet: Option<Sheet>,
    /// Inventory panel contents.
    pub items: Vec<Item>,
    /// Items double-clicked in the panel; drained by the caller.
    pub activated: Vec<u32>,
    pub show_inventory: bool,
    pub blips: Vec<Blip>,
    /// Radar range in metres (edge of the circle).
    pub radar_range: f32,
    frames: Vec<egui::ClippedPrimitive>,
    free: Vec<egui::TextureId>,
    screen: ScreenDescriptor,
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
            sheet: None,
            items: Vec::new(),
            activated: Vec::new(),
            show_inventory: true,
            blips: Vec::new(),
            radar_range: 100.0,
            frames: Vec::new(),
            free: Vec::new(),
            screen: ScreenDescriptor {
                size_in_pixels: [width.max(1), height.max(1)],
                pixels_per_point: 1.0,
            },
        }
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
            egui::Area::new(egui::Id::new("status"))
                .fade_in(false)
                .fixed_pos(egui::pos2(8.0, 8.0))
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(&self.status)
                            .color(egui::Color32::WHITE)
                            .background_color(egui::Color32::from_black_alpha(140)),
                    );
                });
            let w = width as f32 / ppp;
            let h = height as f32 / ppp;
            if let Some(sheet) = &self.sheet {
                egui::Area::new(egui::Id::new("vitals"))
                    .fade_in(false)
                    .fixed_pos(egui::pos2(8.0, 36.0))
                    .show(ctx, |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(160))
                            .inner_margin(6)
                            .show(ui, |ui| {
                                ui.set_min_width(220.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}  (level {})",
                                        sheet.name, sheet.level
                                    ))
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                                );
                                for (i, v) in sheet.vitals.iter().enumerate() {
                                    let color = [
                                        egui::Color32::from_rgb(200, 40, 40),
                                        egui::Color32::from_rgb(220, 180, 40),
                                        egui::Color32::from_rgb(50, 90, 220),
                                    ][i.min(2)];
                                    let frac = if v.max > 0 {
                                        v.current as f32 / v.max as f32
                                    } else {
                                        0.0
                                    };
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(220.0, 16.0),
                                        egui::Sense::hover(),
                                    );
                                    let p = ui.painter();
                                    p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
                                    let mut fill = rect;
                                    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
                                    p.rect_filled(fill, 3.0, color);
                                    p.text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        format!("{} {}/{}", v.name, v.current, v.max),
                                        egui::FontId::proportional(13.0),
                                        egui::Color32::WHITE,
                                    );
                                }
                            });
                    });
            }
            if self.sheet.is_some() {
                let r = 80.0;
                let center = egui::pos2(w - r - 16.0, r + 16.0);
                egui::Area::new(egui::Id::new("radar"))
                    .fade_in(false)
                    .fixed_pos(egui::pos2(center.x - r - 4.0, center.y - r - 4.0))
                    .show(ctx, |ui| {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(2.0 * r + 8.0, 2.0 * r + 8.0),
                            egui::Sense::hover(),
                        );
                        let c = rect.center();
                        let p = ui.painter();
                        p.circle_filled(c, r, egui::Color32::from_black_alpha(160));
                        p.circle_stroke(
                            c,
                            r,
                            egui::Stroke::new(1.5, egui::Color32::from_gray(180)),
                        );
                        p.circle_stroke(
                            c,
                            r * 0.5,
                            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                        );
                        p.line_segment(
                            [egui::pos2(c.x, c.y - r), egui::pos2(c.x, c.y + r)],
                            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                        );
                        p.line_segment(
                            [egui::pos2(c.x - r, c.y), egui::pos2(c.x + r, c.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                        );
                        let scale = r / self.radar_range.max(1.0);
                        for b in &self.blips {
                            let d = (b.x * b.x + b.y * b.y).sqrt();
                            if d > self.radar_range {
                                continue;
                            }
                            let at = egui::pos2(c.x + b.x * scale, c.y - b.y * scale);
                            let (col, rad) = match b.kind {
                                BlipKind::Player => (egui::Color32::from_rgb(90, 220, 90), 3.5),
                                BlipKind::Creature => (egui::Color32::from_rgb(230, 120, 40), 3.0),
                                BlipKind::Other => (egui::Color32::from_gray(200), 2.0),
                            };
                            p.circle_filled(at, rad, col);
                        }
                        p.circle_filled(c, 3.0, egui::Color32::WHITE);
                    });
            }
            if self.sheet.is_some() && self.show_inventory {
                let r = 80.0;
                egui::Window::new("inventory")
                    .fade_in(false)
                    .title_bar(false)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(170))
                            .inner_margin(6),
                    )
                    .fixed_pos(egui::pos2(w - 268.0, 2.0 * r + 40.0))
                    .fixed_size(egui::vec2(260.0, 300.0))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(248.0, 288.0));
                        ui.label(
                            egui::RichText::new(format!(
                                "Inventory ({})",
                                self.items.iter().filter(|i| !i.wielded).count()
                            ))
                            .color(egui::Color32::WHITE)
                            .strong(),
                        );
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                ui.set_min_width(240.0);
                                let mut shown_wielded_header = false;
                                let mut shown_pack_header = false;
                                for it in &self.items {
                                    if it.wielded && !shown_wielded_header {
                                        ui.label(
                                            egui::RichText::new("Worn")
                                                .color(egui::Color32::from_gray(170))
                                                .small(),
                                        );
                                        shown_wielded_header = true;
                                    }
                                    if !it.wielded && !shown_pack_header {
                                        ui.label(
                                            egui::RichText::new("Pack")
                                                .color(egui::Color32::from_gray(170))
                                                .small(),
                                        );
                                        shown_pack_header = true;
                                    }
                                    let label = if it.stack > 1 {
                                        format!("{} ({})", it.name, it.stack)
                                    } else {
                                        it.name.clone()
                                    };
                                    let color = if it.wielded {
                                        egui::Color32::from_rgb(180, 230, 180)
                                    } else {
                                        egui::Color32::WHITE
                                    };
                                    let resp = ui.add(
                                        egui::Label::new(egui::RichText::new(label).color(color))
                                            .sense(egui::Sense::click()),
                                    );
                                    if resp.double_clicked() {
                                        self.activated.push(it.guid);
                                    }
                                    if resp.hovered() {
                                        ui.output_mut(|o| {
                                            o.cursor_icon = egui::CursorIcon::PointingHand
                                        });
                                    }
                                }
                            });
                    });
            }
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
