//! Turns decoded DAT assets into renderable geometry without touching a GPU:
//!
//! * [`Assets`]: memoizing loader over the portal and cell archives.
//! * [`terrain`]: landblock height grid -> triangle mesh, following the
//!   client's cell diagonal rule.
//! * [`model`]: GfxObj -> triangle lists grouped by surface; Setup -> parts
//!   with placement frames.
//! * [`landblock`]: a whole outdoor landblock (terrain + statics +
//!   buildings) as a list of placed models.

pub mod interior;
pub mod landblock;
pub mod model;
pub mod scenery;
pub mod terrain;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use ac_dat::DatArchive;
use ac_formats::{
    gfxobj::GfxObj, palette::Palette, region::Region, scene::Scene, setup::Setup, surface::Surface,
    surface_texture::SurfaceTexture, texture::Texture,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Dat(#[from] ac_dat::Error),
    #[error("{id:#010x}: {source}")]
    Format { id: u32, source: ac_formats::Error },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Constants of the outdoor world (also in Region's LandDefs).
pub const CELL_SIZE: f32 = 24.0;
pub const CELLS_PER_BLOCK: u32 = 8;
pub const BLOCK_SIZE: f32 = CELL_SIZE * CELLS_PER_BLOCK as f32;
pub const VERTS_PER_SIDE: usize = 9;

/// Memoizing asset loader. Single-threaded (`Rc`), intended to be owned by
/// the viewer or client thread.
pub struct Assets {
    pub portal: DatArchive,
    pub cell: DatArchive,
    region: RefCell<Option<Rc<Region>>>,
    gfxobjs: RefCell<HashMap<u32, Rc<GfxObj>>>,
    setups: RefCell<HashMap<u32, Rc<Setup>>>,
    surfaces: RefCell<HashMap<u32, Rc<Surface>>>,
    surface_textures: RefCell<HashMap<u32, Rc<SurfaceTexture>>>,
    textures: RefCell<HashMap<u32, Rc<Texture>>>,
    palettes: RefCell<HashMap<u32, Rc<Palette>>>,
    scenes: RefCell<HashMap<u32, Rc<Scene>>>,
}

macro_rules! cached {
    ($name:ident, $field:ident, $ty:ty, $archive:ident) => {
        pub fn $name(&self, id: u32) -> Result<Rc<$ty>> {
            if let Some(v) = self.$field.borrow().get(&id) {
                return Ok(v.clone());
            }
            let bytes = self.$archive.read(id)?;
            let v =
                Rc::new(<$ty>::parse(id, &bytes).map_err(|source| Error::Format { id, source })?);
            self.$field.borrow_mut().insert(id, v.clone());
            Ok(v)
        }
    };
}

impl Assets {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let d = data_dir.as_ref();
        Ok(Assets {
            portal: DatArchive::open(d.join("client_portal.dat"))?,
            cell: DatArchive::open(d.join("client_cell_1.dat"))?,
            region: RefCell::new(None),
            gfxobjs: Default::default(),
            setups: Default::default(),
            surfaces: Default::default(),
            surface_textures: Default::default(),
            textures: Default::default(),
            palettes: Default::default(),
            scenes: Default::default(),
        })
    }

    cached!(gfxobj, gfxobjs, GfxObj, portal);
    cached!(setup, setups, Setup, portal);
    cached!(surface, surfaces, Surface, portal);
    cached!(surface_texture, surface_textures, SurfaceTexture, portal);
    cached!(texture, textures, Texture, portal);
    cached!(palette, palettes, Palette, portal);
    cached!(scene, scenes, Scene, portal);

    pub fn region(&self) -> Result<Rc<Region>> {
        if let Some(r) = self.region.borrow().as_ref() {
            return Ok(r.clone());
        }
        let bytes = self.portal.read(Region::ID)?;
        let r = Rc::new(
            Region::parse(Region::ID, &bytes).map_err(|source| Error::Format {
                id: Region::ID,
                source,
            })?,
        );
        *self.region.borrow_mut() = Some(r.clone());
        Ok(r)
    }

    /// Resolve a Surface's texture to RGBA. Follows Surface -> SurfaceTexture
    /// (0x05) -> first Texture (0x06), applying the Surface's palette (or the
    /// texture's default) for indexed formats. `None` for solid-color surfaces.
    pub fn surface_rgba(&self, surface_id: u32) -> Result<Option<ac_formats::texture::Rgba>> {
        let s = self.surface(surface_id)?;
        let (tex_id, pal_id) = match s.base {
            ac_formats::surface::SurfaceBase::Solid { .. } => return Ok(None),
            ac_formats::surface::SurfaceBase::Image { texture, palette } => (texture, palette),
        };
        self.texture_rgba(tex_id, if pal_id != 0 { Some(pal_id) } else { None })
            .map(Some)
    }

    /// Decode a texture with explicit palette colors for indexed formats.
    pub fn texture_rgba_with_palette(
        &self,
        id: u32,
        colors: &[u32],
    ) -> Result<ac_formats::texture::Rgba> {
        let tex_id = self.resolve_texture_id(id)?;
        let t = self.texture(tex_id)?;
        t.to_rgba8(Some(colors))
            .map_err(|source| Error::Format { id: tex_id, source })
    }

    /// SurfaceTexture (0x05) -> first Texture (0x06) present; Texture ids pass through.
    pub fn resolve_texture_id(&self, id: u32) -> Result<u32> {
        if id >> 24 == 0x05 {
            let st = self.surface_texture(id)?;
            st.textures
                .iter()
                .copied()
                .find(|t| self.portal.entry(*t).is_some())
                .ok_or_else(|| {
                    Error::Other(format!(
                        "{id:#010x}: no texture variant present in portal.dat"
                    ))
                })
        } else {
            Ok(id)
        }
    }

    /// Decode a Texture (0x06) or SurfaceTexture (0x05) id to RGBA.
    pub fn texture_rgba(
        &self,
        id: u32,
        palette_override: Option<u32>,
    ) -> Result<ac_formats::texture::Rgba> {
        // A SurfaceTexture lists variants (high-res first); some live in
        // client_highres.dat, so take the first one present in portal.
        let tex_id = if id >> 24 == 0x05 {
            let st = self.surface_texture(id)?;
            *st.textures
                .iter()
                .find(|t| self.portal.entry(**t).is_some())
                .ok_or_else(|| {
                    Error::Other(format!(
                        "{id:#010x}: no texture variant present in portal.dat"
                    ))
                })?
        } else {
            id
        };
        let t = self.texture(tex_id)?;
        let pal = match palette_override.or(t.default_palette) {
            Some(pid) if t.format.is_indexed() => Some(self.palette(pid)?),
            _ => None,
        };
        t.to_rgba8(pal.as_ref().map(|p| p.colors.as_slice()))
            .map_err(|source| Error::Format { id: tex_id, source })
    }
}

/// Landblock id helpers. A landblock id is `XXYY0000`; cell ids are
/// `XXYYCCCC`.
pub mod lbid {
    use super::BLOCK_SIZE;
    use glam::Vec3;

    pub fn block_x(id: u32) -> u32 {
        id >> 24
    }
    pub fn block_y(id: u32) -> u32 {
        (id >> 16) & 0xFF
    }
    /// World-space origin of the landblock's local frame.
    pub fn world_origin(id: u32) -> Vec3 {
        Vec3::new(
            block_x(id) as f32 * BLOCK_SIZE,
            block_y(id) as f32 * BLOCK_SIZE,
            0.0,
        )
    }
    pub fn from_xy(x: u32, y: u32) -> u32 {
        (x << 24) | (y << 16)
    }
}
