//! Item materials (ACE `MaterialType`): what loot is made of, and so
//! what salvaging it yields and which tinkering it can take.

/// Names by material id, `"?"` for unknown ones.
pub fn name(id: u32) -> &'static str {
    match id {
        0x01 => "Ceramic",
        0x02 => "Porcelain",
        0x03 => "Cloth",
        0x04 => "Linen",
        0x05 => "Satin",
        0x06 => "Silk",
        0x07 => "Velvet",
        0x08 => "Wool",
        0x09 => "Gem",
        0x0A => "Agate",
        0x0B => "Amber",
        0x0C => "Amethyst",
        0x0D => "Aquamarine",
        0x0E => "Azurite",
        0x0F => "Black Garnet",
        0x10 => "Black Opal",
        0x11 => "Bloodstone",
        0x12 => "Carnelian",
        0x13 => "Citrine",
        0x14 => "Diamond",
        0x15 => "Emerald",
        0x16 => "Fire Opal",
        0x17 => "Green Garnet",
        0x18 => "Green Jade",
        0x19 => "Hematite",
        0x1A => "Imperial Topaz",
        0x1B => "Jet",
        0x1C => "Lapis Lazuli",
        0x1D => "Lavender Jade",
        0x1E => "Malachite",
        0x1F => "Moonstone",
        0x20 => "Onyx",
        0x21 => "Opal",
        0x22 => "Peridot",
        0x23 => "Red Garnet",
        0x24 => "Red Jade",
        0x25 => "Rose Quartz",
        0x26 => "Ruby",
        0x27 => "Sapphire",
        0x28 => "Smokey Quartz",
        0x29 => "Sunstone",
        0x2A => "Tiger Eye",
        0x2B => "Tourmaline",
        0x2C => "Turquoise",
        0x2D => "White Jade",
        0x2E => "White Quartz",
        0x2F => "White Sapphire",
        0x30 => "Yellow Garnet",
        0x31 => "Yellow Topaz",
        0x32 => "Zircon",
        0x33 => "Ivory",
        0x34 => "Leather",
        0x35 => "Armoredillo Hide",
        0x36 => "Gromnie Hide",
        0x37 => "Reed Shark Hide",
        0x38 => "Metal",
        0x39 => "Brass",
        0x3A => "Bronze",
        0x3B => "Copper",
        0x3C => "Gold",
        0x3D => "Iron",
        0x3E => "Pyreal",
        0x3F => "Silver",
        0x40 => "Steel",
        0x41 => "Stone",
        0x42 => "Alabaster",
        0x43 => "Granite",
        0x44 => "Marble",
        0x45 => "Obsidian",
        0x46 => "Sandstone",
        0x47 => "Serpentine",
        0x48 => "Wood",
        0x49 => "Ebony",
        0x4A => "Mahogany",
        0x4B => "Oak",
        0x4C => "Pine",
        0x4D => "Teak",
        _ => "?",
    }
}

/// The Ust, the salvaging tool (ACE `W_TINKERINGTOOL_CLASS`).
pub const UST_WCID: u32 = 20646;

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert_eq!(super::name(0x3D), "Iron");
        assert_eq!(super::name(0x4D), "Teak");
        assert_eq!(super::name(0), "?");
    }
}
