//! DualDidMapper (0x27): two enum → data-id tables (a "client" and a
//! "server" one, each with names) that the client uses to find the weenie
//! class of a game object from an enum value. The ones in the portal:
//! 0x27000000 materials, 0x27000001 gems, 0x27000002 spell components
//! (component id → weenie class id, the mapping the server uses to check
//! and burn components), 0x27000003 component packs, 0x27000004 trade
//! notes. Layout cross-checked against ACE's `DualDidMapper` and the raw
//! file: after the id, four sections each of `u8 numbering type, packed
//! count, entries`; the id sections hold `(u32 enum, u32 id)`, the name
//! sections `(u32 enum, u8-length string)`.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Default, Serialize)]
pub struct DualDidMapper {
    pub id: u32,
    pub client_numbering: u8,
    /// (enum value, data id), sorted by enum value.
    pub client_ids: Vec<(u32, u32)>,
    pub client_name_numbering: u8,
    /// (enum value, name), sorted by enum value.
    pub client_names: Vec<(u32, String)>,
    pub server_numbering: u8,
    pub server_ids: Vec<(u32, u32)>,
    pub server_name_numbering: u8,
    pub server_names: Vec<(u32, String)>,
    /// (data id, enum value), sorted by data id: the reverse of
    /// `client_ids`.
    #[serde(skip)]
    by_id: Vec<(u32, u32)>,
}

impl DualDidMapper {
    /// Spell component id → weenie class id.
    pub const SPELL_COMPONENTS: u32 = 0x2700_0002;

    pub fn parse(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = expect_id(&mut r, id)?;
        let client_numbering = r.u8()?;
        let mut client_ids = r.packed_map(|r| r.u32(), |r| r.u32())?;
        let client_name_numbering = r.u8()?;
        let mut client_names = r.packed_map(|r| r.u32(), |r| r.pstring8())?;
        let server_numbering = r.u8()?;
        let mut server_ids = r.packed_map(|r| r.u32(), |r| r.u32())?;
        let server_name_numbering = r.u8()?;
        let mut server_names = r.packed_map(|r| r.u32(), |r| r.pstring8())?;
        r.finish()?;
        client_ids.sort_by_key(|(k, _)| *k);
        client_names.sort_by_key(|(k, _)| *k);
        server_ids.sort_by_key(|(k, _)| *k);
        server_names.sort_by_key(|(k, _)| *k);
        let mut by_id: Vec<(u32, u32)> = client_ids.iter().map(|&(e, i)| (i, e)).collect();
        by_id.sort_by_key(|(k, _)| *k);
        Ok(DualDidMapper {
            id,
            client_numbering,
            client_ids,
            client_name_numbering,
            client_names,
            server_numbering,
            server_ids,
            server_name_numbering,
            server_names,
            by_id,
        })
    }

    /// The data id (for 0x27000002: the weenie class id) of an enum value
    /// in the client table.
    pub fn id_of(&self, value: u32) -> Option<u32> {
        self.client_ids
            .binary_search_by_key(&value, |(k, _)| *k)
            .ok()
            .map(|i| self.client_ids[i].1)
    }

    /// The enum value whose client-table data id is `id`; the first one
    /// when several share it.
    pub fn value_of(&self, id: u32) -> Option<u32> {
        let i = self.by_id.partition_point(|(k, _)| *k < id);
        self.by_id.get(i).filter(|(k, _)| *k == id).map(|(_, v)| *v)
    }

    /// The client table's name for an enum value.
    pub fn name_of(&self, value: u32) -> Option<&str> {
        self.client_names
            .binary_search_by_key(&value, |(k, _)| *k)
            .ok()
            .map(|i| self.client_names[i].1.as_str())
    }

    /// Spell component id → weenie class id (the 0x27000002 table).
    pub fn component_wcid(&self, component_id: u32) -> Option<u32> {
        self.id_of(component_id)
    }

    /// Weenie class id → spell component id (the 0x27000002 table).
    pub fn component_of_wcid(&self, wcid: u32) -> Option<u32> {
        self.value_of(wcid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pstring8(w: &mut Vec<u8>, s: &str) {
        w.push(s.len() as u8);
        w.extend_from_slice(s.as_bytes());
    }

    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&DualDidMapper::SPELL_COMPONENTS.to_le_bytes());
        // client ids: numbering 3, three entries (one of them the (0, 0)
        // slot the real file carries), out of order
        b.push(3);
        b.push(3);
        for (e, id) in [(178u32, 8357u32), (0, 0), (1, 691)] {
            b.extend_from_slice(&e.to_le_bytes());
            b.extend_from_slice(&id.to_le_bytes());
        }
        // client names: numbering 3, two entries
        b.push(3);
        b.push(2);
        b.extend_from_slice(&178u32.to_le_bytes());
        pstring8(&mut b, "PrismaticTaper");
        b.extend_from_slice(&1u32.to_le_bytes());
        pstring8(&mut b, "LeadScarab");
        // empty server sections
        b.extend_from_slice(&[1, 0, 1, 0]);
        b
    }

    #[test]
    fn maps_both_ways() {
        let m = DualDidMapper::parse(DualDidMapper::SPELL_COMPONENTS, &sample()).unwrap();
        assert_eq!(m.client_ids, vec![(0, 0), (1, 691), (178, 8357)]);
        assert_eq!(m.component_wcid(1), Some(691));
        assert_eq!(m.component_wcid(178), Some(8357));
        assert_eq!(m.component_wcid(2), None);
        assert_eq!(m.component_of_wcid(691), Some(1));
        assert_eq!(m.component_of_wcid(8357), Some(178));
        assert_eq!(m.component_of_wcid(0), Some(0));
        assert_eq!(m.component_of_wcid(700), None);
        assert_eq!(m.name_of(178), Some("PrismaticTaper"));
        assert_eq!(m.name_of(1), Some("LeadScarab"));
        assert_eq!(m.name_of(0), None);
        assert!(m.server_ids.is_empty() && m.server_names.is_empty());
    }

    #[test]
    fn packed_counts_and_trailing_bytes() {
        // A count of 164 takes the two-byte packed form.
        let mut b = Vec::new();
        b.extend_from_slice(&DualDidMapper::SPELL_COMPONENTS.to_le_bytes());
        b.extend_from_slice(&[3, 0x80, 164]);
        for i in 0..164u32 {
            b.extend_from_slice(&i.to_le_bytes());
            b.extend_from_slice(&(1000 + i).to_le_bytes());
        }
        b.extend_from_slice(&[3, 0, 1, 0, 1, 0]);
        let m = DualDidMapper::parse(DualDidMapper::SPELL_COMPONENTS, &b).unwrap();
        assert_eq!(m.client_ids.len(), 164);
        assert_eq!(m.component_wcid(163), Some(1163));
        b.push(0);
        assert!(DualDidMapper::parse(DualDidMapper::SPELL_COMPONENTS, &b).is_err());
        assert!(DualDidMapper::parse(0x2700_0001, &b).is_err());
    }
}
