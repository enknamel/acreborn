// A character that tags along: `/follow` puts it on the team as a
// follower and lets it play on its own. It comes to whoever leads (the
// character whose client has "lead" ticked in the Autoplay panel, or
// called team_lead(true)), keeps within a few metres, fights what turns
// up, flies when the leader flies and takes a journey after the leader
// through portals. `/follow off` stops.
//
// Run the followers as extra sessions, and put every process on the
// bus so they hear each other:
//   acviewer --connect 127.0.0.1:9000 -a ACCOUNT -v PASS --bus
//   acbot --connect 127.0.0.1 --client BOT1:PASS --client BOT2:PASS \
//         --bus --script follow.txt
// with follow.txt holding the one line `/follow`.
fn command(name, args) {
    if name != "follow" { return false; }
    if args.trim() == "off" {
        follow(false);
        autoplay(false);
        log("no longer following");
        return true;
    }
    team(true);
    follow(true);
    autoplay(true);
    log("following the leader");
    true
}
