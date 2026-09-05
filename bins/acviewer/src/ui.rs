//! egui overlay: chat log with an input line, and a status line.

use std::collections::HashMap;
use std::sync::Arc;

use ac_formats::texture::Rgba;

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

/// One line of the skills panel.
pub struct SkillRow {
    pub name: &'static str,
    /// Current skill value (attribute base + creation bonus + ranks).
    pub value: u32,
    pub ranks: u16,
    /// Advancement class: 0 inactive, 1 untrained, 2 trained, 3 specialized.
    pub advancement: u32,
    pub training: &'static str,
}

/// One line of the spellbook panel.
pub struct SpellRow {
    pub id: u32,
    pub name: String,
    /// Spell level 1..=8 (from the spell's power).
    pub level: u32,
    pub school: &'static str,
    pub mana: u32,
    /// Only castable on ourselves; other spells need a selected target.
    pub self_targeted: bool,
    /// RenderSurface (0x06) id of the spell icon.
    pub icon: u32,
    /// Shown on hover: the spell's description and its incantation.
    pub description: String,
    pub words: String,
}

pub struct Sheet {
    pub name: String,
    pub level: i32,
    pub vitals: Vec<VitalBar>,
    pub total_xp: i64,
    pub available_xp: i64,
    pub skill_credits: i32,
    pub skills: Vec<SkillRow>,
}

/// `1234567` -> `1,234,567`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
    /// RenderSurface (0x06) ids: the icon, and what is drawn over and under
    /// it. 0 = none.
    pub icon: u32,
    pub icon_overlay: u32,
    pub icon_underlay: u32,
}

impl Item {
    fn layers(&self) -> IconLayers {
        IconLayers {
            underlay: self.icon_underlay,
            icon: self.icon,
            overlay: self.icon_overlay,
        }
    }
}

/// The layers of an object's icon, bottom to top; 0 = layer absent.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct IconLayers {
    pub underlay: u32,
    pub icon: u32,
    pub overlay: u32,
}

impl IconLayers {
    pub fn is_empty(&self) -> bool {
        self.underlay == 0 && self.icon == 0 && self.overlay == 0
    }
}

/// Decodes a RenderSurface id to RGBA; installed by the app so this module
/// stays free of DAT code.
pub type IconLoader = Box<dyn Fn(u32) -> Option<Rgba>>;

/// Icon id -> egui texture, loaded on first use through the [`IconLoader`].
/// Ids the loader could not decode are remembered so they are asked once.
#[derive(Default)]
struct IconCache {
    loader: Option<IconLoader>,
    textures: HashMap<u32, Option<egui::TextureHandle>>,
}

/// Side of a drawn icon in points.
const ICON_SIZE: f32 = 24.0;

impl IconCache {
    fn texture(&mut self, ctx: &egui::Context, id: u32) -> Option<egui::TextureId> {
        if id == 0 {
            return None;
        }
        if !self.textures.contains_key(&id) {
            let handle = self.loader.as_ref().and_then(|load| load(id)).map(|img| {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width as usize, img.height as usize],
                    &img.pixels,
                );
                ctx.load_texture(
                    format!("icon-{id:#010x}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.textures.insert(id, handle);
        }
        self.textures.get(&id)?.as_ref().map(|h| h.id())
    }

    /// Allocate an `ICON_SIZE` square and paint the layers into it.
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        layers: IconLayers,
        sense: egui::Sense,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(ICON_SIZE), sense);
        if ui.is_rect_visible(rect) {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            for id in [layers.underlay, layers.icon, layers.overlay] {
                if let Some(tex) = self.texture(ui.ctx(), id) {
                    ui.painter().image(tex, rect, uv, egui::Color32::WHITE);
                }
            }
        }
        resp
    }
}

/// A vendor's stock line or one of our items offered for sale.
pub struct TradeItem {
    pub guid: u32,
    pub name: String,
    pub price: u32,
    pub icon: u32,
    /// Unlimited supply (vendor stock) when true.
    pub unlimited: bool,
}

pub struct Vendor {
    pub name: String,
    pub stock: Vec<TradeItem>,
    pub selling: Vec<TradeItem>,
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
    pub sheet: Option<Sheet>,
    /// Inventory panel contents.
    pub items: Vec<Item>,
    /// Items double-clicked in the panel; drained by the caller.
    pub activated: Vec<u32>,
    pub show_inventory: bool,
    /// Skills panel (K).
    pub show_skills: bool,
    /// Spellbook panel (P) contents: the spells we know.
    pub spells: Vec<SpellRow>,
    pub show_spells: bool,
    /// Spells double-clicked in the spellbook; drained by the caller.
    pub cast_requests: Vec<u32>,
    /// Selected/attacked creature: name and health fraction.
    pub target: Option<(String, f32)>,
    /// Open ground container: name and items.
    pub loot: Option<(String, Vec<Item>)>,
    /// Loot items double-clicked or "take all"; drained by the caller.
    pub loot_take: Vec<u32>,
    pub loot_close: bool,
    /// Open vendor window, if any.
    pub vendor: Option<Vendor>,
    /// Buy/sell requests (guid) and close, drained by the caller.
    pub vendor_buy: Vec<u32>,
    pub vendor_sell: Vec<u32>,
    pub vendor_close: bool,
    pub combat: bool,
    pub blips: Vec<Blip>,
    /// Radar range in metres (edge of the circle).
    pub radar_range: f32,
    frames: Vec<egui::ClippedPrimitive>,
    free: Vec<egui::TextureId>,
    screen: ScreenDescriptor,
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
            sheet: None,
            items: Vec::new(),
            activated: Vec::new(),
            show_inventory: true,
            show_skills: false,
            spells: Vec::new(),
            show_spells: false,
            cast_requests: Vec::new(),
            target: None,
            loot: None,
            loot_take: Vec::new(),
            loot_close: false,
            vendor: None,
            vendor_buy: Vec::new(),
            vendor_sell: Vec::new(),
            vendor_close: false,
            combat: false,
            blips: Vec::new(),
            radar_range: 100.0,
            frames: Vec::new(),
            free: Vec::new(),
            screen: ScreenDescriptor {
                size_in_pixels: [width.max(1), height.max(1)],
                pixels_per_point: 1.0,
            },
            icons: IconCache::default(),
        }
    }

    /// Install the callback that decodes icon RenderSurfaces to RGBA.
    /// Icons are loaded lazily the first time they are drawn.
    pub fn set_icon_loader(&mut self, loader: IconLoader) {
        self.icons.loader = Some(loader);
        self.icons.textures.clear();
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
            if let Some((name, health)) = &self.target {
                egui::Area::new(egui::Id::new("target"))
                    .fade_in(false)
                    .fixed_pos(egui::pos2(w * 0.5 - 130.0, 8.0))
                    .show(ctx, |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(160))
                            .inner_margin(6)
                            .show(ui, |ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(248.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                let p = ui.painter();
                                p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
                                let mut fill = rect;
                                fill.set_width(rect.width() * health.clamp(0.0, 1.0));
                                p.rect_filled(fill, 3.0, egui::Color32::from_rgb(200, 40, 40));
                                p.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("{name}  {:.0}%", health * 100.0),
                                    egui::FontId::proportional(14.0),
                                    egui::Color32::WHITE,
                                );
                            });
                    });
            }
            if let Some(v) = &self.vendor {
                egui::Window::new("vendor")
                    .fade_in(false)
                    .title_bar(false)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(200))
                            .inner_margin(8),
                    )
                    .fixed_pos(egui::pos2(w * 0.5 - 220.0, 60.0))
                    .fixed_size(egui::vec2(440.0, 420.0))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(424.0, 404.0));
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&v.name)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            );
                            if ui.button("Close").clicked() {
                                self.vendor_close = true;
                            }
                        });
                        ui.columns(2, |cols| {
                            cols[0].label(
                                egui::RichText::new("For sale")
                                    .color(egui::Color32::from_gray(180)),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("stock")
                                .max_height(360.0)
                                .show(&mut cols[0], |ui| {
                                    for it in &v.stock {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Buy").clicked() {
                                                self.vendor_buy.push(it.guid);
                                            }
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{}  {}p",
                                                    it.name, it.price
                                                ))
                                                .color(egui::Color32::WHITE),
                                            );
                                        });
                                    }
                                });
                            cols[1].label(
                                egui::RichText::new("Your pack")
                                    .color(egui::Color32::from_gray(180)),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("sell")
                                .max_height(360.0)
                                .show(&mut cols[1], |ui| {
                                    for it in &v.selling {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Sell").clicked() {
                                                self.vendor_sell.push(it.guid);
                                            }
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{}  {}p",
                                                    it.name, it.price
                                                ))
                                                .color(egui::Color32::WHITE),
                                            );
                                        });
                                    }
                                });
                        });
                    });
            }
            if let Some((name, items)) = &self.loot {
                egui::Window::new("loot")
                    .fade_in(false)
                    .title_bar(false)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(190))
                            .inner_margin(8),
                    )
                    .fixed_pos(egui::pos2(w * 0.5 - 140.0, h * 0.5 - 120.0))
                    .fixed_size(egui::vec2(280.0, 240.0))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(264.0, 224.0));
                        ui.label(
                            egui::RichText::new(name)
                                .color(egui::Color32::WHITE)
                                .strong(),
                        );
                        egui::ScrollArea::vertical()
                            .max_height(170.0)
                            .show(ui, |ui| {
                                ui.set_min_width(250.0);
                                if items.is_empty() {
                                    ui.label(
                                        egui::RichText::new("(empty)")
                                            .color(egui::Color32::from_gray(170)),
                                    );
                                }
                                for it in items {
                                    let label = if it.stack > 1 {
                                        format!("{} ({})", it.name, it.stack)
                                    } else {
                                        it.name.clone()
                                    };
                                    let resp = ui
                                        .horizontal(|ui| {
                                            let icon = self.icons.draw(
                                                ui,
                                                it.layers(),
                                                egui::Sense::click(),
                                            );
                                            let text = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(label)
                                                        .color(egui::Color32::WHITE),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            icon.union(text)
                                        })
                                        .inner;
                                    if resp.double_clicked() {
                                        self.loot_take.push(it.guid);
                                    }
                                }
                            });
                        ui.horizontal(|ui| {
                            if ui.button("Take all").clicked() {
                                self.loot_take.extend(items.iter().map(|i| i.guid));
                            }
                            if ui.button("Close").clicked() {
                                self.loot_close = true;
                            }
                        });
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
                                    let resp = ui
                                        .horizontal(|ui| {
                                            let icon = self.icons.draw(
                                                ui,
                                                it.layers(),
                                                egui::Sense::click(),
                                            );
                                            let text = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(label).color(color),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            icon.union(text)
                                        })
                                        .inner;
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
            if let (Some(sheet), true) = (&self.sheet, self.show_skills) {
                egui::Window::new("skills")
                    .fade_in(false)
                    .title_bar(false)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(170))
                            .inner_margin(6),
                    )
                    .fixed_pos(egui::pos2(8.0, 132.0))
                    .fixed_size(egui::vec2(360.0, 380.0))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(348.0, 368.0));
                        ui.label(
                            egui::RichText::new(format!("{}  level {}", sheet.name, sheet.level))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "XP {}   unassigned {}   skill credits {}",
                                thousands(sheet.total_xp),
                                thousands(sheet.available_xp),
                                sheet.skill_credits
                            ))
                            .color(egui::Color32::from_gray(200))
                            .small(),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                ui.set_min_width(340.0);
                                egui::Grid::new("skills_grid")
                                    .num_columns(4)
                                    .spacing([14.0, 2.0])
                                    .show(ui, |ui| {
                                        let dim = egui::Color32::from_gray(170);
                                        for h in ["Skill", "Value", "Ranks", "Training"] {
                                            ui.label(egui::RichText::new(h).color(dim).small());
                                        }
                                        ui.end_row();
                                        for s in &sheet.skills {
                                            let color = match s.advancement {
                                                3 => egui::Color32::from_rgb(255, 215, 120),
                                                2 => egui::Color32::WHITE,
                                                1 => egui::Color32::from_gray(160),
                                                _ => egui::Color32::from_gray(110),
                                            };
                                            ui.label(egui::RichText::new(s.name).color(color));
                                            ui.label(
                                                egui::RichText::new(s.value.to_string())
                                                    .color(color)
                                                    .strong(),
                                            );
                                            ui.label(
                                                egui::RichText::new(s.ranks.to_string())
                                                    .color(color),
                                            );
                                            ui.label(egui::RichText::new(s.training).color(color));
                                            ui.end_row();
                                        }
                                    });
                            });
                    });
            }
            if self.sheet.is_some() && self.show_spells {
                // Sits beside the skills panel when both are open.
                let x = if self.show_skills { 380.0 } else { 8.0 };
                egui::Window::new("spellbook")
                    .fade_in(false)
                    .title_bar(false)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(170))
                            .inner_margin(6),
                    )
                    .fixed_pos(egui::pos2(x, 132.0))
                    .fixed_size(egui::vec2(380.0, 380.0))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(368.0, 368.0));
                        ui.label(
                            egui::RichText::new(format!("Spellbook ({})", self.spells.len()))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("double-click to cast")
                                .color(egui::Color32::from_gray(170))
                                .small(),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(330.0)
                            .show(ui, |ui| {
                                ui.set_min_width(360.0);
                                if self.spells.is_empty() {
                                    ui.label(
                                        egui::RichText::new("(no spells known)")
                                            .color(egui::Color32::from_gray(170)),
                                    );
                                }
                                let mut last_level = 0;
                                for sp in &self.spells {
                                    if sp.level != last_level {
                                        ui.label(
                                            egui::RichText::new(format!("Level {}", sp.level))
                                                .color(egui::Color32::from_gray(170))
                                                .small(),
                                        );
                                        last_level = sp.level;
                                    }
                                    let color = if sp.self_targeted {
                                        egui::Color32::from_rgb(180, 230, 180)
                                    } else {
                                        egui::Color32::WHITE
                                    };
                                    let resp = ui
                                        .horizontal(|ui| {
                                            let icon = self.icons.draw(
                                                ui,
                                                IconLayers {
                                                    underlay: 0,
                                                    icon: sp.icon,
                                                    overlay: 0,
                                                },
                                                egui::Sense::click(),
                                            );
                                            let text = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&sp.name).color(color),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            let detail = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(format!(
                                                        "{}  {} mana",
                                                        sp.school, sp.mana
                                                    ))
                                                    .color(egui::Color32::from_gray(170))
                                                    .small(),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            icon.union(text).union(detail)
                                        })
                                        .inner;
                                    let resp = resp.on_hover_text(format!(
                                        "{}\n{}\n{}",
                                        sp.description,
                                        sp.words,
                                        if sp.self_targeted {
                                            "self"
                                        } else {
                                            "needs a target"
                                        }
                                    ));
                                    if resp.double_clicked() {
                                        self.cast_requests.push(sp.id);
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
