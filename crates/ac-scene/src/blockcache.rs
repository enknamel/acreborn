//! Per-landblock collision and navigation, shared by every character in
//! the process.
//!
//! A character walking a block needs its static geometry as a
//! [`CollisionWorld`] (about 1.5 MB for a town block) and, once it has
//! to plan around a wall, a [`NavGraph`] over it. Neither depends on who
//! is walking, apart from the graph's capsule, so twenty followers in
//! Holtburg should hold one of each, not twenty. [`Assets`] owns one
//! [`BlockCache`]; characters keep `Rc`s to what they stand on and let
//! go of blocks they have left, so the cache's own bound (the most
//! recent [`KEEP`] blocks) is the process's bound.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::collision::{Capsule, CollisionWorld};
use crate::nav::NavGraph;
use crate::{Assets, Result};

/// How many blocks' collision the cache keeps; the least recently
/// asked for goes first. Characters spread over more than this many
/// blocks rebuild the ones that fell out, at ~0.5 s each.
pub const KEEP: usize = 48;

/// One block's static geometry and the graphs built over it, one per
/// capsule that has planned on it.
pub struct BlockCollision {
    /// The landblock id (`xxyy0000`).
    pub block: u32,
    pub world: CollisionWorld,
    /// A dungeon block: no terrain to walk on.
    pub dungeon: bool,
    navs: RefCell<Vec<(Capsule, Rc<RefCell<NavGraph>>)>>,
}

impl std::ops::Deref for BlockCollision {
    type Target = CollisionWorld;
    fn deref(&self) -> &CollisionWorld {
        &self.world
    }
}

impl BlockCollision {
    /// The navigation graph over this block for characters shaped like
    /// `cap`, built (empty; it fills in as paths are planned) on first
    /// use. Shared: whoever plans on it grows it for everyone.
    pub fn nav(&self, assets: &Assets, cap: &Capsule) -> Result<Rc<RefCell<NavGraph>>> {
        if let Some((_, n)) = self.navs.borrow().iter().find(|(c, _)| c.same(cap)) {
            return Ok(n.clone());
        }
        let scene = assets.landblock(self.block)?;
        let n = Rc::new(RefCell::new(NavGraph::for_scene(&scene, &self.world, cap)));
        self.navs.borrow_mut().push((*cap, n.clone()));
        Ok(n)
    }

    /// How many graphs (capsules) this block carries.
    pub fn nav_count(&self) -> usize {
        self.navs.borrow().len()
    }
}

/// The process's collision worlds, most recently used [`KEEP`] blocks.
#[derive(Default)]
pub struct BlockCache {
    blocks: RefCell<HashMap<u32, Rc<BlockCollision>>>,
    order: RefCell<VecDeque<u32>>,
}

impl BlockCache {
    /// The collision of landblock `block` (low 16 bits ignored), built
    /// from the assembled scene on first use.
    pub fn collision(&self, assets: &Assets, block: u32) -> Result<Rc<BlockCollision>> {
        let block = block & 0xFFFF_0000;
        if let Some(b) = self.blocks.borrow().get(&block) {
            self.touch(block);
            return Ok(b.clone());
        }
        let scene = assets.landblock(block)?;
        let world = CollisionWorld::from_scene(assets, &scene)?;
        let b = Rc::new(BlockCollision {
            block,
            world,
            dungeon: scene.is_dungeon,
            navs: Default::default(),
        });
        let mut blocks = self.blocks.borrow_mut();
        let mut order = self.order.borrow_mut();
        while order.len() >= KEEP {
            if let Some(old) = order.pop_front() {
                blocks.remove(&old);
            }
        }
        blocks.insert(block, b.clone());
        order.push_back(block);
        Ok(b)
    }

    /// The collision of `block` only if it is already built.
    pub fn cached(&self, block: u32) -> Option<Rc<BlockCollision>> {
        self.blocks.borrow().get(&(block & 0xFFFF_0000)).cloned()
    }

    /// How many blocks are held.
    pub fn len(&self) -> usize {
        self.blocks.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.borrow().is_empty()
    }

    fn touch(&self, block: u32) {
        let mut order = self.order.borrow_mut();
        if order.back() == Some(&block) {
            return;
        }
        if let Some(i) = order.iter().position(|&b| b == block) {
            order.remove(i);
            order.push_back(block);
        }
    }
}
