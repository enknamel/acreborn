//! Friends, titles and squelches: the social lists the server keeps
//! per character (FriendsListUpdate 0x0021, CharacterTitle 0x0029 /
//! UpdateTitle 0x002B, SetSquelchDB 0x01F4).

use ac_net::wire::Reader;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Friend {
    pub guid: u32,
    pub name: String,
    pub online: bool,
}

/// A friends update: the records and what happened to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendsUpdate {
    /// 0 the full list, 1 added, 2 removed, 4 online status changed.
    pub kind: u32,
    pub friends: Vec<Friend>,
}

pub fn parse_friends(body: &[u8]) -> Option<FriendsUpdate> {
    let mut r = Reader::new(body);
    let n = r.u32().ok()? as usize;
    let mut friends = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let guid = r.u32().ok()?;
        let online = r.u32().ok()? != 0;
        let _appear_offline = r.u32().ok()?;
        let name = r.string16().ok()?;
        for _ in 0..2 {
            let m = r.u32().ok()? as usize;
            for _ in 0..m.min(256) {
                r.u32().ok()?;
            }
        }
        friends.push(Friend { guid, name, online });
    }
    let kind = r.u32().ok()?;
    Some(FriendsUpdate { kind, friends })
}

/// The titles the character has earned and the one shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Titles {
    pub current: u32,
    pub ids: Vec<u32>,
}

impl Titles {
    /// CharacterTitle body: 1, current, count, ids.
    pub fn parse(body: &[u8]) -> Option<Titles> {
        let mut r = Reader::new(body);
        let _one = r.u32().ok()?;
        let current = r.u32().ok()?;
        let n = r.u32().ok()? as usize;
        let mut ids = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            ids.push(r.u32().ok()?);
        }
        Some(Titles { current, ids })
    }

    /// UpdateTitle body: title id, shown flag.
    pub fn apply_update(&mut self, body: &[u8]) -> bool {
        let mut r = Reader::new(body);
        let (Ok(id), Ok(shown)) = (r.u32(), r.u32()) else {
            return false;
        };
        if !self.ids.contains(&id) {
            self.ids.push(id);
        }
        if shown != 0 {
            self.current = id;
        }
        true
    }
}

/// One squelched character.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Squelch {
    pub guid: u32,
    pub name: String,
    /// SquelchMask bits (0xFFFFFFFF = everything).
    pub mask: u32,
    /// The whole account is squelched, not only this character.
    pub account: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Squelches {
    pub characters: Vec<Squelch>,
    /// Chat types squelched from everyone.
    pub global: u32,
}

/// SquelchDB body: the (always empty) account table, the character
/// table (guid -> filters list, name, account flag), the global mask.
pub fn parse_squelches(body: &[u8]) -> Option<Squelches> {
    let mut r = Reader::new(body);
    let n = r.u16().ok()? as usize;
    let _buckets = r.u16().ok()?;
    for _ in 0..n {
        r.string16().ok()?;
        r.u32().ok()?;
    }
    let n = r.u16().ok()? as usize;
    let _buckets = r.u16().ok()?;
    let mut characters = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let guid = r.u32().ok()?;
        let m = r.u32().ok()? as usize;
        let mut mask = 0;
        for _ in 0..m.min(64) {
            mask |= r.u32().ok()?;
        }
        let name = r.string16().ok()?;
        let account = r.u32().ok()? != 0;
        characters.push(Squelch {
            guid,
            name,
            mask,
            account,
        });
    }
    let global = r.u32().ok()?;
    Some(Squelches { characters, global })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_net::wire::Writer;

    #[test]
    fn parses_friends_titles_squelches() {
        let mut w = Writer::new();
        w.u32(1)
            .u32(0x5000_0001)
            .u32(1)
            .u32(0)
            .string16("Reborn")
            .u32(0)
            .u32(0)
            .u32(1);
        let f = parse_friends(&w.finish()).unwrap();
        assert_eq!(f.kind, 1);
        assert_eq!(f.friends[0].name, "Reborn");
        assert!(f.friends[0].online);

        let mut w = Writer::new();
        w.u32(1).u32(2).u32(2).u32(1).u32(2);
        let mut t = Titles::parse(&w.finish()).unwrap();
        assert_eq!((t.current, t.ids.clone()), (2, vec![1, 2]));
        let mut w = Writer::new();
        w.u32(9).u32(1);
        assert!(t.apply_update(&w.finish()));
        assert_eq!((t.current, t.ids.len()), (9, 3));

        let mut w = Writer::new();
        w.u16(0).u16(0);
        w.u16(1)
            .u16(32)
            .u32(0x5000_0001)
            .u32(2)
            .u32(0x4)
            .u32(0x8)
            .string16("Reborn")
            .u32(0);
        w.u32(0x40);
        let s = parse_squelches(&w.finish()).unwrap();
        assert_eq!(s.characters[0].mask, 0xC);
        assert_eq!(s.global, 0x40);
    }
}
