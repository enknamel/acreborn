//! Item and spell icons for egui: RenderSurface (0x06) ids become egui
//! textures on first use, through a loader the host installs once
//! (`Host::set_icon_loader`). Plugins reach the cache as `cx.icons()` and
//! paint an icon with [`IconCache::draw`].

use std::collections::HashMap;
use std::rc::Rc;

pub use ac_formats::texture::Rgba;

/// Decodes a RenderSurface id to RGBA. Installed by the host so this crate
/// stays free of DAT code; shared between every cache in the process.
pub type IconLoader = Rc<dyn Fn(u32) -> Option<Rgba>>;

/// Side of a drawn icon in points.
pub const ICON_SIZE: f32 = 24.0;

/// The layers of an object's icon, bottom to top; 0 = layer absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IconLayers {
    pub underlay: u32,
    pub icon: u32,
    pub overlay: u32,
}

impl IconLayers {
    /// Just the icon, nothing over or under it.
    pub fn single(icon: u32) -> Self {
        IconLayers {
            underlay: 0,
            icon,
            overlay: 0,
        }
    }

    /// An object's icon as the server described it.
    pub fn of(o: &ac_world::WorldObject) -> Self {
        IconLayers {
            underlay: o.icon_underlay,
            icon: o.icon_id,
            overlay: o.icon_overlay,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.underlay == 0 && self.icon == 0 && self.overlay == 0
    }
}

/// Icon id -> egui texture, loaded on first use through the [`IconLoader`].
/// Ids the loader could not decode are remembered so they are asked once.
#[derive(Default)]
pub struct IconCache {
    loader: Option<IconLoader>,
    textures: HashMap<u32, Option<egui::TextureHandle>>,
}

impl IconCache {
    /// Install the decoder. Icons already uploaded are dropped so they load
    /// again through the new loader.
    pub fn set_loader(&mut self, loader: IconLoader) {
        self.loader = Some(loader);
        self.textures.clear();
    }

    pub fn has_loader(&self) -> bool {
        self.loader.is_some()
    }

    /// The texture for an icon id, uploading it on first use. `None` when
    /// the id is 0, no loader is installed, or the surface did not decode.
    pub fn texture(&mut self, ctx: &egui::Context, id: u32) -> Option<egui::TextureId> {
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

    /// Allocate an [`ICON_SIZE`] square and paint the layers into it.
    pub fn draw(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers() {
        assert!(IconLayers::default().is_empty());
        assert!(!IconLayers::single(0x0600_0001).is_empty());
        assert_eq!(IconLayers::single(7).icon, 7);
    }

    #[test]
    fn cache_without_loader_yields_nothing() {
        let ctx = egui::Context::default();
        let mut cache = IconCache::default();
        assert!(!cache.has_loader());
        assert_eq!(cache.texture(&ctx, 0), None);
        assert_eq!(cache.texture(&ctx, 0x0600_0001), None);
    }

    #[test]
    fn cache_uploads_once_and_remembers_failures() {
        use std::cell::Cell;
        let asked = Rc::new(Cell::new(0));
        let n = asked.clone();
        let ctx = egui::Context::default();
        let mut cache = IconCache::default();
        cache.set_loader(Rc::new(move |id| {
            n.set(n.get() + 1);
            (id == 1).then(|| Rgba {
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 255],
            })
        }));
        assert!(cache.texture(&ctx, 1).is_some());
        assert!(cache.texture(&ctx, 1).is_some());
        assert!(cache.texture(&ctx, 2).is_none());
        assert!(cache.texture(&ctx, 2).is_none());
        assert_eq!(asked.get(), 2);
    }
}
