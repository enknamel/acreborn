// autoloot.rhai: when the creature we were attacking dies, open its corpse,
// take everything, and close it.
//
// The client clears `me().target` when the target dies (or when combat is
// turned off), so this watches for that transition instead of parsing chat.
// State lives in `this`, a map that is kept between calls, one per session,
// so several sessions running this script do not mix their targets up.
// Missing keys of `this` read as `()`.

fn tick(dt) {
    let m = me();

    if m.target != () {
        // Fighting: remember whom, so we know whose corpse to look for.
        this.phase = "fighting";
        this.target_name = m.target_name;
        this.wait = 0.0;
        return;
    }

    if this.phase == "fighting" {
        // The target is gone. The corpse takes a moment to arrive, and
        // opening it turns combat off; give up after a while (it may have
        // been a retreat, not a kill).
        this.wait += dt;
        if this.wait < 1.0 {
            return;
        }
        if loot("Corpse of " + this.target_name) {
            this.phase = "looting";
            this.wait = 0.0;
        } else if this.wait > 8.0 {
            this.phase = ();
        }
        return;
    }

    if this.phase == "looting" {
        // Wait for the server to show us the corpse's contents.
        if m.container_open {
            let n = take_all();
            close_container();
            log("autoloot: took " + n + " item(s) from " + this.target_name);
            this.phase = ();
            return;
        }
        this.wait += dt;
        if this.wait > 5.0 {
            log("autoloot: corpse of " + this.target_name + " did not open");
            this.phase = ();
        }
    }
}
