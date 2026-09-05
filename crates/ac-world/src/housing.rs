//! Housing: what a house sign (the slumlord) tells us when used
//! (HouseProfile 0x021D), our own house (HouseData 0x0225, the answer to
//! HouseQuery 0x021E), its guest list (UpdateHAR 0x0257, the answer to
//! RequestFullGuestList) and who may enter a house near us
//! (HouseUpdateRestrictions 0x0248). See ACE
//! `Network/Structure/House{Profile,Data,Payment,Access}.cs`.

use crate::object::Position;
use ac_net::wire::Reader;

/// One item a house costs or its rent takes: how many, how many are
/// already paid, and the item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Payment {
    pub needed: u32,
    pub paid: u32,
    pub wcid: u32,
    pub name: String,
    pub plural: String,
}

impl Payment {
    pub fn outstanding(&self) -> u32 {
        self.needed.saturating_sub(self.paid)
    }
}

/// House types (ACE `HouseType`).
pub fn kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "Cottage",
        2 => "Villa",
        3 => "Mansion",
        4 => "Apartment",
        _ => "Dwelling",
    }
}

/// What a house sign says (ACE `HouseProfile`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HouseProfile {
    /// The sign (slumlord) we used, for BuyHouse/RentHouse.
    pub slumlord: u32,
    pub dwelling_id: u32,
    /// Owner guid, 0 when for sale.
    pub owner: u32,
    pub owner_name: String,
    /// Bitmask bit 1: active; bit 2: requires a monarch.
    pub active: bool,
    pub requires_monarch: bool,
    /// -1 when there is no requirement.
    pub min_level: i32,
    pub max_level: i32,
    pub min_rank: i32,
    pub max_rank: i32,
    pub maintenance_free: bool,
    pub kind: u32,
    pub buy: Vec<Payment>,
    pub rent: Vec<Payment>,
}

/// Our own house (ACE `HouseData`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HouseData {
    pub buy_time: u32,
    pub rent_time: u32,
    pub kind: u32,
    pub maintenance_free: bool,
    pub buy: Vec<Payment>,
    pub rent: Vec<Payment>,
    pub position: Option<Position>,
}

impl HouseData {
    /// Whether this period's maintenance is fully paid.
    pub fn rent_paid(&self) -> bool {
        self.maintenance_free || self.rent.iter().all(|p| p.paid >= p.needed)
    }
}

/// One guest of our house.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guest {
    pub guid: u32,
    pub name: String,
    /// May also use the storage chests.
    pub storage: bool,
}

/// Our house's access records (ACE `HouseAccess`, "HAR").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HouseAccess {
    pub open: bool,
    pub allegiance_guests: bool,
    pub allegiance_storage: bool,
    pub monarch: u32,
    pub guests: Vec<Guest>,
    pub roommates: Vec<u32>,
}

fn read_payments(r: &mut Reader) -> Option<Vec<Payment>> {
    let n = r.u32().ok()? as usize;
    let mut v = Vec::with_capacity(n.min(16));
    for _ in 0..n {
        v.push(Payment {
            needed: r.u32().ok()?,
            paid: r.u32().ok()?,
            wcid: r.u32().ok()?,
            name: r.string16().ok()?,
            plural: r.string16().ok()?,
        });
    }
    Some(v)
}

/// A payment list on its own (UpdateRentPayment 0x0228).
pub fn read_payments_pub(r: &mut Reader) -> Option<Vec<Payment>> {
    read_payments(r)
}

/// HouseProfile event body: slumlord guid then the profile.
pub fn parse_profile(body: &[u8]) -> Option<HouseProfile> {
    let mut r = Reader::new(body);
    let slumlord = r.u32().ok()?;
    let dwelling_id = r.u32().ok()?;
    let owner = r.u32().ok()?;
    let bits = r.u32().ok()?;
    let min_level = r.i32().ok()?;
    let max_level = r.i32().ok()?;
    let min_rank = r.i32().ok()?;
    let max_rank = r.i32().ok()?;
    let maintenance_free = r.u32().ok()? != 0;
    let kind = r.u32().ok()?;
    let owner_name = r.string16().ok()?;
    let buy = read_payments(&mut r)?;
    let rent = read_payments(&mut r)?;
    Some(HouseProfile {
        slumlord,
        dwelling_id,
        owner,
        owner_name,
        active: bits & 1 != 0,
        requires_monarch: bits & 2 != 0,
        min_level,
        max_level,
        min_rank,
        max_rank,
        maintenance_free,
        kind,
        buy,
        rent,
    })
}

/// HouseData event body.
pub fn parse_data(body: &[u8]) -> Option<HouseData> {
    let mut r = Reader::new(body);
    let buy_time = r.u32().ok()?;
    let rent_time = r.u32().ok()?;
    let kind = r.u32().ok()?;
    let maintenance_free = r.u32().ok()? != 0;
    let buy = read_payments(&mut r)?;
    let rent = read_payments(&mut r)?;
    let position = Position::parse(&mut r).ok().filter(|p| p.cell != 0);
    Some(HouseData {
        buy_time,
        rent_time,
        kind,
        maintenance_free,
        buy,
        rent,
        position,
    })
}

/// UpdateHAR event body: version, bitmask, monarch, guest hash table
/// (count, buckets, then guid, storage flag, name), roommate list.
pub fn parse_access(body: &[u8]) -> Option<HouseAccess> {
    let mut r = Reader::new(body);
    let _version = r.u32().ok()?;
    let bits = r.u32().ok()?;
    let monarch = r.u32().ok()?;
    let n = r.u16().ok()? as usize;
    let _buckets = r.u16().ok()?;
    let mut guests = Vec::with_capacity(n.min(128));
    for _ in 0..n {
        let guid = r.u32().ok()?;
        let storage = r.u32().ok()? != 0;
        let name = r.string16().ok()?;
        guests.push(Guest {
            guid,
            name,
            storage,
        });
    }
    let n = r.u32().ok()? as usize;
    let mut roommates = Vec::with_capacity(n.min(16));
    for _ in 0..n {
        roommates.push(r.u32().ok()?);
    }
    Some(HouseAccess {
        open: bits & 1 != 0,
        allegiance_guests: bits & 2 != 0,
        allegiance_storage: bits & 4 != 0,
        monarch,
        guests,
        roommates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_net::wire::Writer;

    fn write_payment(w: &mut Writer, needed: u32, paid: u32, wcid: u32, name: &str) {
        w.u32(needed)
            .u32(paid)
            .u32(wcid)
            .string16(name)
            .string16("");
    }

    #[test]
    fn parses_profile_data_and_access() {
        let mut w = Writer::new();
        w.u32(0x7720_0007).u32(2001).u32(0).u32(1);
        w.i32(20).i32(-1).i32(-1).i32(-1).u32(0).u32(4).string16("");
        w.u32(2);
        write_payment(&mut w, 100_000, 0, 273, "Pyreal");
        write_payment(&mut w, 1, 0, 11710, "Writ of Refuge");
        w.u32(1);
        write_payment(&mut w, 10_000, 0, 273, "Pyreal");
        let p = parse_profile(&w.finish()).unwrap();
        assert_eq!((p.slumlord, p.dwelling_id, p.owner), (0x7720_0007, 2001, 0));
        assert!(p.active && !p.requires_monarch);
        assert_eq!((p.min_level, p.max_level), (20, -1));
        assert_eq!(kind_name(p.kind), "Apartment");
        assert_eq!(p.buy.len(), 2);
        assert_eq!(p.buy[1].name, "Writ of Refuge");
        assert_eq!(p.rent[0].outstanding(), 10_000);

        let mut w = Writer::new();
        w.u32(1_700_000_000).u32(1_700_000_100).u32(4).u32(0);
        w.u32(0);
        w.u32(1);
        write_payment(&mut w, 10_000, 10_000, 273, "Pyreal");
        w.u32(0x7200_018C).f32(90.0).f32(-138.0).f32(0.0);
        w.f32(1.0).f32(0.0).f32(0.0).f32(0.0);
        let d = parse_data(&w.finish()).unwrap();
        assert_eq!(d.kind, 4);
        assert!(d.rent_paid());
        assert_eq!(d.position.map(|p| p.cell), Some(0x7200_018C));

        let mut w = Writer::new();
        w.u32(0x1000_0002).u32(3).u32(0x5000_0002);
        w.u16(1).u16(23).u32(0x5000_0001).u32(1).string16("Reborn");
        w.u32(1).u32(0x5000_0003);
        let a = parse_access(&w.finish()).unwrap();
        assert!(a.open && a.allegiance_guests && !a.allegiance_storage);
        assert_eq!(a.guests[0].name, "Reborn");
        assert!(a.guests[0].storage);
        assert_eq!(a.roommates, vec![0x5000_0003]);
        assert!(parse_access(&[1, 2, 3]).is_none());
    }
}
