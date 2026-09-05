//! Reader for Turbine DAT archives (`client_portal.dat`, `client_cell_1.dat`,
//! `client_highres.dat`, `client_local_English.dat`).
//!
//! The container is a block-chained file with a B-tree directory:
//!
//! * A fixed [`Header`] lives at byte offset `0x140`.
//! * Every block is `block_size` bytes. Its first 4 bytes hold the absolute
//!   offset of the next block in the chain (0 for the last block); the
//!   remaining `block_size - 4` bytes are payload.
//! * A directory node is one chained object of [`DIR_NODE_SIZE`] bytes:
//!   62 branch offsets, an entry count, then 61 [`Entry`] slots. A node whose
//!   first branch is 0 is a leaf. Otherwise it has `entry_count + 1` children.
//! * Files are addressed by a 32-bit id. The id's high byte (portal) or low
//!   16 bits (cell) classify the file; see [`FileKind`].
//!
//! This crate knows nothing about the contents of files.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

pub const HEADER_OFFSET: usize = 0x140;
/// "BT" magic in the header's first field.
pub const MAGIC: u32 = 0x5442;
const HEADER_SIZE: usize = 15 * 4 + 16 + 4;
const BRANCHES: usize = 0x3E;
const MAX_ENTRIES: usize = 0x3D;
const ENTRY_SIZE: usize = 6 * 4;
/// Size of a serialized directory node.
pub const DIR_NODE_SIZE: usize = BRANCHES * 4 + 4 + MAX_ENTRIES * ENTRY_SIZE;
/// Id of the iteration (versioning) file present in every DAT.
pub const ITERATION_FILE_ID: u32 = 0xFFFF_0001;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("file too small to contain a DAT header")]
    TooSmall,
    #[error("bad magic {0:#x} (expected {MAGIC:#x})")]
    BadMagic(u32),
    #[error("unsupported block size {0}")]
    BadBlockSize(u32),
    #[error("offset {offset:#x} + {len} bytes is outside the archive ({file_len} bytes)")]
    OutOfBounds {
        offset: u64,
        len: usize,
        file_len: usize,
    },
    #[error("block chain at {0:#x} loops or exceeds the archive")]
    BadChain(u32),
    #[error("directory node at {0:#x} has invalid entry count")]
    BadDirectory(u32),
    #[error("no file with id {0:#010x}")]
    NotFound(u32),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Which archive this is, as recorded in `Header::data_set`.
/// `client_highres.dat` also reports `Portal`; `Language`/`HighRes` are
/// ACE's extensions, not values the client writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSet {
    Cell,
    Portal,
    Language,
    HighRes,
    Unknown(u32),
}

impl From<u32> for DataSet {
    fn from(v: u32) -> Self {
        match v {
            1 => DataSet::Portal,
            2 => DataSet::Cell,
            3 => DataSet::Language,
            4 => DataSet::HighRes,
            other => DataSet::Unknown(other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    pub file_type: u32,
    pub block_size: u32,
    pub file_size: u32,
    pub data_set: DataSet,
    pub data_subset: u32,
    pub free_head: u32,
    pub free_tail: u32,
    pub free_count: u32,
    pub btree: u32,
    pub new_lru: u32,
    pub old_lru: u32,
    pub use_lru: bool,
    pub master_map_id: u32,
    pub engine_pack_version: u32,
    pub game_pack_version: u32,
    pub version_major: [u8; 16],
    pub version_minor: u32,
}

impl Header {
    fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < HEADER_SIZE {
            return Err(Error::TooSmall);
        }
        let u = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        let file_type = u(0);
        if file_type != MAGIC {
            return Err(Error::BadMagic(file_type));
        }
        let block_size = u(1);
        if block_size <= 4 || block_size > 0x10000 {
            return Err(Error::BadBlockSize(block_size));
        }
        let mut version_major = [0u8; 16];
        version_major.copy_from_slice(&b[60..76]);
        Ok(Header {
            file_type,
            block_size,
            file_size: u(2),
            data_set: DataSet::from(u(3)),
            data_subset: u(4),
            free_head: u(5),
            free_tail: u(6),
            free_count: u(7),
            btree: u(8),
            new_lru: u(9),
            old_lru: u(10),
            use_lru: u(11) == 1,
            master_map_id: u(12),
            engine_pack_version: u(13),
            game_pack_version: u(14),
            version_major,
            version_minor: u32::from_le_bytes(b[76..80].try_into().unwrap()),
        })
    }
}

/// Directory entry for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub flags: u32,
    pub id: u32,
    /// Absolute offset of the first block of the file's chain.
    pub offset: u32,
    pub size: u32,
    pub date: u32,
    pub iteration: u32,
}

impl Entry {
    fn parse(b: &[u8]) -> Self {
        let u = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        Entry {
            flags: u(0),
            id: u(1),
            offset: u(2),
            size: u(3),
            date: u(4),
            iteration: u(5),
        }
    }
}

/// Coarse classification of a file id, following the client's conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    // cell.dat
    LandBlock,
    LandBlockInfo,
    EnvCell,
    // portal.dat, by high byte
    GfxObj,
    Setup,
    Animation,
    Palette,
    SurfaceTexture,
    Texture,
    Surface,
    MotionTable,
    Wave,
    Environment,
    PaletteSet,
    Clothing,
    DegradeInfo,
    Scene,
    Region,
    KeyMap,
    RenderTexture,
    RenderMaterial,
    MaterialModifier,
    MaterialInstance,
    SoundTable,
    UiLayout,
    EnumMapper,
    StringTable,
    DidMapper,
    ActionMap,
    DualDidMapper,
    CombatTable,
    String,
    ParticleEmitter,
    PhysicsScript,
    PhysicsScriptTable,
    MasterProperty,
    Font,
    StringState,
    DbProperties,
    // 0x0E tables
    QualityFilter,
    MonitoredProperties,
    CharacterGenerator,
    SecondaryAttributeTable,
    SkillTable,
    ChatPoseTable,
    ObjectHierarchy,
    SpellTable,
    SpellComponentTable,
    XpTable,
    BadData,
    ContractTable,
    TabooTable,
    FileToId,
    NameFilterTable,
    Iteration,
    Unknown,
}

impl FileKind {
    pub fn classify(data_set: DataSet, id: u32) -> FileKind {
        use FileKind::*;
        if id == ITERATION_FILE_ID {
            return Iteration;
        }
        if data_set == DataSet::Cell {
            return match id & 0xFFFF {
                0xFFFF => LandBlock,
                0xFFFE => LandBlockInfo,
                _ => EnvCell,
            };
        }
        match id >> 24 {
            0x01 => GfxObj,
            0x02 => Setup,
            0x03 => Animation,
            0x04 => Palette,
            0x05 => SurfaceTexture,
            0x06 => Texture,
            0x08 => Surface,
            0x09 => MotionTable,
            0x0A => Wave,
            0x0D => Environment,
            0x0F => PaletteSet,
            0x10 => Clothing,
            0x11 => DegradeInfo,
            0x12 => Scene,
            0x13 => Region,
            0x14 => KeyMap,
            0x15 => RenderTexture,
            0x16 => RenderMaterial,
            0x17 => MaterialModifier,
            0x18 => MaterialInstance,
            0x20 => SoundTable,
            0x21 => UiLayout,
            0x22 => EnumMapper,
            0x23 => StringTable,
            0x25 => DidMapper,
            0x26 => ActionMap,
            0x27 => DualDidMapper,
            0x30 => CombatTable,
            0x31 => String,
            0x32 => ParticleEmitter,
            0x33 => PhysicsScript,
            0x34 => PhysicsScriptTable,
            0x39 => MasterProperty,
            0x40 => Font,
            0x41 => StringState,
            0x78 => DbProperties,
            0x0E => match id {
                0x0E00_0002 => CharacterGenerator,
                0x0E00_0003 => SecondaryAttributeTable,
                0x0E00_0004 => SkillTable,
                0x0E00_0007 => ChatPoseTable,
                0x0E00_000D => ObjectHierarchy,
                0x0E00_000E => SpellTable,
                0x0E00_000F => SpellComponentTable,
                0x0E00_0018 => XpTable,
                0x0E00_001A => BadData,
                0x0E00_001D => ContractTable,
                0x0E00_001E => TabooTable,
                0x0E00_001F => FileToId,
                0x0E00_0020 => NameFilterTable,
                _ => match id >> 16 {
                    0x0E01 => QualityFilter,
                    0x0E02 => MonitoredProperties,
                    _ => Unknown,
                },
            },
            _ => Unknown,
        }
    }
}

/// An opened DAT archive. The whole file is memory-mapped; reads copy the
/// requested chain out of the map.
pub struct DatArchive {
    map: Mmap,
    header: Header,
    /// Directory entries sorted by id (the cell archive has close to a
    /// million, so a flat table beats a map for both opening and lookup).
    entries: Vec<Entry>,
}

impl DatArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the archive is opened read-only and is not expected to be
        // modified while mapped; a concurrent writer could cause a SIGBUS,
        // which is the standard mmap caveat we accept for a read-only tool.
        let map = unsafe { Mmap::map(&file)? };
        if map.len() < HEADER_OFFSET + HEADER_SIZE {
            return Err(Error::TooSmall);
        }
        let header = Header::parse(&map[HEADER_OFFSET..HEADER_OFFSET + HEADER_SIZE])?;
        let mut archive = DatArchive {
            map,
            header,
            entries: Vec::new(),
        };
        let mut entries = Vec::new();
        archive.walk_directory(archive.header.btree, &mut entries, 0)?;
        // An in-order walk of a well-formed B-tree is already sorted.
        if !entries.is_sorted_by_key(|e: &Entry| e.id) {
            entries.sort_unstable_by_key(|e| e.id);
        }
        entries.dedup_by_key(|e| e.id);
        archive.entries = entries;
        Ok(archive)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn data_set(&self) -> DataSet {
        self.header.data_set
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All directory entries, ordered by id.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn entry(&self, id: u32) -> Option<&Entry> {
        self.entries
            .binary_search_by_key(&id, |e| e.id)
            .ok()
            .map(|i| &self.entries[i])
    }

    pub fn kind(&self, id: u32) -> FileKind {
        FileKind::classify(self.header.data_set, id)
    }

    /// Read a whole file by id.
    pub fn read(&self, id: u32) -> Result<Vec<u8>> {
        let e = self.entry(id).ok_or(Error::NotFound(id))?;
        self.read_chain(e.offset, e.size as usize)
    }

    /// Follow a block chain starting at `offset`, returning `len` payload bytes.
    pub fn read_chain(&self, offset: u32, len: usize) -> Result<Vec<u8>> {
        let block = self.header.block_size as usize;
        let payload = block - 4;
        let mut out = Vec::with_capacity(len);
        let mut next = offset;
        // A chain can never legitimately be longer than the archive holds.
        let max_blocks = self.map.len() / block + 1;
        let mut visited = 0usize;
        while out.len() < len {
            if next == 0 || visited > max_blocks {
                return Err(Error::BadChain(offset));
            }
            visited += 1;
            let start = next as usize;
            let remaining = len - out.len();
            let take = remaining.min(payload);
            let blk = self.slice(start as u64, 4 + take)?;
            next = u32::from_le_bytes(blk[..4].try_into().unwrap());
            out.extend_from_slice(&blk[4..4 + take]);
        }
        Ok(out)
    }

    fn slice(&self, offset: u64, len: usize) -> Result<&[u8]> {
        let end = offset.checked_add(len as u64).ok_or(Error::OutOfBounds {
            offset,
            len,
            file_len: self.map.len(),
        })?;
        if end > self.map.len() as u64 {
            return Err(Error::OutOfBounds {
                offset,
                len,
                file_len: self.map.len(),
            });
        }
        Ok(&self.map[offset as usize..end as usize])
    }

    /// In-order walk: the subtree below branch `i` holds ids smaller than
    /// entry `i`, so entries come out sorted.
    fn walk_directory(&self, offset: u32, out: &mut Vec<Entry>, depth: u32) -> Result<()> {
        if depth > 64 {
            return Err(Error::BadDirectory(offset));
        }
        let node = self.read_chain(offset, DIR_NODE_SIZE)?;
        let u = |i: usize| u32::from_le_bytes(node[i * 4..i * 4 + 4].try_into().unwrap());
        let count = u(BRANCHES) as usize;
        if count > MAX_ENTRIES {
            return Err(Error::BadDirectory(offset));
        }
        let entries_base = BRANCHES * 4 + 4;
        let is_leaf = u(0) == 0;
        for i in 0..count {
            if !is_leaf {
                self.walk_directory(u(i), out, depth + 1)?;
            }
            out.push(Entry::parse(
                &node[entries_base + i * ENTRY_SIZE..][..ENTRY_SIZE],
            ));
        }
        if !is_leaf {
            self.walk_directory(u(count), out, depth + 1)?;
        }
        Ok(())
    }
}

/// The iteration file (`0xFFFF0001`): the archive's version counter, as
/// reported to the server during DDD negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iteration {
    pub total: i32,
    /// (starting iteration, consecutive count) pairs. More than one pair
    /// means the archive was only partially patched.
    pub ranges: Vec<(i32, i32)>,
}

impl Iteration {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let rd = |i: usize| {
            b.get(i * 4..i * 4 + 4)
                .map(|s| i32::from_le_bytes(s.try_into().unwrap()))
        };
        let total = rd(0)?;
        let mut ranges = Vec::new();
        let mut remaining = total;
        let mut i = 1;
        while remaining > 0 {
            let consecutive = rd(i)?;
            let start = rd(i + 1)?;
            ranges.push((start, consecutive));
            remaining += consecutive;
            i += 2;
        }
        Some(Iteration { total, ranges })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_node_size_matches_layout() {
        // 62 branches + count + 61 entries of 24 bytes.
        assert_eq!(DIR_NODE_SIZE, 62 * 4 + 4 + 61 * 24);
    }

    #[test]
    fn classify_cell_ids() {
        assert_eq!(
            FileKind::classify(DataSet::Cell, 0xA9B4_FFFF),
            FileKind::LandBlock
        );
        assert_eq!(
            FileKind::classify(DataSet::Cell, 0xA9B4_FFFE),
            FileKind::LandBlockInfo
        );
        assert_eq!(
            FileKind::classify(DataSet::Cell, 0xA9B4_0100),
            FileKind::EnvCell
        );
    }

    #[test]
    fn classify_portal_ids() {
        assert_eq!(
            FileKind::classify(DataSet::Portal, 0x0100_0001),
            FileKind::GfxObj
        );
        assert_eq!(
            FileKind::classify(DataSet::Portal, 0x0E00_000E),
            FileKind::SpellTable
        );
        assert_eq!(
            FileKind::classify(DataSet::Portal, 0x0E01_0001),
            FileKind::QualityFilter
        );
        assert_eq!(
            FileKind::classify(DataSet::Portal, ITERATION_FILE_ID),
            FileKind::Iteration
        );
    }

    #[test]
    fn iteration_parses_single_range() {
        let mut b = Vec::new();
        b.extend_from_slice(&2072i32.to_le_bytes());
        b.extend_from_slice(&(-2072i32).to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        let it = Iteration::parse(&b).unwrap();
        assert_eq!(it.total, 2072);
        assert_eq!(it.ranges, vec![(1, -2072)]);
    }
}
