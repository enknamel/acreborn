//! Navigation over a neighbourhood of landblocks.
//!
//! [`crate::nav::NavGraph`] works on any rectangle, but the client has
//! so far only ever given it one landblock, which is why a character
//! walking towards a goal in the next block would run at a city wall:
//! the only route around it goes through a gate the single-block graph
//! could not see past. An [`Area`] assembles several neighbouring
//! blocks into one collision world with one terrain function over all
//! of them, so a path can leave the block it started in and come back.
//!
//! Areas are meant to be built off the frame thread: assembling nine
//! blocks costs tens of milliseconds and a long search across them
//! costs a hundred more. The graph fills in chunk by chunk as searches
//! reach it, so a second path through the same streets is much cheaper
//! than the first.

use std::collections::HashMap;
use std::rc::Rc;

use glam::{Vec2, Vec3};

use crate::collision::{Capsule, CollisionWorld};
use crate::landblock::LandblockScene;
use crate::nav::{self, Ground, NavGraph, INDOOR_SPACING, OUTDOOR_SPACING};
use crate::worldroute::{water_kind, Water};
use crate::{lbid, Assets, Result, BLOCK_SIZE};

/// Most blocks on a side of an area. Nine blocks is about 600 m across,
/// which covers any city and more than one travel leg.
pub const MAX_SIDE: u32 = 3;

/// The landblock a world position falls in.
pub fn block_at(p: Vec3) -> u32 {
    let x = (p.x / BLOCK_SIZE).floor().clamp(0.0, 254.0) as u32;
    let y = (p.y / BLOCK_SIZE).floor().clamp(0.0, 254.0) as u32;
    lbid::from_xy(x, y)
}

/// The blocks an area should cover to hold a walk from `from` to `to`:
/// the bounding box of the two, grown by one block, and clamped to
/// [`MAX_SIDE`] blocks a side around `from`.
pub fn blocks_for(from: Vec3, to: Vec3) -> Vec<u32> {
    let (a, b) = (block_at(from), block_at(to));
    let (ax, ay) = (lbid::block_x(a) as i32, lbid::block_y(a) as i32);
    let (bx, by) = (lbid::block_x(b) as i32, lbid::block_y(b) as i32);
    let half = (MAX_SIDE as i32 - 1) / 2;
    let span = |a: i32, b: i32| {
        let (lo, hi) = (a.min(b) - 1, a.max(b) + 1);
        // Keep the character's own block in the middle when the goal is
        // far enough that the box would be bigger than we allow.
        let (lo, hi) = if hi - lo + 1 > MAX_SIDE as i32 {
            (a - half, a + half)
        } else {
            (lo, hi)
        };
        (lo.max(0), hi.min(254))
    };
    let (x0, x1) = span(ax, bx);
    let (y0, y1) = span(ay, by);
    let mut out = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            out.push(lbid::from_xy(x as u32, y as u32));
        }
    }
    out
}

/// Several landblocks assembled into one thing paths can be planned on.
pub struct Area {
    /// The blocks covered, sorted.
    pub blocks: Vec<u32>,
    /// Their static geometry, merged.
    pub collision: CollisionWorld,
    /// The scenes, kept for their terrain.
    scenes: HashMap<u32, Rc<LandblockScene>>,
    pub nav: NavGraph,
    /// A dungeon area: no terrain under it, fine lattice.
    pub dungeon: bool,
    /// Which of the region's terrain types are open sea, by index. A
    /// character cannot walk into the ocean -- the server refuses the
    /// move and it stops dead against nothing, which is what an
    /// invisible wall in the water is -- so no route may cross one.
    /// Empty when the region could not be read, which costs us only
    /// routes planned into the sea.
    sea_types: Vec<bool>,
}

impl Area {
    /// Assemble `blocks`. A dungeon is assembled alone whatever else was
    /// asked for: the blocks around it on the lattice are unrelated
    /// pieces of the surface, and there is no terrain under it to walk.
    pub fn build(assets: &Assets, blocks: &[u32], cap: &Capsule, centre: u32) -> Result<Area> {
        let centre = centre & 0xFFFF_0000;
        let centre_scene = crate::landblock::load(assets, centre)?;
        let dungeon = centre_scene.is_dungeon;
        let wanted: Vec<u32> = if dungeon {
            vec![centre]
        } else {
            let mut v: Vec<u32> = blocks.iter().map(|b| b & 0xFFFF_0000).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let mut scenes: HashMap<u32, Rc<LandblockScene>> = HashMap::new();
        let mut collision = CollisionWorld::default();
        for &id in &wanted {
            let scene = if id == centre {
                centre_scene.clone()
            } else {
                match crate::landblock::load(assets, id) {
                    Ok(s) => s,
                    // A block off the edge of the world or unreadable:
                    // the area is simply smaller.
                    Err(_) => continue,
                }
            };
            // Never mix a dungeon's geometry into the surface: it sits
            // at the same coordinates as the ground above it.
            if scene.is_dungeon && id != centre {
                continue;
            }
            if let Ok(w) = CollisionWorld::from_scene(assets, &scene) {
                collision.absorb(&w);
            }
            scenes.insert(id, scene);
        }
        let blocks: Vec<u32> = {
            let mut v: Vec<u32> = scenes.keys().copied().collect();
            v.sort_unstable();
            v
        };
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &id in &blocks {
            let o = lbid::world_origin(id);
            lo = lo.min(Vec2::new(o.x, o.y));
            hi = hi.max(Vec2::new(o.x + BLOCK_SIZE, o.y + BLOCK_SIZE));
        }
        if dungeon {
            if let Some((clo, chi)) = collision.bounds() {
                lo = lo.min(Vec2::new(clo.x, clo.y));
                hi = hi.max(Vec2::new(chi.x, chi.y));
            }
        }
        let spacing = if dungeon {
            INDOOR_SPACING
        } else {
            OUTDOOR_SPACING
        };
        let nav = NavGraph::new(lo, hi, spacing, cap);
        let sea_types = match assets.region() {
            Ok(r) => (0..32u16)
                .map(|t| water_kind(&r, t) == Water::Sea)
                .collect(),
            Err(_) => Vec::new(),
        };
        Ok(Area {
            blocks,
            collision,
            scenes,
            nav,
            dungeon,
            sea_types,
        })
    }

    /// Open sea at a world `(x, y)`: every corner of the terrain cell it
    /// falls in is a sea type. A cell with a corner ashore is the beach,
    /// which can be waded, so only water all the way round counts.
    pub fn is_sea(&self, x: f32, y: f32) -> bool {
        if self.dungeon || self.sea_types.is_empty() {
            return false;
        }
        match self.scenes.get(&block_at(Vec3::new(x, y, 0.0))) {
            Some(s) => sea_at(&s.terrain, &self.sea_types, x, y, s.id),
            None => false,
        }
    }

    /// Run something against the area's ground: its merged collision,
    /// the terrain of whichever block a point falls in, and where the
    /// sea is. The graph is handed over at the same time because every
    /// caller needs both and they borrow different fields.
    fn with_ground<R>(&mut self, f: impl FnOnce(&Ground, &mut NavGraph) -> R) -> R {
        let Area {
            collision,
            scenes,
            nav,
            dungeon,
            sea_types,
            ..
        } = self;
        let terrain = |x: f32, y: f32| -> Option<f32> {
            let scene = scenes.get(&block_at(Vec3::new(x, y, 0.0)))?;
            let o = lbid::world_origin(scene.id);
            scene.terrain.height_at(Vec3::new(x - o.x, y - o.y, 0.0))
        };
        let sea = |x: f32, y: f32| -> bool {
            if sea_types.is_empty() {
                return false;
            }
            match scenes.get(&block_at(Vec3::new(x, y, 0.0))) {
                Some(s) => sea_at(&s.terrain, sea_types, x, y, s.id),
                None => false,
            }
        };
        let ground = Ground {
            collision,
            terrain: (!*dungeon).then_some(&terrain),
            sea: (!*dungeon).then_some(&sea),
        };
        f(&ground, nav)
    }

    /// This area can plan a walk from `from` to `to`.
    pub fn holds(&self, from: Vec3, to: Vec3) -> bool {
        self.blocks.contains(&block_at(from)) && self.blocks.contains(&block_at(to))
    }

    /// The capsule the graph was built for.
    pub fn capsule(&self) -> Capsule {
        self.nav.capsule
    }

    /// Height of the ground at a world `(x, y)`, from whichever block it
    /// falls in. `None` in a dungeon, and outside the area.
    pub fn terrain_at(&self, x: f32, y: f32) -> Option<f32> {
        if self.dungeon {
            return None;
        }
        let scene = self.scenes.get(&block_at(Vec3::new(x, y, 0.0)))?;
        let o = lbid::world_origin(scene.id);
        scene.terrain.height_at(Vec3::new(x - o.x, y - o.y, 0.0))
    }

    /// Waypoints from `from` to `to` around the area's geometry, ending
    /// with `to` and not including `from`. `None` when the two are not
    /// connected by anything the capsule can walk.
    pub fn path(&mut self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        self.with_ground(|ground, nav| nav.find_path(ground, from, to))
    }

    /// Build every chunk of the graph now. For tools and tests: paths
    /// build only what they cross.
    pub fn build_everything(&mut self) {
        self.with_ground(|ground, nav| nav.build_all(ground));
    }

    /// Nothing in the area crosses the chest-height line between the two
    /// points: the test that decides a route is not needed at all.
    pub fn line_clear(&self, from: Vec3, to: Vec3) -> bool {
        nav::line_clear(&self.collision, from, to)
    }
}

/// Whether every corner of the terrain cell holding world `(x, y)` is a
/// sea type. The mesh keeps a terrain type per lattice vertex, nine to a
/// side, so a cell's four corners are the four vertices around it.
fn sea_at(mesh: &crate::terrain::TerrainMesh, sea_types: &[bool], x: f32, y: f32, id: u32) -> bool {
    let o = lbid::world_origin(id);
    let n = crate::VERTS_PER_SIDE;
    let last = (crate::CELLS_PER_BLOCK - 1) as f32;
    let cx = ((x - o.x) / crate::CELL_SIZE).floor().clamp(0.0, last) as usize;
    let cy = ((y - o.y) / crate::CELL_SIZE).floor().clamp(0.0, last) as usize;
    [(cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)]
        .into_iter()
        .all(|(vx, vy)| {
            mesh.vertices
                .get(vx * n + vy)
                .and_then(|v| sea_types.get(v.terrain_type as usize))
                .copied()
                .unwrap_or(false)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_covers_the_walk_and_stays_within_its_size() {
        let o = |id: u32| lbid::world_origin(id) + Vec3::new(96.0, 96.0, 0.0);
        // Neighbours: the box around both, grown by one, is 3x3 already.
        let here = lbid::from_xy(0x33, 0xD9);
        let next = lbid::from_xy(0x34, 0xD9);
        let blocks = blocks_for(o(here), o(next));
        assert_eq!(blocks.len(), (MAX_SIDE * MAX_SIDE) as usize);
        assert!(blocks.contains(&here) && blocks.contains(&next));
        // A distant goal: the box is centred on where we are instead.
        let far = lbid::from_xy(0x50, 0xD9);
        let blocks = blocks_for(o(here), o(far));
        assert_eq!(blocks.len(), (MAX_SIDE * MAX_SIDE) as usize);
        assert!(blocks.contains(&here));
        assert!(!blocks.contains(&far));
        // The world's corner: clamped, so fewer blocks.
        let corner = lbid::from_xy(0, 0);
        let blocks = blocks_for(o(corner), o(corner));
        assert_eq!(blocks.len(), 4);
        assert!(blocks.contains(&corner));
    }

    #[test]
    fn positions_map_to_the_block_they_fall_in() {
        let id = lbid::from_xy(0x33, 0xD9);
        let o = lbid::world_origin(id);
        assert_eq!(block_at(o), id);
        assert_eq!(block_at(o + Vec3::new(191.0, 191.0, 0.0)), id);
        assert_eq!(
            block_at(o + Vec3::new(192.0, 0.0, 0.0)),
            lbid::from_xy(0x34, 0xD9)
        );
        // Off the map: clamped rather than wrapped into another block.
        assert_eq!(block_at(Vec3::new(-10.0, -10.0, 0.0)), lbid::from_xy(0, 0));
    }
}
