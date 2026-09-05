// assist.rhai: whoever is fighting posts their target on the bus; every
// other session in the process attacks it.
//
// The bus (`post` / `messages`) is shared by all sessions and plugins: a
// message posted this frame is readable by everyone during the next frame,
// then gone. `msg.from` is the posting session's index, so a session can
// ignore its own posts. Sessions are numbered from 0.
//
// Run several sessions (aclauncher, or acviewer with several accounts),
// put this file in every session's script directory (it is one process, so
// once is enough), and attack something in the active session: the others
// join in.

fn tick(dt) {
    let m = me();

    // Leader side: announce a new target once, not every frame.
    if m.target != () {
        if m.target != this.last {
            this.last = m.target;
            post("assist", #{ guid: m.target, name: m.target_name });
        }
    } else {
        this.last = ();
    }

    // Follower side: attack what the others announced (by guid, so a
    // creature with a common name is not confused with another).
    for msg in messages("assist") {
        if msg.from != m.session {
            attack(msg.value.guid);
        }
    }
}
