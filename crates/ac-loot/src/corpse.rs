//! What a corpse and the character looking into it look like to the
//! rules.
//!
//! Plain values, so that emptying one can be followed at a desk. What
//! it deliberately does not describe is *why* an item is worth having:
//! that is the player's profile, judged elsewhere, and arrives here as
//! a verdict already reached.

/// What the rules have been told about one thing in the corpse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Worth taking.
    Take,
    /// Not worth taking.
    Leave,
    /// Cannot be judged without asking the server about it.
    MustAsk,
}

/// One thing lying in the corpse.
#[derive(Clone, Debug, PartialEq)]
pub struct Lying {
    pub guid: u32,
    pub name: String,
    /// What the whole stack weighs.
    pub burden: u32,
    /// What the profile made of it.
    pub verdict: Verdict,
}

/// The corpse, and the character standing over it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Open {
    /// The corpse itself.
    pub guid: u32,
    pub name: String,
    /// How far off the character is standing.
    pub away: f32,
    /// The window is open and its contents are known.
    pub open: bool,
    pub items: Vec<Lying>,
    /// Slots left in the pack.
    pub slots_free: u32,
    /// How much more the character means to carry (see
    /// `growth::carry_room`): zero when it has had enough.
    pub carry_room: u32,
    /// Asking the server to identify things is allowed.
    pub may_ask: bool,
    /// An identify is already out for these.
    pub asking: Vec<u32>,
}

/// How near the character must be before a corpse will open for it.
pub const REACH: f32 = 2.5;

impl Open {
    /// The things still worth taking, dearest first is not the order --
    /// a corpse is emptied in the order it lists, because the server
    /// moves one at a time and the character wants the lot.
    pub fn wanted(&self) -> impl Iterator<Item = &Lying> {
        self.items.iter().filter(|i| i.verdict == Verdict::Take)
    }

    /// The things whose fate cannot be settled without the server.
    ///
    /// Only these are asked about. An identify is a round trip each,
    /// and on a corpse of eight that is eight of them before anything
    /// is picked up; a profile whose early rules ask about name, kind
    /// and worth empties a corpse without a single one.
    pub fn unjudged(&self) -> impl Iterator<Item = &Lying> {
        self.items
            .iter()
            .filter(|i| i.verdict == Verdict::MustAsk && !self.asking.contains(&i.guid))
    }
}
