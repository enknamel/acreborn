// find_items.rhai: F6 lists the carried weapons that hit for more than 10,
// with the lines their tooltips would show.
//
// `find_items(query)` searches everything carried and worn with the same
// query language as the inventory search box: words match names, materials,
// kinds and spell names; `dmg>10` compares a number. Damage, armor level and
// spells are only known once the server has appraised an item, so the first
// press queues appraisals of whatever is missing (`appraise_all`, one at a
// time in the background) and asks you to press again.

fn key(name, pressed) {
    if name != "F6" || !pressed {
        return false;
    }

    let missing = unappraised();
    if missing > 0 {
        let queued = appraise_all();
        log("find_items: appraising " + queued + " item(s), press F6 again in a moment");
        return true;
    }

    let weapons = find_items("type:weapon dmg>10");
    log("find_items: " + weapons.len() + " weapon(s) with damage over 10");
    for w in weapons {
        let worn = if w.wielded { " (wielded)" } else { "" };
        log("  " + w.name + worn + ": " + w.damage_low + "-" + w.damage_high + " " + w.damage_type);
        for line in w.summary {
            log("    " + line);
        }
    }
    true
}
