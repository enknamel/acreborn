// A live acceptance suite: one login, every check in order, each
// asserting something measured and printing what it measured.
//
// Nearly every fault worth finding in this client has been invisible to
// `cargo test` -- a merge the server refused without a word, a step
// refused by geometry that fit at both ends, a walk cancelled by the
// acknowledgement of the request that started it. Each needed a real
// server to answer and a real character to be standing somewhere. This
// is where those are caught from now on.
//
// Each check prints:  CHECK <name> PASS|FAIL <what was seen>
//
// Rhai functions cannot see the enclosing scope, so a check either
// asks the world afresh or leaves itself a note on the bus. Nothing
// here remembers anything implicitly.

fn want(name, cond, detail) {
    if cond { log("CHECK " + name + " PASS " + detail); }
    else { log("CHECK " + name + " FAIL " + detail); }
}

fn stacks_of(what) {
    let out = [];
    for it in find_items(what) { out.push(it); }
    out
}

fn count_of(what) {
    let n = 0;
    for it in find_items(what) { n += it.stack; }
    n
}

fn with_count(what, n) {
    let found = 0;
    for it in find_items(what) { if it.stack == n { found += 1; } }
    found
}

fn command(name, args) {
    // ---- the world we were given --------------------------------
    if name == "v_placed" {
        let m = me();
        want("placed", m.cell != 0 && m.name != "",
             "name=" + m.name + " cell=" + m.cell + " level=" + m.level
             + " str=" + m.attributes.strength);
        return true;
    }

    // ---- our idea of weight agrees with the server's -------------
    if name == "v_burden" {
        let b = burden();
        let m = me();
        let total = 0;
        // Counted as the server counts it: a side pack's weight already
        // includes what is inside it (ACE Container.cs, "this value
        // includes the containers burden itself + all child items"), so
        // only what hangs directly off the character is added.
        for it in find_items("") {
            if it.container == m.guid || it.wielded { total += it.burden; }
        }
        // The server sends EncumbranceVal; we also know what every item
        // weighs. If those disagree, everything resting on weight --
        // what to loot, what to buy, whether a stack may be merged --
        // rests on a wrong number.
        want("burden_matches_items", b.carried == total,
             "server=" + b.carried + " sum_of_items=" + total
             + " capacity=" + b.capacity + " ceiling=" + b.ceiling
             + " room=" + b.room);
        return true;
    }

    // ---- what makes a thing stackable ---------------------------
    if name == "v_maxstack" {
        let t = stacks_of("Prismatic Taper");
        let every = 1;
        for x in t { if x.max_stack < 2 { every = 0; } }
        // Whether two things combine is max_stack, never how many are
        // in them now: two single tapers are one stack of two.
        want("stackable_is_max_not_count", t.len() > 0 && every == 1,
             "taper stacks seen " + t.len() + ", all reporting max_stack > 1");
        return true;
    }

    // ---- merging ------------------------------------------------
    // 37 and 41 are made fresh for this: counts nothing else carries,
    // so the check needs no memory of what was there before.
    if name == "v_merge_do" {
        let a = ();
        let b = ();
        for it in find_items("Prismatic Taper") {
            if it.stack == 37 { a = it; }
            if it.stack == 41 { b = it; }
        }
        if a == () || b == () {
            log("CHECK stacks_merge FAIL the 37 and 41 were not created");
            return true;
        }
        log("merge " + a.guid + "(37) -> " + b.guid + "(41) sent=" + merge(a.guid, b.guid));
        return true;
    }
    if name == "v_merge_check" {
        let joined = with_count("Prismatic Taper", 78);
        let left = with_count("Prismatic Taper", 37) + with_count("Prismatic Taper", 41);
        want("stacks_merge", joined >= 1 && left == 0,
             "stacks of 78: " + joined + ", of 37 or 41 still loose: " + left
             + ", tapers carried " + count_of("Prismatic Taper"));
        return true;
    }

    // ---- splitting ----------------------------------------------
    if name == "v_split_do" {
        let s = ();
        for it in find_items("Prismatic Taper") { if it.stack == 78 { s = it; } }
        if s == () { log("CHECK stacks_split FAIL no stack of 78 to cut"); return true; }
        log("split 13 off " + s.guid + " => " + split(s.guid, 13));
        return true;
    }
    if name == "v_split_check" {
        want("stacks_split",
             with_count("Prismatic Taper", 13) >= 1 && with_count("Prismatic Taper", 65) >= 1,
             "stacks of 13: " + with_count("Prismatic Taper", 13)
             + ", of 65: " + with_count("Prismatic Taper", 65));
        return true;
    }

    // ---- walking ------------------------------------------------
    // The one check that needs memory: where we set off from, and
    // where we were told to go. The shared board keeps it.
    if name == "v_walk_do" {
        let m = me();
        let tx = m.x + 10.0;
        let ty = m.y;
        board_set("walk_from_x", m.x);
        board_set("walk_from_y", m.y);
        board_set("walk_to_x", tx);
        board_set("walk_to_y", ty);
        walk_to(tx, ty, m.z, 1.0);
        log("walking from " + m.x + "," + m.y + " to " + tx + "," + ty);
        return true;
    }
    if name == "v_walk_check" {
        let fx = board_get("walk_from_x");
        let fy = board_get("walk_from_y");
        let dx = board_get("walk_to_x");
        let dy = board_get("walk_to_y");
        if fx == () || dx == () {
            log("CHECK walks_to_a_point FAIL nothing was recorded");
            return true;
        }
        let m = me();
        let moved = ((m.x - fx) * (m.x - fx) + (m.y - fy) * (m.y - fy)).sqrt();
        let left = ((m.x - dx) * (m.x - dx) + (m.y - dy) * (m.y - dy)).sqrt();
        walk_stop();
        // Ten metres of open ground. Arriving is the point; moving at
        // all is the thing that was broken, so both are reported.
        want("walks_to_a_point", left < 2.0,
             "moved " + moved + " m, still " + left + " m short");
        return true;
    }

    false
}
