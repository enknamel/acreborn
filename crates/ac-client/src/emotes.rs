//! Soul emotes: `*wave*` typed in chat (or `/wave`) plays a motion on
//! the character, sends it in the movement state so everyone in view
//! sees it, and says the emote line ("+Admin waves"). The retail client
//! read the words from its emote table in the language DAT; this is the
//! part of that table ACE accepts (`Entity/SoulEmote.cs`), with the
//! wording of the line beside each.

/// (words that trigger it, MotionCommand, the line's verb phrase).
pub const EMOTES: &[(&[&str], u32, &str)] = &[
    (
        &["wave", "waves", "hello", "hi", "howdy", "wave1"],
        0x1300_0087,
        "waves",
    ),
    (
        &["wave high", "wavehigh", "wave2"],
        0x1300_008e,
        "waves high",
    ),
    (&["wave low", "wavelow", "wave3"], 0x1300_008f, "waves low"),
    (&["waving", "waving hand"], 0x4300_00f1, "is waving"),
    (
        &["bow", "bows", "bow deep", "bowdeep"],
        0x4300_00ec,
        "bows deeply",
    ),
    (&["curtsey", "curtsy"], 0x4300_011a, "curtseys"),
    (
        &["cheer", "yay", "happy", "joy", "woo hoo", "whoo hoo"],
        0x1300_004c,
        "cheers",
    ),
    (
        &["clap", "claps", "applause", "clap hands", "claphands"],
        0x1300_007e,
        "claps",
    ),
    (&["clapping", "clapping hands"], 0x4300_00ed, "is clapping"),
    (
        &["laugh", "haha", "hehe", "ha", "heh", "lol"],
        0x1300_0080,
        "laughs",
    ),
    (
        &["hearty laugh", "heartylaugh", "big laugh", "biglaugh"],
        0x1300_0089,
        "laughs heartily",
    ),
    (&["cry", "cries", "sad"], 0x1300_007f, "cries"),
    (
        &["nod", "nods", "yes", "ok", "okay", "k"],
        0x1300_0083,
        "nods",
    ),
    (
        &["shake head", "shakes head", "no", "nope"],
        0x1300_0085,
        "shakes their head",
    ),
    (
        &["shrug", "shrugs", "dunno", "beats me", "i dunno"],
        0x1300_0086,
        "shrugs",
    ),
    (&["point", "points", "there"], 0x4300_00f0, "points"),
    (&["point left", "pointleft"], 0x1300_014c, "points left"),
    (&["point right", "pointright"], 0x1300_014d, "points right"),
    (&["point down", "pointdown"], 0x1300_014e, "points down"),
    (
        &["salute", "salutes", "yes sir", "yessir"],
        0x4300_00f3,
        "salutes",
    ),
    (&["kneel", "kneels"], 0x4300_00f7, "kneels"),
    (
        &["beckon", "beckons", "come", "come here", "comehere"],
        0x1300_007a,
        "beckons",
    ),
    (
        &["blow kiss", "blowkiss", "kiss", "kisses"],
        0x1300_007c,
        "blows a kiss",
    ),
    (
        &["be seeing you", "beseeingyou", "bcinu", "bcingu"],
        0x1300_007b,
        "will be seeing you",
    ),
    (
        &["shake fist", "shakefist", "shakes fist", "angry"],
        0x1300_0079,
        "shakes a fist",
    ),
    (
        &["shaking fist", "shakingfist", "getting angry"],
        0x4300_00ea,
        "is shaking a fist",
    ),
    (
        &["cringe", "cringes", "cower", "flinch"],
        0x1300_0091,
        "cringes",
    ),
    (
        &["cross arms", "crossarms"],
        0x4300_00ee,
        "crosses their arms",
    ),
    (
        &[
            "dance",
            "crazy dance",
            "crazydance",
            "drudge dance",
            "dance crazy",
        ],
        0x4300_0144,
        "dances like a drudge",
    ),
    (
        &["dance step", "dancestep"],
        0x1300_0151,
        "does a dance step",
    ),
    (&["akimbo", "heroic", "super"], 0x4300_00f2, "stands akimbo"),
    (&["at ease", "atease"], 0x4300_0149, "stands at ease"),
    (
        &["afk", "away", "away from keyboard"],
        0x4300_011b,
        "is away from the keyboard",
    ),
    (
        &["scratch head", "scratches head", "huh?"],
        0x1300_008b,
        "scratches their head",
    ),
    (
        &["scratching head", "scratching", "hmm", "hmmm", "itchy"],
        0x4300_00f4,
        "is scratching their head",
    ),
    (
        &[
            "smack head",
            "smackhead",
            "smacks head",
            "doh",
            "doh!",
            "oops",
            "slap head",
            "v8",
        ],
        0x1300_008c,
        "smacks their head",
    ),
    (
        &["tap foot", "tapfoot", "taps foot", "tapping foot", "wait"],
        0x4300_00f5,
        "taps a foot",
    ),
    (
        &["yawn", "yawns", "stretch", "stretches", "tired"],
        0x1300_0090,
        "yawns and stretches",
    ),
    (
        &["plead", "pleads", "please", "grovel", "grovels"],
        0x4300_00f8,
        "pleads",
    ),
    (
        &["shiver", "shivers", "shudder", "shudders", "brrr", "cold"],
        0x1300_0094,
        "shivers",
    ),
    (
        &["shoo", "shoos", "go away", "goaway"],
        0x1300_0095,
        "shoos",
    ),
    (&["slouch", "slouches"], 0x4300_00fa, "slouches"),
    (&["spit", "spits"], 0x1300_0097, "spits"),
    (
        &["surrender", "surrenders", "give up", "giveup"],
        0x4300_00fb,
        "surrenders",
    ),
    (&["woah", "whoa", "stop", "stops"], 0x4300_00fc, "says woah"),
    (&["winded"], 0x4300_00fd, "is winded"),
    (&["pray"], 0x4300_00eb, "prays"),
    (
        &["meditate", "pray kneel", "praykneel"],
        0x4300_011c,
        "meditates",
    ),
    (
        &["mock", "point and laugh", "pointandlaugh", "rofl"],
        0x1300_00cb,
        "points and laughs",
    ),
    (
        &["teapot", "i'm a little teapot"],
        0x1300_00cc,
        "is a little teapot",
    ),
    (
        &["warm hands", "warmhands", "blow hands", "blow on hands"],
        0x1300_0119,
        "warms their hands",
    ),
    (
        &["helper", "available"],
        0x1300_0135,
        "is available to help",
    ),
    (&["nudge left", "nudgeleft"], 0x1300_014a, "nudges left"),
    (&["nudge right", "nudgeright"], 0x1300_014b, "nudges right"),
    (&["knock"], 0x1300_014f, "knocks"),
    (
        &["scan", "scan horizon", "scanhorizon", "lookout", "peer"],
        0x1300_0150,
        "scans the horizon",
    ),
    (
        &["eat", "eats", "mime eat", "mimeeat"],
        0x1300_0081,
        "mimes eating",
    ),
    (
        &["drink", "drinks", "mime drink", "mimedrink"],
        0x1300_0082,
        "mimes drinking",
    ),
    (
        &["sit", "sits", "sit down", "sitdown", "sitting"],
        0x4300_013d,
        "sits down",
    ),
    (
        &["sit back", "sitback", "sits back"],
        0x4300_013f,
        "sits back",
    ),
    (
        &[
            "sit cross legged",
            "sitcrosslegged",
            "cross legs",
            "crosslegs",
        ],
        0x4300_013e,
        "sits cross-legged",
    ),
    (&["lean"], 0x4300_00f6, "leans"),
    (&["read", "read a book", "readabook"], 0x4300_0146, "reads"),
    (&["think", "thinker"], 0x4300_0147, "thinks"),
    (
        &["talk to the hand", "talktothehand", "talk to hand"],
        0x4300_0142,
        "talks to the hand",
    ),
    (
        &["possum", "play dead", "playdead", "play possum"],
        0x4300_0145,
        "plays dead",
    ),
    (
        &["snow angel", "snowangel"],
        0x4300_0118,
        "makes a snow angel",
    ),
    (
        &["have a seat", "haveaseat", "offer seat", "offerseat"],
        0x4300_0148,
        "offers a seat",
    ),
    (
        &["musical chair", "musicalchair"],
        0x1300_0152,
        "plays musical chairs",
    ),
    (&["ymca"], 0x1200_009b, "does the YMCA"),
    (&["atoyot"], 0x4200_00f9, "does the ATOYOT"),
];

/// The emote for a word or phrase (case-insensitive, `*` stripped).
pub fn lookup(words: &str) -> Option<(u32, &'static str)> {
    let w = words.trim().trim_matches('*').trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    EMOTES
        .iter()
        .find(|(names, _, _)| names.iter().any(|n| *n == w))
        .map(|(_, cmd, text)| (*cmd, *text))
}

/// A chat line of the form `*wave*` is an emote.
pub fn from_chat_line(line: &str) -> Option<(u32, &'static str)> {
    let t = line.trim();
    if t.len() >= 3 && t.starts_with('*') && t.ends_with('*') {
        lookup(t)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_map_to_motions() {
        assert_eq!(lookup("wave"), Some((0x1300_0087, "waves")));
        assert_eq!(lookup("*Hello*"), Some((0x1300_0087, "waves")));
        assert_eq!(lookup("bow deep").map(|e| e.1), Some("bows deeply"));
        assert!(lookup("moonwalk").is_none());
        assert_eq!(from_chat_line("*nod*").map(|e| e.1), Some("nods"));
        assert!(from_chat_line("nod").is_none());
    }
}
