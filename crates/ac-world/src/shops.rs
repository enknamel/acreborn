//! What every shop in Dereth sells and buys.
//!
//! [`crate::landmarks`] says where the vendors stand. This says what
//! they are for, which is the difference between walking to the nearest
//! shop and walking to one that has what you came for. A mage out of
//! Prismatic Tapers is not helped by the blacksmith next door, and an
//! archer out of quarrels is not helped by the archmage; both are
//! helped by knowing, before setting off, which door to knock on.
//!
//! Like the landmarks these are server-side objects and so are not in
//! the client's own data files; `data/vendors.csv` and
//! `data/vendor_buys.csv` are copies of the lists (see
//! `reference/scripts/data/vendors.sh` and `vendor_buys.sh`).
//!
//! A vendor's stock is endless in Asheron's Call: the rows say what
//! kinds of thing it sells, not how many are left.

use glam::{Vec2, Vec3};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One thing a vendor has for sale.
#[derive(Clone, Debug, PartialEq)]
pub struct Ware {
    pub wcid: u32,
    pub name: String,
    /// `crate::item_type` bits.
    pub item_type: u32,
    /// The item's base worth. What the vendor charges is this times the
    /// shop's [`Shop::sell_rate`].
    pub value: u32,
}

/// A shop: one vendor, where it stands, and what it deals in.
#[derive(Clone, Debug, PartialEq)]
pub struct Shop {
    pub wcid: u32,
    pub name: String,
    pub cell: u32,
    /// World position.
    pub at: Vec3,
    /// What it has for sale, by name.
    pub sells: Vec<Ware>,
    /// The kinds of thing it will buy (`crate::item_type` bits); 0 buys
    /// nothing.
    pub buys: u32,
    /// It refuses anything worth less than this, or more than `max_value`
    /// when that is not 0.
    pub min_value: u32,
    pub max_value: u32,
    /// It pays `value * buy_rate` and charges `value * sell_rate`.
    pub buy_rate: f32,
    pub sell_rate: f32,
}

impl Shop {
    pub fn xy(&self) -> Vec2 {
        self.at.truncate()
    }

    /// Whether it stands outdoors, so a walk can reach it.
    pub fn outdoors(&self) -> bool {
        self.cell & 0xFFFF < 0x100
    }

    /// What it has whose name contains `needle`, case-insensitively.
    pub fn stocks(&self, needle: &str) -> Option<&Ware> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        self.sells
            .iter()
            .find(|w| w.name.to_lowercase().contains(&needle))
    }

    /// Whether it sells the item with this weenie class.
    pub fn stocks_wcid(&self, wcid: u32) -> bool {
        self.sells.iter().any(|w| w.wcid == wcid)
    }

    /// Whether it sells anything of these kinds.
    pub fn stocks_type(&self, types: u32) -> bool {
        self.sells.iter().any(|w| w.item_type & types != 0)
    }

    /// What it would pay for an item worth `value` of kind `item_type`,
    /// or `None` when it will not take it. A shop refuses whole kinds
    /// of thing, and refuses what is too cheap or too dear even within
    /// a kind it deals in.
    pub fn pays_for(&self, item_type: u32, value: u32) -> Option<u32> {
        if self.buys & item_type == 0 || value < self.min_value {
            return None;
        }
        if self.max_value > 0 && value > self.max_value {
            return None;
        }
        Some((value as f32 * self.buy_rate).round() as u32)
    }

    /// What it charges for one of `ware`.
    pub fn charges_for(&self, ware: &Ware) -> u32 {
        (ware.value as f32 * self.sell_rate).round().max(1.0) as u32
    }
}

const SELLS: &str = include_str!("../data/vendors.csv");
const BUYS: &str = include_str!("../data/vendor_buys.csv");

fn rows(text: &str, fields: usize) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split(',').collect::<Vec<&str>>())
        .filter(move |f| f.len() == fields)
}

fn parse(sells: &str, buys: &str) -> Vec<Shop> {
    let hex = |s: &str| u32::from_str_radix(s.trim(), 16).ok();
    let num = |s: &str| s.trim().parse::<f32>().ok();
    let int = |s: &str| s.trim().parse::<u32>().ok();

    // The prices and the location come from the buys file, which has
    // one row per shop *placement*; the wares are added to it.
    //
    // Keyed on the vendor and where it stands, not the vendor alone.
    // The same shopkeeper is placed in many towns -- the Academy
    // Shopkeep stands in twenty-six of them -- and keying on the weenie
    // would keep one of each and throw the rest away, sending a mage in
    // Holtburg to the archmage of the same name in Yaraq.
    let mut shops: BTreeMap<(u32, u32), Shop> = BTreeMap::new();
    for f in rows(buys, 11) {
        let (Some(wcid), Some(cell)) = (int(f[0]), hex(f[2])) else {
            continue;
        };
        let (Some(x), Some(y), Some(z)) = (num(f[3]), num(f[4]), num(f[5])) else {
            continue;
        };
        let Some(buys) = hex(f[6]) else { continue };
        shops.insert(
            (wcid, cell),
            Shop {
                wcid,
                name: f[1].to_string(),
                cell,
                at: crate::landblock_origin(cell) + Vec3::new(x, y, z),
                sells: Vec::new(),
                buys,
                min_value: int(f[7]).unwrap_or(0),
                max_value: int(f[8]).unwrap_or(0),
                buy_rate: num(f[9]).unwrap_or(1.0),
                sell_rate: num(f[10]).unwrap_or(1.0),
            },
        );
    }
    for f in rows(sells, 10) {
        let (Some(vendor), Some(cell), Some(wcid)) = (int(f[0]), hex(f[2]), int(f[6])) else {
            continue;
        };
        let Some(item_type) = hex(f[8]) else { continue };
        let Some(shop) = shops.get_mut(&(vendor, cell)) else {
            continue;
        };
        shop.sells.push(Ware {
            wcid,
            name: f[7].to_string(),
            item_type,
            value: int(f[9]).unwrap_or(0),
        });
    }
    shops.into_values().collect()
}

/// Every shop, parsed on first use.
pub fn all() -> &'static [Shop] {
    static SHOPS: OnceLock<Vec<Shop>> = OnceLock::new();
    SHOPS.get_or_init(|| parse(SELLS, BUYS))
}

/// What a vendor charges for a trade note, as a multiple of its face
/// value. The server fixes this at 1.15 whatever the shop's own sell
/// rate is, and pays back only face value, so a purse turned into notes
/// and back is thirteen percent lighter. Notes are still how a fortune
/// is carried: pyreals stack 25,000 to a slot, and a 250,000 note is
/// one item.
pub const NOTE_MARKUP: f32 = 1.15;

/// What a note of `face` value costs to buy.
pub fn note_price(face: u32) -> u32 {
    ((NOTE_MARKUP * face as f32) - 0.1).ceil().max(1.0) as u32
}

/// Every trade note in the world, largest face value first.
pub fn trade_notes() -> &'static [Ware] {
    static NOTES: OnceLock<Vec<Ware>> = OnceLock::new();
    NOTES.get_or_init(|| {
        let mut seen: BTreeMap<u32, Ware> = BTreeMap::new();
        for shop in all() {
            for w in &shop.sells {
                if w.item_type & crate::item_type::PROMISSORY_NOTE != 0
                    && w.value > 0
                    && w.name.starts_with("Trade Note")
                {
                    seen.entry(w.wcid).or_insert_with(|| w.clone());
                }
            }
        }
        let mut v: Vec<Ware> = seen.into_values().collect();
        v.sort_by_key(|w| std::cmp::Reverse(w.value));
        v
    })
}

/// How to turn `spare` pyreals into trade notes: the fewest notes that
/// carry the most of it, largest face value first.
///
/// Greedy works here because the faces are the usual money-like set
/// (100, 500, 1,000 and up), where each is a whole multiple of the ones
/// below it often enough that no smaller-first arrangement carries
/// more. Whatever is left over stays as coin.
pub fn notes_for(spare: u32, notes: &[Ware]) -> Vec<(&Ware, u32)> {
    let mut left = spare;
    let mut out = Vec::new();
    for note in notes {
        let price = note_price(note.value);
        if price == 0 || price > left {
            continue;
        }
        let how_many = left / price;
        if how_many > 0 {
            out.push((note, how_many));
            left -= how_many * price;
        }
    }
    out
}

/// The shops selling something whose name contains `needle`, nearest
/// `from` first.
///
/// This is what turns "I am out of Prismatic Tapers" into a door to
/// knock on, rather than a walk to whatever shop happens to be closest.
pub fn selling(needle: &str, from: Vec2) -> Vec<&'static Shop> {
    let mut v: Vec<&Shop> = all()
        .iter()
        .filter(|s| s.stocks(needle).is_some())
        .collect();
    v.sort_by(|a, b| a.xy().distance(from).total_cmp(&b.xy().distance(from)));
    v
}

/// The shops selling this exact item, nearest first.
pub fn selling_wcid(wcid: u32, from: Vec2) -> Vec<&'static Shop> {
    let mut v: Vec<&Shop> = all().iter().filter(|s| s.stocks_wcid(wcid)).collect();
    v.sort_by(|a, b| a.xy().distance(from).total_cmp(&b.xy().distance(from)));
    v
}

/// The shops that would buy an item of this kind and worth, nearest
/// first. Loot is worth carrying to a shop that takes it and no
/// further.
pub fn buying(item_type: u32, value: u32, from: Vec2) -> Vec<&'static Shop> {
    let mut v: Vec<&Shop> = all()
        .iter()
        .filter(|s| s.pays_for(item_type, value).is_some())
        .collect();
    v.sort_by(|a, b| a.xy().distance(from).total_cmp(&b.xy().distance(from)));
    v
}

/// The shop nearest `from` that has everything in `wanted` (by name),
/// or the one that has the most of it when none has it all.
///
/// A trip that fills the whole list at one counter is worth a longer
/// walk than one that fills half of it nearby, so the count comes
/// before the distance.
pub fn best_for(wanted: &[String], from: Vec2) -> Option<&'static Shop> {
    if wanted.is_empty() {
        return None;
    }
    all()
        .iter()
        .map(|s| {
            let has = wanted.iter().filter(|w| s.stocks(w).is_some()).count();
            (s, has)
        })
        .filter(|(_, has)| *has > 0)
        .min_by(|(a, ha), (b, hb)| {
            hb.cmp(ha)
                .then_with(|| a.xy().distance(from).total_cmp(&b.xy().distance(from)))
        })
        .map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holtburg() -> Vec2 {
        crate::towns::find("Holtburg").unwrap().world_xy()
    }

    #[test]
    fn the_shops_of_dereth_read() {
        let all = all();
        // One row per placement, not per shopkeeper: the same
        // shopkeeper stands in many towns.
        assert_eq!(all.len(), 1_043, "shops");
        let names: std::collections::BTreeSet<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.len() < all.len(),
            "{} names for {} shops",
            names.len(),
            all.len()
        );
        let wares: usize = all.iter().map(|s| s.sells.len()).sum();
        assert!(wares > 40_000, "{wares} wares");
        // Every shop has a place in the world and a name.
        assert!(all.iter().all(|s| !s.name.is_empty()));
        assert!(all.iter().all(|s| s.cell != 0));
    }

    #[test]
    fn a_shopkeeper_who_works_in_many_towns_is_in_all_of_them() {
        // The Academy Shopkeep stands in twenty-six places. Keeping one
        // of each would send a character across the world to a counter
        // that is also next door.
        let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
        for shop in all() {
            *by_name.entry(shop.name.as_str()).or_default() += 1;
        }
        let most = by_name.values().max().copied().unwrap_or(0);
        assert!(most > 10, "no shopkeeper is placed more than {most} times");
        // Every placement has its own spot in the world.
        let mut spots: Vec<(u32, u32)> = all().iter().map(|s| (s.wcid, s.cell)).collect();
        let before = spots.len();
        spots.sort_unstable();
        spots.dedup();
        assert_eq!(spots.len(), before, "two shops share a wcid and a cell");
    }

    #[test]
    fn a_mage_is_sent_to_an_archmage_and_an_archer_to_a_fletcher() {
        // The whole point of the table: what you are short of decides
        // which door you knock on.
        let tapers = selling("Prismatic Taper", holtburg());
        assert!(!tapers.is_empty(), "nobody sells tapers");
        let arrows = selling("Arrowhead", holtburg());
        assert!(!arrows.is_empty(), "nobody sells arrowheads");
        // And they are not the same shops.
        let taper_names: Vec<&str> = tapers.iter().map(|s| s.name.as_str()).collect();
        assert!(
            arrows
                .iter()
                .any(|s| !taper_names.contains(&s.name.as_str())),
            "every fletcher also sells tapers"
        );
    }

    #[test]
    fn the_nearest_shop_that_has_it_comes_first() {
        let from = holtburg();
        let found = selling("Healing Kit", from);
        assert!(found.len() > 1, "only {} shops sell kits", found.len());
        for pair in found.windows(2) {
            assert!(
                pair[0].xy().distance(from) <= pair[1].xy().distance(from),
                "{} came before {}",
                pair[0].name,
                pair[1].name
            );
        }
    }

    #[test]
    fn one_counter_that_fills_the_list_beats_a_nearer_one_that_does_not() {
        let wanted = vec!["Prismatic Taper".to_string(), "Lead Scarab".to_string()];
        let shop = best_for(&wanted, holtburg()).expect("a shop");
        assert_eq!(
            wanted.iter().filter(|w| shop.stocks(w).is_some()).count(),
            2,
            "{} does not stock both",
            shop.name
        );
        assert!(best_for(&[], holtburg()).is_none());
    }

    #[test]
    fn a_shop_refuses_what_it_does_not_deal_in() {
        let armour = crate::item_type::ARMOR;
        let food = crate::item_type::FOOD;
        let shop = Shop {
            wcid: 1,
            name: "Test".into(),
            cell: 0xA9B40019,
            at: Vec3::ZERO,
            sells: Vec::new(),
            buys: armour,
            min_value: 10,
            max_value: 1000,
            buy_rate: 0.5,
            sell_rate: 2.0,
        };
        // A kind it does not take.
        assert_eq!(shop.pays_for(food, 100), None);
        // Too cheap, and too dear.
        assert_eq!(shop.pays_for(armour, 5), None);
        assert_eq!(shop.pays_for(armour, 5_000), None);
        // And one it takes, at half.
        assert_eq!(shop.pays_for(armour, 100), Some(50));
        // No maximum means no upper limit.
        let open = Shop {
            max_value: 0,
            ..shop.clone()
        };
        assert_eq!(open.pays_for(armour, 5_000), Some(2_500));
    }

    #[test]
    fn a_vendor_charges_more_than_a_thing_is_worth() {
        let ware = Ware {
            wcid: 1,
            name: "Prismatic Taper".into(),
            item_type: crate::item_type::SPELL_COMPONENTS,
            value: 25,
        };
        let shop = &all()[0];
        assert!(shop.sell_rate >= 1.0, "{} undercuts value", shop.name);
        assert!(shop.charges_for(&ware) >= ware.value);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let buys = "# c\n\nnope\n7,Sam,A9B40019,1.0,2.0,3.0,4,0,1000,0.5,2.0\n";
        let sells = "# c\n7,Sam,A9B40019,1.0,2.0,3.0,99,Nail,4,10\nrubbish,x\n";
        let v = parse(sells, buys);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Sam");
        assert_eq!(v[0].sells.len(), 1);
        assert_eq!(v[0].sells[0].name, "Nail");
        assert!(v[0].outdoors());
        // A ware whose vendor is not in the buys file is dropped rather
        // than inventing a shop with no prices and no place.
        let orphan = parse("9,Nobody,A9B40019,1.0,2.0,3.0,99,Nail,4,10\n", buys);
        assert_eq!(orphan.len(), 1);
        assert!(orphan[0].sells.is_empty());
    }
    #[test]
    fn a_note_costs_more_than_it_is_worth() {
        // The server charges 1.15 times face however cheap the shop is.
        assert_eq!(note_price(100), 115);
        assert_eq!(note_price(250_000), 287_500);
        // Never free, however small.
        assert!(note_price(1) >= 1);
    }

    #[test]
    fn the_world_really_sells_trade_notes() {
        let notes = trade_notes();
        assert!(notes.len() >= 10, "only {} notes", notes.len());
        // Largest first, and the top one is the 250,000.
        assert_eq!(notes[0].value, 250_000);
        for pair in notes.windows(2) {
            assert!(pair[0].value >= pair[1].value);
        }
        assert!(notes
            .iter()
            .all(|n| n.item_type & crate::item_type::PROMISSORY_NOTE != 0));
    }

    #[test]
    fn a_purse_becomes_the_fewest_notes_that_carry_it() {
        let notes = trade_notes();
        // Enough for one 250,000 note and change.
        let bought = notes_for(300_000, notes);
        assert_eq!(bought[0].0.value, 250_000);
        assert_eq!(bought[0].1, 1);
        // Never spends more than it has.
        let spent: u32 = bought.iter().map(|(n, c)| note_price(n.value) * c).sum();
        assert!(spent <= 300_000, "spent {spent} of 300000");
    }

    #[test]
    fn a_purse_too_small_for_any_note_buys_none() {
        let notes = trade_notes();
        assert!(notes_for(50, notes).is_empty());
        assert!(notes_for(0, notes).is_empty());
    }

    #[test]
    fn whatever_is_carried_home_is_worth_most_of_what_went_in() {
        // The markup is the price of the pack space; it should not be
        // eating the purse. Across a wide range, the face value carried
        // stays close to what the purse could buy.
        let notes = trade_notes();
        for purse in [1_000u32, 12_345, 100_000, 999_999, 5_000_000] {
            let bought = notes_for(purse, notes);
            let spent: u32 = bought.iter().map(|(n, c)| note_price(n.value) * c).sum();
            let face: u32 = bought.iter().map(|(n, c)| n.value * c).sum();
            assert!(spent <= purse, "{purse}: spent {spent}");
            // What is left as coin plus the face carried is never worse
            // than 87% of the purse.
            let kept = (purse - spent) + face;
            assert!(
                kept as f32 >= purse as f32 * 0.86,
                "{purse}: kept only {kept}"
            );
        }
    }
}
