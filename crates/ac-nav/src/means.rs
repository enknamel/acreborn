//! Walk, or make a journey of it?
//!
//! Arithmetic over two positions, kept apart from everything that can
//! send a packet so that it can be argued about at a desk. It has been
//! wrong in both directions in one day: a character that treated every
//! goal underground as a journey could not cross a room to hit
//! anything, and one that treated a vendor a hundred metres overhead as
//! a stroll walked into the rock beneath it.

/// How far a goal may be and still be walked to directly. One landblock
/// across: beyond that the way there is a journey, not a stroll, and
/// the steering has no business trying.
pub const WALKABLE: f32 = 192.0;

/// How to get somewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Means {
    /// Near enough to walk to, working round whatever is in between.
    Walk,
    /// Too far to walk: portals, recalls, gems.
    Journey,
    /// A journey there is already under way; leave it alone.
    Already,
}

/// Which it is.
///
/// `underground` is not a special case here and deliberately so. A
/// dungeon keeps its own corner of the world, tens of thousands of
/// metres from the town above it, so anywhere outside one is far past
/// [`WALKABLE`] and comes out a journey on distance alone -- while the
/// rest of the dungeon stays a walk, which is what a character needs to
/// reach anything it is fighting.
pub fn how_to_get_there(away: f32, travelling: bool) -> Means {
    if away <= WALKABLE {
        Means::Walk
    } else if travelling {
        Means::Already
    } else {
        Means::Journey
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn across_a_room_is_a_walk_wherever_the_room_is() {
        assert_eq!(how_to_get_there(4.0, false), Means::Walk);
        assert_eq!(how_to_get_there(120.0, false), Means::Walk);
        // Including underground: a monster across a dungeon is a walk,
        // and ruling that out left a character unable to reach anything
        // it was fighting.
        assert_eq!(how_to_get_there(WALKABLE, false), Means::Walk);
    }

    #[test]
    fn out_of_a_dungeon_is_never_a_walk() {
        // Holtburg Dungeon sits at (192, 47232); the town it belongs to
        // is at (32448, 34560). The distance says everything the cell
        // ids cannot.
        let away = ((32448.0f32 - 192.0).powi(2) + (34560.0f32 - 47232.0).powi(2)).sqrt();
        assert!(away > WALKABLE);
        assert_eq!(how_to_get_there(away, false), Means::Journey);
    }

    #[test]
    fn a_journey_under_way_is_not_started_again() {
        assert_eq!(how_to_get_there(5_000.0, true), Means::Already);
        // But a walk is a walk: a short hop is not left to a journey
        // that is heading somewhere else.
        assert_eq!(how_to_get_there(5.0, true), Means::Walk);
    }
}
