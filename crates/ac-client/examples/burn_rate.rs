//! What a cast actually burns, from the client's own tables.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example burn_rate [SKILL]`
//!
//! ACE rolls each component of a formula separately
//! (`Spell.TryBurnComponents`):
//!
//! ```text
//! burn = spell.ComponentLoss * component.CDM * min(1, spell.Power / skill)
//! ```
//!
//! so the rate falls as the caster's skill rises past the spell's
//! power. Both inputs are in the client's own data, which means the
//! expected burn per cast can be worked out rather than guessed at.
use std::rc::Rc;

use ac_scene::Assets;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let skill: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(400.0);
    let assets = Rc::new(Assets::open(std::path::Path::new(&dir)).unwrap());
    let spells = assets.spell_table().unwrap();
    let comps = assets.spell_components().unwrap();

    println!("caster skill {skill:.0}\n");
    // Machine-readable when asked, so the arithmetic can be done
    // outside: one line per component of the chosen spell.
    let tsv = std::env::var("BURN_TSV").is_ok();
    if !tsv {
        println!(
            "{:<34} {:>5} {:>6} {:>7}  expected burn per cast",
            "spell", "power", "loss", "skillMod"
        );
    }

    // A spread of war spells, low to top, by name.
    let mut rows: Vec<&ac_formats::spell_table::Spell> = spells
        .spells
        .iter()
        .map(|(_, s)| s)
        .filter(|s| {
            let want = std::env::args()
                .nth(2)
                .unwrap_or_else(|| "Shock Wave".into());
            s.name.contains(&want)
        })
        .collect();
    rows.sort_by_key(|s| s.power);

    for sp in rows.iter().rev().take(4) {
        let name = sp.name.as_str();
        let skill_mod = (sp.power as f32 / skill).min(1.0);
        // With foci a formula collapses to its scarabs (and chorizite)
        // plus prismatic tapers: no herbs, no talismans, no coloured
        // tapers. That is what a real caster burns, so it is what this
        // prices unless FULL_FORMULA says otherwise.
        let full: Vec<u32> = sp.formula().collect();
        let formula: Vec<u32> = if std::env::var("FULL_FORMULA").is_ok() {
            full
        } else {
            ac_client::magic::foci_formula(&full)
        };
        let mut parts: Vec<String> = Vec::new();
        for c in &formula {
            let Some(comp) = comps.get(*c) else {
                continue;
            };
            let burn = sp.component_loss * comp.cdm * skill_mod;
            // Casts per unit reads better than a fraction per cast.
            let per = if burn > 0.0 {
                format!("1 per {:.0} casts", 1.0 / burn)
            } else {
                "never".to_string()
            };
            parts.push(format!("{}: {per}", comp.name));
        }
        if tsv {
            // spell, power, cast seconds, component, burn per cast
            let secs: f32 = formula
                .iter()
                .filter_map(|c| comps.get(*c))
                .map(|c| c.time)
                .sum();
            for c in &formula {
                if let Some(comp) = comps.get(*c) {
                    let burn = sp.component_loss * comp.cdm * skill_mod;
                    println!("{name}\t{}\t{secs:.2}\t{}\t{burn:.6}", sp.power, comp.name);
                }
            }
        } else {
            println!(
                "{name:<34} {:>5} {:>6.2} {:>7.2}  {}",
                sp.power,
                sp.component_loss,
                skill_mod,
                parts.join(", ")
            );
        }
    }
}
