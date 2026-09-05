//! SpellComponentTable (0x0E00000F): one record per spell component id
//! (scarabs, herbs, powders, potions, talismans, tapers) with its icon,
//! casting gesture and the spell word it contributes. Layout cross-checked
//! against ACE's `SpellComponentsTable`/`SpellComponentBase`.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

/// [`SpellComponent::kind`] values.
pub mod component_type {
    /// Leads a formula; determines the windup gesture and the spell level.
    pub const SCARAB: u32 = 1;
    pub const HERB: u32 = 2;
    pub const POWDER: u32 = 3;
    pub const POTION: u32 = 4;
    /// Last in a formula; determines the cast gesture.
    pub const TALISMAN: u32 = 5;
    pub const TAPER: u32 = 6;
    /// Foci substitutes ("pea" components).
    pub const PEA: u32 = 7;

    pub fn name(kind: u32) -> &'static str {
        match kind {
            SCARAB => "Scarab",
            HERB => "Herb",
            POWDER => "Powder",
            POTION => "Potion",
            TALISMAN => "Talisman",
            TAPER => "Taper",
            PEA => "Pea",
            _ => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpellComponent {
    pub name: String,
    pub category: u32,
    /// RenderSurface (0x06) id.
    pub icon_id: u32,
    /// See [`component_type`].
    pub kind: u32,
    /// MotionCommand played when this component is consumed.
    pub gesture: u32,
    /// Seconds the gesture takes.
    pub time: f32,
    /// The spell word this component contributes to the incantation.
    pub text: String,
    /// Component burn multiplier ("component destruction modifier").
    pub cdm: f32,
}

impl SpellComponent {
    fn parse(r: &mut Reader) -> Result<Self> {
        let name = r.obfuscated_string()?;
        r.align4()?;
        let category = r.u32()?;
        let icon_id = r.u32()?;
        let kind = r.u32()?;
        let gesture = r.u32()?;
        let time = r.f32()?;
        let text = r.obfuscated_string()?;
        r.align4()?;
        let cdm = r.f32()?;
        Ok(SpellComponent {
            name,
            category,
            icon_id,
            kind,
            gesture,
            time,
            text,
            cdm,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpellComponentTable {
    pub id: u32,
    /// Sorted by component id.
    pub components: Vec<(u32, SpellComponent)>,
}

impl SpellComponentTable {
    pub const ID: u32 = 0x0E00_000F;

    pub fn parse(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = expect_id(&mut r, id)?;
        // u16 count then a u16 the client skips for alignment (the bucket
        // count of the packed hash table).
        let mut components = r.packed_hash_table(|r| r.u32(), SpellComponent::parse)?;
        r.finish()?;
        components.sort_by_key(|(k, _)| *k);
        Ok(SpellComponentTable { id, components })
    }

    pub fn get(&self, component_id: u32) -> Option<&SpellComponent> {
        self.components
            .binary_search_by_key(&component_id, |(k, _)| *k)
            .ok()
            .map(|i| &self.components[i].1)
    }

    /// The component whose name equals `name`, else the first whose name
    /// starts with it (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Option<u32> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.components
            .iter()
            .find(|(_, c)| c.name.to_lowercase() == want)
            .or_else(|| {
                self.components
                    .iter()
                    .find(|(_, c)| c.name.to_lowercase().starts_with(&want))
            })
            .map(|(id, _)| *id)
    }

    /// The incantation for a formula: the herb's word, then the powder's
    /// and potion's words joined and capitalised as one ("Zojak Quafeth").
    pub fn spell_words(&self, formula: impl IntoIterator<Item = u32>) -> String {
        let mut herb = "";
        let mut powder = "";
        let mut potion = "";
        for id in formula {
            let Some(c) = self.get(id) else { continue };
            match c.kind {
                component_type::HERB => herb = &c.text,
                component_type::POWDER => powder = &c.text,
                component_type::POTION => potion = &c.text,
                _ => {}
            }
        }
        let mut second: String = format!("{powder}{}", potion.to_lowercase());
        if let Some(first) = second.get(..1) {
            second = first.to_uppercase() + &second[1..];
        }
        format!("{herb} {second}").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_words_join_and_capitalise() {
        let comp = |kind, text: &str| SpellComponent {
            kind,
            text: text.into(),
            ..Default::default()
        };
        let t = SpellComponentTable {
            id: SpellComponentTable::ID,
            components: vec![
                (1, comp(component_type::SCARAB, "")),
                (10, comp(component_type::HERB, "Zojak")),
                (20, comp(component_type::POWDER, "Qua")),
                (30, comp(component_type::POTION, "Feth")),
                (40, comp(component_type::TALISMAN, "")),
            ],
        };
        assert_eq!(t.spell_words([1, 10, 20, 30, 40]), "Zojak Quafeth");
        assert_eq!(t.spell_words([1, 10, 30]), "Zojak Feth");
        assert_eq!(t.spell_words([1, 40]), "");
        assert_eq!(t.spell_words([99]), "");
    }
}
