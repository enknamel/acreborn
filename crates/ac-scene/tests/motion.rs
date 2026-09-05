//! One-shot motions resolve through a MotionTable's links: an attack over
//! a combat stance, and a door swinging between Off and On. Needs
//! AC_DATA_DIR.

use ac_formats::motion_table::{command_mask, same_command};
use ac_scene::anim::{motion, AnimPlayer};
use ac_scene::Assets;

/// The human motion table (used by the player races).
const HUMAN: u32 = 0x0900_0001;
/// A plain door.
const DOOR: u32 = 0x0900_0004;

fn assets() -> Option<Assets> {
    let dir = std::env::var_os("AC_DATA_DIR")?;
    Some(Assets::open(dir).unwrap())
}

#[test]
fn human_hand_combat_attack_plays_once() {
    let Some(assets) = assets() else { return };
    let table = ac_scene::anim::motion_table(&assets, HUMAN).unwrap();
    let stance = motion::STANCE_HAND_COMBAT;
    let idle = table
        .default_motion(stance)
        .expect("HandCombat has a default");
    assert_eq!(idle, motion::READY);

    for cmd in [
        motion::ATTACK_HIGH1,
        motion::ATTACK_MED1,
        motion::ATTACK_LOW1,
    ] {
        let data = table
            .link(stance, idle, cmd)
            .unwrap_or_else(|| panic!("link for {cmd:#010x}"));
        assert!(!data.anims.is_empty());
        // The wire only carries the index; the link resolves the same way.
        let low = cmd & 0xFFFF;
        assert!(same_command(low, cmd));
        assert!(std::ptr::eq(table.link(stance, idle, low).unwrap(), data));
        assert_eq!(table.full_command(low as u16), Some(cmd));
        assert_eq!(cmd & command_mask::ACTION, command_mask::ACTION);

        let mut p = AnimPlayer::link(&assets, &table, stance, idle, low).expect("player");
        assert!(!p.looping());
        let frames = p.frame_count();
        assert!(frames > 1 && frames < 1000, "{frames} frames");
        assert!(
            p.duration() > 0.1 && p.duration() < 10.0,
            "{} s",
            p.duration()
        );
        assert!(!p.finished());
        assert!(!p.advance(p.duration() * 0.5));
        assert!(!p.finished());
        assert!(p.advance(p.duration()));
        assert!(p.finished());
        // Holds the last frame rather than wrapping.
        let n = assets.setup(0x0200_0001).unwrap().parts.len();
        let pose = p.part_transforms(n);
        assert_eq!(pose.len(), n);
        assert!(pose.iter().all(|m| m.is_finite()));
    }

    // Idle in the same stance still loops.
    let mut idle_player = AnimPlayer::cycle(&assets, &table, stance, idle).unwrap();
    assert!(idle_player.looping());
    assert!(!idle_player.advance(idle_player.duration() * 3.0));
    assert!(!idle_player.finished());

    // A command this stance has no link for is None rather than a wrong clip.
    assert!(table.link(stance, idle, 0xFFFF).is_none());
}

#[test]
fn door_swings_between_off_and_on() {
    let Some(assets) = assets() else { return };
    let table = ac_scene::anim::motion_table(&assets, DOOR).unwrap();
    let stance = motion::STANCE_NON_COMBAT;
    assert_eq!(table.default_style, stance);
    assert_eq!(
        table.default_motion(stance),
        Some(motion::OFF),
        "doors start closed"
    );

    // Both resting states are cycles, and the wire's low bits find them.
    assert!(table.cycle(stance, motion::ON).is_some());
    assert!(table.cycle(stance, motion::OFF & 0xFFFF).is_some());

    let open = AnimPlayer::link(&assets, &table, stance, motion::OFF, motion::ON & 0xFFFF)
        .expect("Off -> On");
    let close =
        AnimPlayer::link(&assets, &table, stance, motion::ON, motion::OFF).expect("On -> Off");
    assert!(!open.looping() && !close.looping());
    assert!(open.frame_count() > 1);
    assert!(open.duration() > 0.0 && close.duration() > 0.0);
    assert_eq!(table.full_command(motion::ON as u16), Some(motion::ON));

    // A door has no attack.
    assert!(table
        .link(stance, motion::OFF, motion::ATTACK_HIGH1)
        .is_none());
}
