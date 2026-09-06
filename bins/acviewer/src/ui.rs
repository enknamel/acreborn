//! egui overlay: the host's own widgets, a chat log with an input line and
//! a status line. Everything else on screen (vitals, radar, inventory...)
//! is a plugin, see `ac_plugin::panels`. The chat's data (tabs, unread
//! counts, scrollback, name lookup) lives in `crate::chat`.

use std::sync::Arc;

use ac_plugin::icons::{IconCache, IconLayers, IconLoader};
use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

use crate::chat::{self, ChatLine, ChatLog, Tab};

/// The chat window's width in points; it sits in the bottom-left corner.
const CHAT_WIDTH: f32 = 560.0;

pub struct Ui {
    pub ctx: egui::Context,
    /// Hide the status line and chat (the lobby screens before the world).
    pub hud_hidden: bool,
    state: Option<egui_winit::State>,
    renderer: egui_wgpu::Renderer,
    pub chat: ChatLog,
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
    /// Events queued by `inject` for the next frame (tests, automation).
    injected: Vec<egui::Event>,
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
            hud_hidden: false,
            state,
            injected: Vec::new(),
            renderer,
            chat: ChatLog::new(),
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

    /// Queue synthetic input for the next frame (headless tests and
    /// automation): key presses and typed text as egui events.
    pub fn inject(&mut self, events: impl IntoIterator<Item = egui::Event>) {
        self.injected.extend(events);
    }

    /// Add a line to the chat log, stamped with the local time.
    pub fn push_chat(&mut self, text: String, kind: u32) {
        self.chat.push(text, kind);
    }

    /// Put `/tell Name ` in the chat box, caret at the end, and focus it.
    fn start_tell(&mut self, name: &str) {
        self.input = chat::tell_prefix(name);
        let id = chat_input_id();
        let mut st = egui::text_edit::TextEditState::load(&self.ctx, id).unwrap_or_default();
        let end = egui::text::CCursor::new(self.input.chars().count());
        st.cursor
            .set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
        st.store(&self.ctx, id);
        self.chat_focus = true;
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
        let mut raw = match (&mut self.state, window) {
            (Some(s), Some(w)) => s.take_egui_input(w),
            _ => egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                ..Default::default()
            },
        };
        raw.events.append(&mut self.injected);
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
        // A clone of the handle (an Arc), so the closure can borrow the
        // rest of `self` (the chat log resizes and scrolls itself).
        let ctx_handle = self.ctx.clone();
        let mut full = ctx_handle.run_ui(raw, |ctx| {
            extra(ctx);
            if self.hud_hidden {
                return;
            }
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
            let max_h = (h - 40.0).max(ChatLog::MIN_HEIGHT);
            self.chat.height = self.chat.height.clamp(ChatLog::MIN_HEIGHT, max_h);
            let win_h = self.chat.height;
            egui::Window::new("chat")
                .fade_in(false)
                .title_bar(false)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(190))
                        .inner_margin(6),
                )
                .fixed_pos(egui::pos2(8.0, h - win_h - 10.0))
                .fixed_size(egui::vec2(CHAT_WIDTH, win_h))
                .show(ctx, |ui| {
                    ui.set_min_size(egui::vec2(CHAT_WIDTH - 12.0, win_h - 12.0));
                    ui.spacing_mut().item_spacing.y = 3.0;
                    // The top edge is a grip: drag it to resize the window
                    // (the bottom stays put).
                    let (grip_rect, grip) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 6.0),
                        egui::Sense::drag(),
                    );
                    let grip = grip.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                    if grip.dragged() {
                        self.chat.height = (self.chat.height - grip.drag_delta().y)
                            .clamp(ChatLog::MIN_HEIGHT, max_h);
                    }
                    let grip_color = if grip.hovered() || grip.dragged() {
                        egui::Color32::from_gray(200)
                    } else {
                        egui::Color32::from_gray(90)
                    };
                    ui.painter().hline(
                        grip_rect.center().x - 20.0..=grip_rect.center().x + 20.0,
                        grip_rect.center().y,
                        egui::Stroke::new(2.0, grip_color),
                    );
                    // Tabs, with the count of lines that arrived on a tab
                    // while another one was active.
                    let mut switch: Option<Tab> = None;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for tab in Tab::ALL {
                            let unread = self.chat.unread(tab);
                            let label = if unread > 0 {
                                format!("{} ({unread})", tab.label())
                            } else {
                                tab.label().to_string()
                            };
                            let active = tab == self.chat.tab;
                            let text = egui::RichText::new(label).color(if active {
                                egui::Color32::WHITE
                            } else if unread > 0 {
                                egui::Color32::from_rgb(255, 220, 120)
                            } else {
                                egui::Color32::from_gray(170)
                            });
                            if ui.selectable_label(active, text).clicked() && !active {
                                switch = Some(tab);
                            }
                        }
                    });
                    if let Some(tab) = switch {
                        self.chat.select(tab);
                    }
                    let input_h = ui.spacing().interact_size.y + 2.0 * ui.spacing().item_spacing.y;
                    let list_h = (ui.available_height() - input_h).max(40.0);
                    let mut tell: Option<String> = None;
                    let jump = std::mem::take(&mut self.chat.jump);
                    let out = egui::ScrollArea::vertical()
                        .id_salt("chat_log")
                        // Only follow new lines when the reader is at the
                        // bottom; scrolled up, the view stays put and the
                        // pill below counts what arrived.
                        .stick_to_bottom(self.chat.scroll.at_bottom)
                        .animated(false)
                        .auto_shrink([false, false])
                        .max_height(list_h)
                        .min_scrolled_height(list_h)
                        .show(ui, |ui| {
                            ui.set_min_height(list_h);
                            ui.set_min_width(CHAT_WIDTH - 20.0);
                            ui.spacing_mut().item_spacing.y = 1.0;
                            for line in self.chat.visible() {
                                if let Some(name) = chat_line(ui, line) {
                                    tell = Some(name);
                                }
                            }
                            if jump {
                                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                            }
                        });
                    let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                    if !jump {
                        self.chat.scroll.observe(out.state.offset.y, max);
                    }
                    if self.chat.scroll.unseen > 0 {
                        let n = self.chat.scroll.unseen;
                        let pill = egui::Rect::from_center_size(
                            egui::pos2(out.inner_rect.center().x, out.inner_rect.bottom() - 14.0),
                            egui::vec2(170.0, 22.0),
                        );
                        let text = egui::RichText::new(format!(
                            "\u{2193} {n} new message{}",
                            if n == 1 { "" } else { "s" }
                        ))
                        .color(egui::Color32::WHITE);
                        let button = egui::Button::new(text)
                            .fill(egui::Color32::from_rgb(60, 90, 140))
                            .corner_radius(11.0);
                        if ui.put(pill, button).clicked() {
                            self.chat.scroll.jump();
                            self.chat.jump = true;
                        }
                    }
                    if let Some(name) = tell {
                        self.start_tell(&name);
                    }
                    let edit = egui::TextEdit::singleline(&mut self.input)
                        .id(chat_input_id())
                        .hint_text("Enter to chat, @command for server commands")
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    if self.chat_focus && !r.has_focus() {
                        r.request_focus();
                        want_focus = true;
                    }
                    // Enter while the box is focused submits; egui 0.36's
                    // singleline edit keeps focus on Enter, so do not wait
                    // for `lost_focus`.
                    let enter = ui.input(|i| {
                        i.events.iter().any(|e| {
                            matches!(
                                e,
                                egui::Event::Key {
                                    key: egui::Key::Enter,
                                    pressed: true,
                                    ..
                                }
                            )
                        })
                    });
                    if enter && (r.has_focus() || r.lost_focus()) {
                        let t = self.input.trim().to_string();
                        if !t.is_empty() {
                            submit = Some(t);
                        }
                        self.input.clear();
                        self.chat_focus = false;
                        r.surrender_focus();
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

fn chat_input_id() -> egui::Id {
    egui::Id::new("chat_input")
}

/// One log line: a dim timestamp, then the text in its kind's colour
/// with the sender's name (if the line has one) as a link. Returns the
/// name when it was clicked.
fn chat_line(ui: &mut egui::Ui, line: &ChatLine) -> Option<String> {
    let mut clicked = None;
    let color = chat::color_for(line.kind);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(
            egui::RichText::new(format!("{} ", line.stamp))
                .color(chat::STAMP_COLOR)
                .small(),
        );
        match &line.name {
            Some(r) => {
                if r.start > 0 {
                    ui.label(egui::RichText::new(&line.text[..r.start]).color(color));
                }
                let name = &line.text[r.clone()];
                let link = ui
                    .add(egui::Link::new(egui::RichText::new(name).color(color)))
                    .on_hover_text(format!("Tell {}", name.trim_start_matches('+')));
                if link.clicked() {
                    clicked = Some(name.to_string());
                }
                ui.label(egui::RichText::new(&line.text[r.end..]).color(color));
            }
            None => {
                ui.label(egui::RichText::new(&line.text).color(color));
            }
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Enter focuses the box, typed text lands in it, Enter submits it.
    #[test]
    fn chat_box_submits_on_enter() {
        let Ok(gpu) = crate::gpu::Gpu::headless(320, 240) else {
            eprintln!("no GPU; skipping");
            return;
        };
        let (device, queue) = (gpu.device(), gpu.queue());
        let mut ui = Ui::new(device, gpu.format(), None, 320, 240);
        let mut frame = |ui: &mut Ui| ui.begin(None, &mut |_| {}, device, queue, 320, 240);
        frame(&mut ui); // fonts
        ui.chat_focus = true;
        frame(&mut ui); // focus requested
        frame(&mut ui); // focus taken
        ui.inject([egui::Event::Text("hello there".into())]);
        frame(&mut ui);
        assert_eq!(ui.input, "hello there");
        ui.inject([key(egui::Key::Enter, true), key(egui::Key::Enter, false)]);
        frame(&mut ui);
        frame(&mut ui);
        assert_eq!(ui.outgoing, vec!["hello there".to_string()]);
        assert!(ui.input.is_empty());
        assert!(!ui.chat_focus);
    }
}
