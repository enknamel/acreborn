//! The allegiance: the tree of patrons and vassals a character belongs
//! to, as the server describes it in AllegianceUpdate (0x0020: our rank,
//! then a profile) and AllegianceInfoResponse (0x027C: a player guid,
//! then that player's profile). A profile is the member counts and an
//! `AllegianceHierarchy`: officers, titles, broadcast counters, motd,
//! chat room, bind point, name, lock state, then the monarch's record and
//! a list of (parent guid, record) for the patron, the player themselves
//! and their direct vassals; the whole tree is never sent. See ACE
//! `Network/Structure/Allegiance{Profile,Hierarchy,Data}.cs`.

use crate::object::Position;
use ac_net::wire::Reader;

/// `AllegianceIndex` bits in a member record.
pub mod index {
    pub const LOGGED_IN: u32 = 0x1;
    pub const UPDATE: u32 = 0x2;
    pub const HAS_ALLEGIANCE_AGE: u32 = 0x4;
    pub const HAS_PACKED_LEVEL: u32 = 0x8;
    pub const MAY_PASSUP_EXPERIENCE: u32 = 0x10;
}

/// One member record (ACE `AllegianceData`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Member {
    pub guid: u32,
    pub name: String,
    /// Allegiance rank 1..=10 (from the number of vassals below).
    pub rank: u32,
    pub level: u32,
    pub loyalty: u32,
    pub leadership: u32,
    pub online: bool,
    /// Whether XP this member earns passes up to their patron.
    pub may_passup: bool,
    /// XP received from vassals and not yet collected (patron side).
    pub xp_cached: u64,
    /// XP this member has generated for their patron.
    pub xp_tithed: u64,
    pub gender: u8,
    pub heritage: u8,
    /// Seconds in the allegiance, when the record carries an age.
    pub allegiance_age: u32,
}

/// The part of the tree the server sends us.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allegiance {
    /// Our own rank, 0 outside an allegiance.
    pub rank: u32,
    /// Members including the monarch.
    pub total_members: u32,
    /// Everyone below us.
    pub total_vassals: u32,
    pub name: String,
    pub motd: String,
    pub motd_set_by: String,
    /// Turbine chat room id for allegiance chat.
    pub chat_room: u32,
    /// The allegiance's bind point (recall target), if set.
    pub bind_point: Option<Position>,
    pub locked: bool,
    /// (guid, officer level 1 speaker, 2 seneschal, 3 castellan).
    pub officers: Vec<(u32, u32)>,
    pub officer_titles: Vec<String>,
    pub monarch: Option<Member>,
    pub patron: Option<Member>,
    /// The player the profile is about (us, or the one asked about).
    pub me: Option<Member>,
    pub vassals: Vec<Member>,
}

impl Allegiance {
    /// Everyone in the profile, monarch first, each once (the monarch
    /// may also be the patron or us).
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        let monarch = self.monarch.as_ref().map(|m| m.guid);
        self.monarch
            .iter()
            .chain(self.patron.iter().filter(move |m| Some(m.guid) != monarch))
            .chain(self.me.iter().filter(move |m| Some(m.guid) != monarch))
            .chain(self.vassals.iter())
    }

    /// Whether the profile's subject is the monarch.
    pub fn is_monarch(&self) -> bool {
        matches!((&self.me, &self.monarch), (Some(a), Some(b)) if a.guid == b.guid)
    }

    /// In an allegiance at all (ACE counts one only from two members).
    pub fn is_member(&self) -> bool {
        self.total_members > 1
    }

    pub fn member(&self, guid: u32) -> Option<&Member> {
        self.members().find(|m| m.guid == guid)
    }

    pub fn member_mut(&mut self, guid: u32) -> Option<&mut Member> {
        self.monarch
            .iter_mut()
            .chain(self.patron.iter_mut())
            .chain(self.me.iter_mut())
            .chain(self.vassals.iter_mut())
            .find(|m| m.guid == guid)
    }

    /// The officer level of a member, 0 when none.
    pub fn officer_level(&self, guid: u32) -> u32 {
        self.officers
            .iter()
            .find(|(g, _)| *g == guid)
            .map(|(_, l)| *l)
            .unwrap_or(0)
    }
}

fn read_member(r: &mut Reader) -> Option<Member> {
    let guid = r.u32().ok()?;
    let xp_cached = r.u32().ok()? as u64;
    let xp_tithed = r.u32().ok()? as u64;
    let bits = r.u32().ok()?;
    let gender = r.u8().ok()?;
    let heritage = r.u8().ok()?;
    let rank = r.u16().ok()? as u32;
    let level = if bits & index::HAS_PACKED_LEVEL != 0 {
        r.u32().ok()?
    } else {
        0
    };
    let loyalty = r.u16().ok()? as u32;
    let leadership = r.u16().ok()? as u32;
    let allegiance_age = if bits & index::HAS_ALLEGIANCE_AGE != 0 {
        let _time_online = r.u32().ok()?;
        r.u32().ok()?
    } else {
        let _time_online = r.u64().ok()?;
        0
    };
    let name = r.string16().ok()?;
    Some(Member {
        guid,
        name,
        rank,
        level,
        loyalty,
        leadership,
        online: bits & index::LOGGED_IN != 0,
        may_passup: bits & index::MAY_PASSUP_EXPERIENCE != 0,
        xp_cached,
        xp_tithed,
        gender,
        heritage,
        allegiance_age,
    })
}

fn read_position(r: &mut Reader) -> Option<Position> {
    Position::parse(r).ok()
}

/// Parse an `AllegianceProfile` for the player `about` (our guid for
/// AllegianceUpdate, the asked-about guid for AllegianceInfoResponse).
/// `rank` is the leading rank of an AllegianceUpdate, 0 otherwise.
/// Returns `None` when the profile is malformed; an empty profile (no
/// allegiance) parses to a value with no members.
pub fn parse_profile(r: &mut Reader, about: u32, rank: u32) -> Option<Allegiance> {
    let total_members = r.u32().ok()?;
    let total_vassals = r.u32().ok()?;
    let record_count = r.u16().ok()? as usize;
    let _old_version = r.u16().ok()?;
    // Officers: PackableHashTable header (count, buckets) then pairs.
    let n = r.u16().ok()? as usize;
    let _buckets = r.u16().ok()?;
    let mut officers = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        officers.push((r.u32().ok()?, r.u32().ok()?));
    }
    let n = r.u32().ok()? as usize;
    let mut officer_titles = Vec::with_capacity(n.min(8));
    for _ in 0..n {
        officer_titles.push(r.string16().ok()?);
    }
    let _monarch_broadcast_time = r.u32().ok()?;
    let _monarch_broadcasts_today = r.u32().ok()?;
    let _spokes_broadcast_time = r.u32().ok()?;
    let _spokes_broadcasts_today = r.u32().ok()?;
    let motd = r.string16().ok()?;
    let motd_set_by = r.string16().ok()?;
    let chat_room = r.u32().ok()?;
    let bind_point = read_position(r).filter(|p| p.cell != 0);
    let name = r.string16().ok()?;
    let _name_last_set_time = r.u32().ok()?;
    let locked = r.u32().ok()? != 0;
    let _approved_vassal = r.u32().ok()?;
    let mut a = Allegiance {
        rank,
        total_members,
        total_vassals,
        name,
        motd,
        motd_set_by,
        chat_room,
        bind_point,
        locked,
        officers,
        officer_titles,
        ..Default::default()
    };
    if record_count == 0 {
        return Some(a);
    }
    let monarch = read_member(r)?;
    let monarch_guid = monarch.guid;
    if monarch_guid == about {
        a.me = Some(monarch.clone());
    }
    a.monarch = Some(monarch);
    // The remaining records are (parent guid, member): the patron (its
    // parent written as the monarch), ourselves (parent: the patron) and
    // our vassals (parent: us). Sort them out by guid rather than trust
    // the order.
    for _ in 1..record_count {
        let parent = r.u32().ok()?;
        let m = read_member(r)?;
        if m.guid == about {
            a.me = Some(m);
        } else if parent == about {
            a.vassals.push(m);
        } else if m.guid != monarch_guid {
            a.patron = Some(m);
        }
    }
    // No patron record means the patron is the monarch (the server only
    // lists a patron who is not).
    if a.patron.is_none()
        && a.me
            .as_ref()
            .map(|m| m.guid != monarch_guid)
            .unwrap_or(false)
    {
        a.patron = a.monarch.clone();
    }
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_net::wire::Writer;

    fn write_member(w: &mut Writer, guid: u32, name: &str, rank: u16, level: u32, online: bool) {
        let mut bits = index::HAS_ALLEGIANCE_AGE | index::HAS_PACKED_LEVEL;
        if online {
            bits |= index::LOGGED_IN;
        }
        w.u32(guid)
            .u32(1234)
            .u32(77)
            .u32(bits)
            .u8(1)
            .u8(2)
            .u16(rank);
        w.u32(level);
        w.u16(150).u16(90);
        w.u32(3600).u32(86400);
        w.string16(name);
    }

    fn write_header(w: &mut Writer, members: u32, vassals: u32, records: u16, name: &str) {
        w.u32(members).u32(vassals).u16(records).u16(0x000B);
        w.u16(1).u16(256).u32(0x50000AAA).u32(2); // one seneschal
        w.u32(0); // titles
        w.u32(0).u32(0).u32(0).u32(0);
        w.string16("Be excellent").string16("King");
        w.u32(0x3300);
        w.u32(0xA9B4_0019)
            .f32(84.0)
            .f32(7.1)
            .f32(94.0)
            .f32(1.0)
            .f32(0.0)
            .f32(0.0)
            .f32(0.0);
        w.string16(name).u32(0).u32(1).u32(0);
    }

    #[test]
    fn parses_a_vassal_profile() {
        // Monarch K (0xAAA) -> patron P (0xBBB) -> me M (0xCCC) -> vassals V1, V2.
        let mut w = Writer::new();
        write_header(&mut w, 5, 2, 5, "The Realm");
        write_member(&mut w, 0x50000AAA, "King", 4, 50, true);
        w.u32(0x50000AAA);
        write_member(&mut w, 0x50000BBB, "Patron", 3, 30, false);
        w.u32(0x50000BBB);
        write_member(&mut w, 0x50000CCC, "Me", 2, 12, true);
        w.u32(0x50000CCC);
        write_member(&mut w, 0x50000DD1, "Vassal One", 1, 5, true);
        w.u32(0x50000CCC);
        write_member(&mut w, 0x50000DD2, "Vassal Two", 1, 7, false);
        let body = w.finish();
        let a = parse_profile(&mut Reader::new(&body), 0x50000CCC, 2).unwrap();
        assert_eq!(a.rank, 2);
        assert_eq!((a.total_members, a.total_vassals), (5, 2));
        assert_eq!(a.name, "The Realm");
        assert_eq!(a.motd, "Be excellent");
        assert_eq!(a.chat_room, 0x3300);
        assert_eq!(a.bind_point.map(|p| p.cell), Some(0xA9B4_0019));
        assert!(a.locked);
        assert_eq!(a.officers, vec![(0x50000AAA, 2)]);
        assert_eq!(a.officer_level(0x50000AAA), 2);
        assert_eq!(a.monarch.as_ref().map(|m| m.name.as_str()), Some("King"));
        assert_eq!(a.patron.as_ref().map(|m| m.name.as_str()), Some("Patron"));
        assert_eq!(a.me.as_ref().map(|m| (m.level, m.online)), Some((12, true)));
        let v: Vec<_> = a.vassals.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(v, ["Vassal One", "Vassal Two"]);
        assert_eq!(a.member(0x50000DD2).map(|m| m.loyalty), Some(150));
        assert_eq!(a.members().count(), 5);
        let m = a.me.as_ref().unwrap();
        assert_eq!(
            (m.xp_cached, m.xp_tithed, m.allegiance_age),
            (1234, 77, 86400)
        );
        assert!(!m.may_passup);
    }

    #[test]
    fn parses_a_monarch_and_an_empty_profile() {
        let mut w = Writer::new();
        write_header(&mut w, 2, 1, 2, "");
        write_member(&mut w, 0x50000AAA, "King", 1, 50, true);
        w.u32(0x50000AAA);
        write_member(&mut w, 0x50000BBB, "Only Vassal", 0, 3, false);
        let body = w.finish();
        let a = parse_profile(&mut Reader::new(&body), 0x50000AAA, 1).unwrap();
        assert_eq!(a.me.as_ref().map(|m| m.name.as_str()), Some("King"));
        assert!(a.patron.is_none());
        assert_eq!(a.vassals.len(), 1);
        assert!(a.is_monarch() && a.is_member());
        assert_eq!(a.members().count(), 2);
        // The vassal's own profile: no patron record, so the monarch is it.
        let mut w = Writer::new();
        write_header(&mut w, 2, 0, 2, "");
        write_member(&mut w, 0x50000AAA, "King", 1, 50, true);
        w.u32(0x50000AAA);
        write_member(&mut w, 0x50000BBB, "Only Vassal", 0, 3, true);
        let body = w.finish();
        let a = parse_profile(&mut Reader::new(&body), 0x50000BBB, 1).unwrap();
        assert_eq!(a.patron.as_ref().map(|m| m.name.as_str()), Some("King"));
        assert_eq!(a.me.as_ref().map(|m| m.name.as_str()), Some("Only Vassal"));
        assert!(!a.is_monarch());
        assert_eq!(a.members().count(), 2);

        let mut w = Writer::new();
        w.u32(0).u32(0).u16(0).u16(0x000B).u16(0).u16(256).u32(0);
        w.u32(0)
            .u32(0)
            .u32(0)
            .u32(0)
            .string16("")
            .string16("")
            .u32(0);
        w.u32(0)
            .f32(0.0)
            .f32(0.0)
            .f32(0.0)
            .f32(1.0)
            .f32(0.0)
            .f32(0.0)
            .f32(0.0);
        w.string16("").u32(0).u32(0).u32(0);
        let body = w.finish();
        let a = parse_profile(&mut Reader::new(&body), 0x50000AAA, 0).unwrap();
        assert_eq!(a.total_members, 0);
        assert!(a.monarch.is_none() && a.bind_point.is_none());
        assert!(parse_profile(&mut Reader::new(&body[..20]), 1, 0).is_none());
    }
}
