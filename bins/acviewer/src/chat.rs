//! The chat log behind the overlay's chat window: lines tagged with the
//! server's ChatMessageType, the tabs that filter them, unread counts,
//! the scrollback rule and the sender-name lookup for click-to-tell.
//! Drawing is in `ui.rs`; everything here is plain data so it can be
//! tested without a GPU.

use std::ops::Range;

use ac_net::messages::{channel, turbine};

/// The tabs across the top of the chat window, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    All,
    Say,
    Combat,
    Magic,
    Channels,
    System,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::All,
        Tab::Say,
        Tab::Combat,
        Tab::Magic,
        Tab::Channels,
        Tab::System,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Say => "Say",
            Tab::Combat => "Combat",
            Tab::Magic => "Magic",
            Tab::Channels => "Channels",
            Tab::System => "System",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Whether a line of this `kind` belongs on this tab.
    pub fn shows(self, kind: u32) -> bool {
        self == Tab::All || tab_for(kind) == self
    }
}

/// The tab a line's kind belongs to (never `All`). The values are the
/// server's ChatMessageType (`ac_client::Event::Chat::kind`): 2 speech,
/// 3/4 tells, 0xC/0x1F emotes; 5/6 the client's own hit lines and 0x15/
/// 0x16 CombatEnemy/CombatSelf; 7 magic and 0x11 spellcasting; 8/9 the
/// group channels, 0x12/0x13 allegiance/fellowship plus the channel and
/// Turbine-room markers; everything else (0 broadcast, 1 appraisal, 0xD
/// advancement, 0x17 recall, 0x18 craft...) is system.
pub fn tab_for(kind: u32) -> Tab {
    match kind {
        2..=4 | 0xC | 0x1F => Tab::Say,
        5 | 6 | 0x15 | 0x16 => Tab::Combat,
        7 | 0x11 => Tab::Magic,
        8..=0xB | 0x12 | 0x13 => Tab::Channels,
        k if k == channel::KIND || k == turbine::KIND => Tab::Channels,
        _ => Tab::System,
    }
}

/// The colour a line of this kind is drawn in.
pub fn color_for(kind: u32) -> egui::Color32 {
    match kind {
        2 => egui::Color32::from_rgb(230, 230, 230),
        3 | 4 => egui::Color32::from_rgb(255, 170, 230),
        0xC | 0x1F => egui::Color32::from_rgb(215, 205, 160),
        0 => egui::Color32::from_rgb(255, 220, 120),
        5 | 6 | 0x15 | 0x16 => egui::Color32::from_rgb(255, 145, 120),
        7 | 0x11 => egui::Color32::from_rgb(170, 190, 255),
        8..=0xB | 0x12 | 0x13 => egui::Color32::from_rgb(150, 230, 170),
        k if k == channel::KIND => egui::Color32::from_rgb(150, 230, 170),
        k if k == turbine::KIND => egui::Color32::from_rgb(140, 225, 225),
        _ => egui::Color32::from_rgb(180, 210, 255),
    }
}

/// The colour of the timestamp at the start of each line.
pub const STAMP_COLOR: egui::Color32 = egui::Color32::from_gray(135);

/// Byte range of the sender's name in a line the client formats as
/// `Name says, "..."`, `Name tells you, "..."`, `[Channel] Name says,
/// "..."` or `[Room] Name: ...` (see `ac_client`'s chat formatting).
/// `None` for our own lines (`You say, ...`) and everything else.
pub fn sender_name(text: &str) -> Option<Range<usize>> {
    let start = if text.starts_with('[') {
        text.find("] ")? + 2
    } else {
        0
    };
    let rest = &text[start..];
    let mut end = [" tells you, \"", " says, \""]
        .iter()
        .filter_map(|m| rest.find(m))
        .min();
    if start > 0 {
        // A Turbine room line: `[General] Name: text`.
        end = end.into_iter().chain(rest.find(": ")).min();
    }
    let end = end?;
    let name = &rest[..end];
    if name.is_empty() || name == "You" || name.len() > 40 || name.contains('"') {
        return None;
    }
    Some(start..start + end)
}

/// The chat-box text a click on `name` produces: `/tell Name ` (the
/// `+` admins are shown with is not part of the name).
pub fn tell_prefix(name: &str) -> String {
    format!("/tell {} ", name.trim_start_matches('+'))
}

/// Where the log is scrolled to, and how many lines arrived while it was
/// scrolled up. The view only follows new lines when it was already at
/// the bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scrollback {
    pub at_bottom: bool,
    /// Lines added since the user scrolled up; the "new messages" pill.
    pub unseen: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Scrollback {
            at_bottom: true,
            unseen: 0,
        }
    }
}

impl Scrollback {
    /// Where the scroll landed this frame: `offset` of at most `max`
    /// (content height less the viewport's).
    pub fn observe(&mut self, offset: f32, max: f32) {
        self.at_bottom = offset >= max - 1.0;
        if self.at_bottom {
            self.unseen = 0;
        }
    }

    /// A line was added: true when the view should follow it.
    pub fn pushed(&mut self) -> bool {
        if !self.at_bottom {
            self.unseen += 1;
        }
        self.at_bottom
    }

    /// The user asked for the bottom (the pill, a tab switch).
    pub fn jump(&mut self) {
        self.at_bottom = true;
        self.unseen = 0;
    }
}

pub struct ChatLine {
    pub text: String,
    /// ChatMessageType from the server (0 broadcast, 2 speech, ...).
    pub kind: u32,
    /// Local time of arrival, `HH:MM`.
    pub stamp: String,
    /// The clickable sender name within `text`, if any.
    pub name: Option<Range<usize>>,
}

pub struct ChatLog {
    pub lines: Vec<ChatLine>,
    pub tab: Tab,
    unread: [usize; 6],
    pub scroll: Scrollback,
    /// Scroll to the bottom on the next draw (set by `select` and the
    /// pill); the drawing code clears it.
    pub jump: bool,
    /// Height of the chat window in points; dragged from its top edge.
    pub height: f32,
}

impl ChatLog {
    pub const MAX_LINES: usize = 2000;
    pub const DEFAULT_HEIGHT: f32 = 260.0;
    pub const MIN_HEIGHT: f32 = 140.0;

    pub fn new() -> Self {
        ChatLog {
            lines: Vec::new(),
            tab: Tab::All,
            unread: [0; 6],
            scroll: Scrollback::default(),
            jump: false,
            height: Self::DEFAULT_HEIGHT,
        }
    }

    /// Add a line stamped with the local time.
    pub fn push(&mut self, text: String, kind: u32) {
        self.push_at(text, kind, stamp_now());
    }

    pub fn push_at(&mut self, text: String, kind: u32, stamp: String) {
        // A line the active tab shows has been seen, whichever other tabs
        // would also carry it (All shows everything, so nothing is unread
        // while it is open).
        if self.tab.shows(kind) {
            self.scroll.pushed();
        } else {
            for tab in Tab::ALL {
                if tab != self.tab && tab.shows(kind) {
                    self.unread[tab.index()] += 1;
                }
            }
        }
        let name = sender_name(&text);
        self.lines.push(ChatLine {
            text,
            kind,
            stamp,
            name,
        });
        if self.lines.len() > Self::MAX_LINES {
            let extra = self.lines.len() - Self::MAX_LINES;
            self.lines.drain(..extra);
        }
    }

    /// Switch tabs: the tab's unread count clears and the view jumps to
    /// its newest line.
    pub fn select(&mut self, tab: Tab) {
        self.tab = tab;
        self.unread[tab.index()] = 0;
        self.scroll.jump();
        self.jump = true;
    }

    /// Lines that arrived on `tab` while another tab was active.
    pub fn unread(&self, tab: Tab) -> usize {
        self.unread[tab.index()]
    }

    /// The lines the active tab shows.
    pub fn visible(&self) -> impl Iterator<Item = &ChatLine> {
        let tab = self.tab;
        self.lines.iter().filter(move |l| tab.shows(l.kind))
    }
}

impl Default for ChatLog {
    fn default() -> Self {
        Self::new()
    }
}

/// The local time as `HH:MM`.
pub fn stamp_now() -> String {
    let (h, m) = local_hm();
    format!("{h:02}:{m:02}")
}

#[cfg(unix)]
fn local_hm() -> (u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    (tm.tm_hour as u32, tm.tm_min as u32)
}

#[cfg(not(unix))]
fn local_hm() -> (u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs / 3600 % 24) as u32, (secs / 60 % 60) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_tabs() {
        assert_eq!(tab_for(2), Tab::Say);
        assert_eq!(tab_for(3), Tab::Say);
        assert_eq!(tab_for(4), Tab::Say);
        assert_eq!(tab_for(0x1F), Tab::Say);
        assert_eq!(tab_for(5), Tab::Combat);
        assert_eq!(tab_for(6), Tab::Combat);
        assert_eq!(tab_for(0x16), Tab::Combat);
        assert_eq!(tab_for(7), Tab::Magic);
        assert_eq!(tab_for(0x11), Tab::Magic);
        assert_eq!(tab_for(channel::KIND), Tab::Channels);
        assert_eq!(tab_for(turbine::KIND), Tab::Channels);
        assert_eq!(tab_for(0x12), Tab::Channels);
        assert_eq!(tab_for(0x13), Tab::Channels);
        assert_eq!(tab_for(0), Tab::System);
        assert_eq!(tab_for(1), Tab::System);
        assert_eq!(tab_for(0xD), Tab::System);
        assert!(Tab::All.shows(7));
        assert!(Tab::Magic.shows(7));
        assert!(!Tab::Say.shows(7));
        // Every kind lands on exactly one tab besides All.
        for kind in (0..0x20).chain([channel::KIND, turbine::KIND]) {
            let n = Tab::ALL[1..].iter().filter(|t| t.shows(kind)).count();
            assert_eq!(n, 1, "kind {kind:#x}");
        }
    }

    #[test]
    fn names_are_found_in_speech_and_tells() {
        let t = "Reborn tells you, \"hi there\"";
        assert_eq!(&t[sender_name(t).unwrap()], "Reborn");
        let t = "Asheron's Wrath says, \"ho\"";
        assert_eq!(&t[sender_name(t).unwrap()], "Asheron's Wrath");
        let t = "[Fellowship] Bob Smith says, \"pull\"";
        assert_eq!(&t[sender_name(t).unwrap()], "Bob Smith");
        let t = "[General] +Admin: he says, \"no\"";
        assert_eq!(&t[sender_name(t).unwrap()], "+Admin");
        let t = "[Trade] Seller: wts bow";
        assert_eq!(&t[sender_name(t).unwrap()], "Seller");
        // Our own lines and everything else.
        assert_eq!(sender_name("You say, \"hi\""), None);
        assert_eq!(sender_name("[Fellowship] You say, \"hi\""), None);
        assert_eq!(sender_name("Reborn waves at you."), None);
        assert_eq!(sender_name("You hit Drudge for 12 points."), None);
        assert_eq!(sender_name("Bob says hello: no quote"), None);
        assert_eq!(sender_name(""), None);
        assert_eq!(tell_prefix("+Admin"), "/tell Admin ");
        assert_eq!(tell_prefix("Reborn"), "/tell Reborn ");
    }

    #[test]
    fn view_sticks_to_bottom_only_when_there() {
        let mut s = Scrollback::default();
        assert!(s.pushed());
        assert_eq!(s.unseen, 0);
        // Scrolled up: new lines do not move the view but are counted.
        s.observe(100.0, 400.0);
        assert!(!s.at_bottom);
        assert!(!s.pushed());
        assert!(!s.pushed());
        assert_eq!(s.unseen, 2);
        // Back at (or within a pixel of) the bottom: the count clears.
        s.observe(399.5, 400.0);
        assert!(s.at_bottom);
        assert_eq!(s.unseen, 0);
        // Content that fits has nothing to scroll.
        s.observe(0.0, -50.0);
        assert!(s.at_bottom);
        s.observe(0.0, 400.0);
        s.pushed();
        s.jump();
        assert!(s.at_bottom);
        assert_eq!(s.unseen, 0);
    }

    #[test]
    fn unread_counts_follow_the_active_tab() {
        let mut log = ChatLog::new();
        log.push_at("Bob says, \"hi\"".into(), 2, "12:00".into());
        assert_eq!(log.unread(Tab::Say), 0);
        assert_eq!(log.unread(Tab::All), 0);
        log.select(Tab::Combat);
        log.push_at("You hit Drudge for 3 points.".into(), 5, "12:01".into());
        log.push_at("Bob says, \"again\"".into(), 2, "12:01".into());
        assert_eq!(log.unread(Tab::Combat), 0);
        // Only the line Combat did not show counts as unread.
        assert_eq!(log.unread(Tab::Say), 1);
        assert_eq!(log.unread(Tab::All), 1);
        assert_eq!(log.visible().count(), 1);
        log.select(Tab::Say);
        assert_eq!(log.unread(Tab::Say), 0);
        assert_eq!(log.unread(Tab::All), 1);
        assert_eq!(log.visible().count(), 2);
        assert!(log.jump);
        assert_eq!(log.lines[0].name, Some(0..3));
        for i in 0..ChatLog::MAX_LINES + 10 {
            log.push_at(format!("line {i}"), 0, "12:02".into());
        }
        assert_eq!(log.lines.len(), ChatLog::MAX_LINES);
    }
}
