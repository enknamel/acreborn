// watch.rhai: a running commentary of what autoplay does, for this
// session and for every other one in reach.
//
// Autoplay reports each change of what it is doing (fighting, looting,
// buffing, healing, following...) as an event: `ev.kind == "autoplay"`
// with `ev.doing` (the state, one lower-case word) and `ev.text` (the
// line the Autoplay panel shows). The host repeats every such event on
// the bus topic "autoplay.event", so the other sessions of this process
// and, with --bus, of every other process can be watched too: each
// message carries `session`, `name`, `doing` and `text`, and `origin`
// names the process it came from when it is not this one.

// Our own session's changes, as they happen.
fn on_event(ev) {
    if ev.kind != "autoplay" { return; }
    this.changes = if this.changes == () { 1 } else { this.changes + 1 };
    log("[" + ev.doing + "] " + ev.text + " (#" + this.changes + ")");
}

// Everyone else's, from the bus (a message from ourselves is skipped:
// on_event already said it).
fn tick(dt) {
    for m in messages("autoplay.event") {
        let mine = m.origin == () && m.from == session_index();
        if mine { continue; }
        let who = if m.value.name == "" { "session " + m.value.session } else { m.value.name };
        let from = if m.origin == () { "" } else { " @" + m.origin };
        log(who + from + ": " + m.value.text);
    }
}
