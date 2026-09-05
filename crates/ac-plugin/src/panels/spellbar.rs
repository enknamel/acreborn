//! Spell bar: the eight spell tabs the server keeps for the character
//! (`Options::spell_bars`), along the bottom of the screen. A tab is an
//! ordered list of shortcuts into the spellbook and nothing else (see
//! `docs/game/mechanics.md`, "The spell bar is not the component list").
//!
//! The bar is visible while B has toggled it on, while the spellbook is
//! open, or while the character is in magic mode. Keys, when it is visible
//! and no text box has focus:
//!
//! * `1`..`9` cast the nth spell of the shown tab;
//! * PageUp / Insert show the next / previous tab, PageDown / Delete
//!   select the next / previous spell; with Ctrl they jump to the last /
//!   first;
//! * Shift+Delete (or the Remove button) takes the selected spell off the
//!   tab.
//!
//! Clicking a spell selects it, double-clicking (or Cast) casts it. A
//! spell dragged from the spellbook onto a slot, the bar or a tab is added
//! there (AddSpellFavorite). Hovering shows why a spell cannot be cast
//! right now (`Client::can_cast`); such spells are dimmed but still
//! castable, since the test server does not require components.
//!
//! `/bar` prints the tabs, `/bar N` shows tab N, `/bar add NAME [N]` and
//! `/bar remove NAME [N]` edit tab N (the shown one by default).

use ac_client::magic::{CastCheck, SPELL_BARS};
use ac_formats::spell_components::SpellComponentTable;
use ac_formats::spell_table::SpellTable;

use super::{caption, has_sheet, window, Source, SpellDrag};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin};

/// Blackboard key: the shown tab (0-based), so the spellbook knows where
/// a double-clicked spell goes.
pub const SHOWN_KEY: &str = "panels.spellbar_shown";
/// Blackboard key: `true` while the bar is drawn.
pub const VISIBLE_KEY: &str = "panels.spellbar_visible";

/// One slot of a spell bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarSpell {
    pub id: u32,
    pub name: String,
    /// RenderSurface (0x06) id of the spell icon.
    pub icon: u32,
    /// Why the spell cannot be cast right now; `None` when it can.
    pub blocked: Option<String>,
}

/// The eight bars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarView {
    pub bars: Vec<Vec<BarSpell>>,
}

impl Default for BarView {
    fn default() -> Self {
        BarView {
            bars: vec![Vec::new(); SPELL_BARS],
        }
    }
}

/// How a cycling key moves the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Prev,
    First,
    Last,
}

/// Move through `len` slots with wrap-around; `None` when there are none.
/// From no selection, `Next` picks the first slot and `Prev` the last.
pub fn cycle(current: Option<usize>, len: usize, step: Step) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (step, current) {
        (Step::First, _) | (Step::Next, None) => 0,
        (Step::Last, _) | (Step::Prev, None) => len - 1,
        (Step::Next, Some(i)) => (i + 1) % len,
        (Step::Prev, Some(i)) => (i + len - 1) % len,
    })
}

/// `1`..`9` -> slot 0..8.
pub fn number_key(key: egui::Key) -> Option<usize> {
    use egui::Key::*;
    Some(match key {
        Num1 => 0,
        Num2 => 1,
        Num3 => 2,
        Num4 => 3,
        Num5 => 4,
        Num6 => 5,
        Num7 => 6,
        Num8 => 7,
        Num9 => 8,
        _ => return None,
    })
}

/// PageUp / Insert cycle the tabs; Ctrl jumps to the last / first.
pub fn tab_step(key: egui::Key, ctrl: bool) -> Option<Step> {
    Some(match (key, ctrl) {
        (egui::Key::PageUp, false) => Step::Next,
        (egui::Key::PageUp, true) => Step::Last,
        (egui::Key::Insert, false) => Step::Prev,
        (egui::Key::Insert, true) => Step::First,
        _ => return None,
    })
}

/// PageDown / Delete cycle the selected spell; Ctrl jumps to the last /
/// first.
pub fn spell_step(key: egui::Key, ctrl: bool) -> Option<Step> {
    Some(match (key, ctrl) {
        (egui::Key::PageDown, false) => Step::Next,
        (egui::Key::PageDown, true) => Step::Last,
        (egui::Key::Delete, false) => Step::Prev,
        (egui::Key::Delete, true) => Step::First,
        _ => return None,
    })
}

/// A `/bar` command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarCmd {
    /// `/bar`: print the tabs.
    Status,
    /// `/bar N`: show tab N (0-based here, 1-based on the command line).
    Show(usize),
    /// `/bar add NAME [N]`.
    Add(String, Option<usize>),
    /// `/bar remove NAME [N]`.
    Remove(String, Option<usize>),
    Bad,
}

/// Split a trailing tab number (1..=8) off a spell name.
fn split_tab(rest: &str) -> (String, Option<usize>) {
    let rest = rest.trim();
    if let Some((name, last)) = rest.rsplit_once(' ') {
        if let Ok(n) = last.parse::<usize>() {
            if (1..=SPELL_BARS).contains(&n) {
                return (name.trim().to_string(), Some(n - 1));
            }
        }
    }
    (rest.to_string(), None)
}

/// Parse the arguments of `/bar`.
pub fn parse_bar_args(args: &str) -> BarCmd {
    let args = args.trim();
    if args.is_empty() {
        return BarCmd::Status;
    }
    if let Ok(n) = args.parse::<usize>() {
        return if (1..=SPELL_BARS).contains(&n) {
            BarCmd::Show(n - 1)
        } else {
            BarCmd::Bad
        };
    }
    let (verb, rest) = args.split_once(' ').unwrap_or((args, ""));
    let (name, tab) = split_tab(rest);
    if name.is_empty() {
        return BarCmd::Bad;
    }
    match verb.to_ascii_lowercase().as_str() {
        "add" => BarCmd::Add(name, tab),
        "remove" | "rm" | "del" => BarCmd::Remove(name, tab),
        _ => BarCmd::Bad,
    }
}

/// The spell called `query` among `names`: an exact (case-insensitive)
/// match first, else the first whose name starts with it.
pub fn resolve_spell(names: &[(u32, String)], query: &str) -> Option<u32> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    names
        .iter()
        .find(|(_, n)| n.to_ascii_lowercase() == q)
        .or_else(|| {
            names
                .iter()
                .find(|(_, n)| n.to_ascii_lowercase().starts_with(&q))
        })
        .map(|(id, _)| *id)
}

/// A one-line reason a spell cannot be cast, with component names from
/// the SpellComponentsTable when given; `None` when it can.
pub fn blocked_reason(check: &CastCheck, comps: Option<&SpellComponentTable>) -> Option<String> {
    Some(match check {
        CastCheck::Ok => return None,
        CastCheck::NotKnown => "not in the spellbook".to_string(),
        CastCheck::NoCaster => "no magic caster wielded".to_string(),
        CastCheck::MissingComponents(list) => {
            let names: Vec<String> = list
                .iter()
                .map(|(id, short)| {
                    let name = comps
                        .and_then(|t| t.get(*id))
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| format!("component {id}"));
                    if *short > 1 {
                        format!("{name} x{short}")
                    } else {
                        name
                    }
                })
                .collect();
            format!("missing components: {}", names.join(", "))
        }
        CastCheck::NotEnoughMana { need, have } => format!("not enough mana ({have}/{need})"),
    })
}

/// The bars of this session, named through the spell table, with the
/// cast check of every slot.
pub fn view(c: &Client) -> BarView {
    let table = c.assets.spell_table().ok();
    let comps = c.assets.spell_components().ok();
    let mut bars: Vec<Vec<BarSpell>> = c
        .spell_bars()
        .iter()
        .map(|bar| {
            bar.iter()
                .map(|&id| {
                    let sp = table.as_ref().and_then(|t| t.get(id));
                    BarSpell {
                        id,
                        name: sp
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|| format!("spell {id}")),
                        icon: sp.map(|s| s.icon_id).unwrap_or(0),
                        blocked: blocked_reason(&c.can_cast(id), comps.as_deref()),
                    }
                })
                .collect()
        })
        .collect();
    bars.resize_with(SPELL_BARS, Vec::new);
    bars.truncate(SPELL_BARS);
    BarView { bars }
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    /// Show this tab.
    pub show: Option<usize>,
    /// Select this slot of the shown tab.
    pub select: Option<usize>,
    pub cast: Option<u32>,
    /// `(tab, position, spell)`; `usize::MAX` appends.
    pub add: Vec<(usize, usize, u32)>,
    /// `(tab, spell)`.
    pub remove: Option<(usize, u32)>,
}

const HIGHLIGHT: egui::Color32 = egui::Color32::from_rgb(255, 215, 120);

fn drop_outline(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.expand(1.0),
        2.0,
        egui::Stroke::new(1.0, HIGHLIGHT),
        egui::StrokeKind::Outside,
    );
}

/// Draw the bar along the bottom of the viewport.
pub fn draw(
    egui: &egui::Context,
    icons: &mut IconCache,
    v: &BarView,
    shown: usize,
    selected: Option<usize>,
) -> Actions {
    let mut a = Actions::default();
    let shown = shown.min(SPELL_BARS - 1);
    let vp = egui.viewport_rect();
    let width = 720.0;
    window(
        "spellbar",
        egui::pos2(vp.center().x - width * 0.5, vp.max.y - 160.0),
        egui::vec2(width, 96.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(width - 12.0, 84.0));
        let bar = &v.bars[shown];
        let sel = selected.and_then(|i| bar.get(i));
        ui.horizontal(|ui| {
            caption(ui, "Spells");
            for (i, tab) in v.bars.iter().enumerate() {
                let r = ui
                    .selectable_label(i == shown, format!("{}", i + 1))
                    .on_hover_text(format!("tab {} ({} spells)", i + 1, tab.len()));
                if r.clicked() {
                    a.show = Some(i);
                }
                if r.dnd_hover_payload::<SpellDrag>().is_some() {
                    drop_outline(ui, r.rect);
                }
                if let Some(p) = r.dnd_release_payload::<SpellDrag>() {
                    a.add.push((i, usize::MAX, p.0));
                }
            }
            ui.separator();
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Cast"))
                .clicked()
            {
                a.cast = sel.map(|s| s.id);
            }
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Remove"))
                .on_hover_text("take the selected spell off this tab (Shift+Delete)")
                .clicked()
            {
                a.remove = sel.map(|s| (shown, s.id));
            }
            if let Some(s) = sel {
                ui.label(egui::RichText::new(&s.name).color(egui::Color32::WHITE));
                if let Some(b) = &s.blocked {
                    ui.label(
                        egui::RichText::new(b)
                            .color(egui::Color32::from_rgb(255, 150, 120))
                            .small(),
                    );
                }
            }
        });
        let row = ui.horizontal(|ui| {
            ui.set_min_width(width - 12.0);
            for (i, s) in bar.iter().enumerate() {
                let r = ui
                    .vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let icon = icons.draw(ui, IconLayers::single(s.icon), egui::Sense::click());
                        if s.blocked.is_some() {
                            ui.painter().rect_filled(
                                icon.rect,
                                2.0,
                                egui::Color32::from_black_alpha(150),
                            );
                        }
                        if selected == Some(i) {
                            ui.painter().rect_stroke(
                                icon.rect.expand(2.0),
                                2.0,
                                egui::Stroke::new(1.5, HIGHLIGHT),
                                egui::StrokeKind::Outside,
                            );
                        }
                        let number = if i < 9 {
                            (i + 1).to_string()
                        } else {
                            " ".to_string()
                        };
                        let n = ui.add(
                            egui::Label::new(
                                egui::RichText::new(number)
                                    .small()
                                    .color(egui::Color32::from_gray(200)),
                            )
                            .sense(egui::Sense::click()),
                        );
                        icon.union(n)
                    })
                    .inner;
                let r = r.on_hover_text(match &s.blocked {
                    Some(b) => format!("{}\n{b}", s.name),
                    None => s.name.clone(),
                });
                if r.hovered() {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }
                if r.double_clicked() {
                    a.select = Some(i);
                    a.cast = Some(s.id);
                } else if r.clicked() {
                    a.select = Some(i);
                }
                if r.dnd_hover_payload::<SpellDrag>().is_some() {
                    drop_outline(ui, r.rect);
                }
                if let Some(p) = r.dnd_release_payload::<SpellDrag>() {
                    a.add.push((shown, i, p.0));
                }
            }
            if bar.is_empty() {
                caption(
                    ui,
                    "(empty: drag or double-click spells from the spellbook, P)",
                );
            }
        });
        // The rest of the row appends.
        let zone = ui.interact(
            row.response.rect,
            ui.id().with("spellbar_drop"),
            egui::Sense::hover(),
        );
        if zone.dnd_hover_payload::<SpellDrag>().is_some() {
            drop_outline(ui, zone.rect);
        }
        if let Some(p) = zone.dnd_release_payload::<SpellDrag>() {
            a.add.push((shown, usize::MAX, p.0));
        }
    });
    a
}

pub struct SpellBar {
    source: Source<BarView>,
    /// Toggled by B; the bar also shows with the spellbook or in magic mode.
    pub show: bool,
    /// The shown tab, 0-based.
    pub shown: usize,
    /// Selected slot of the shown tab.
    pub selected: Option<usize>,
    /// Drawn this frame (keys only act then).
    visible: bool,
    ctrl: bool,
    shift: bool,
    /// What was drawn last, for the number keys.
    view: BarView,
}

impl Default for SpellBar {
    fn default() -> Self {
        SpellBar {
            source: Source::Live,
            show: false,
            shown: 0,
            selected: None,
            visible: false,
            ctrl: false,
            shift: false,
            view: BarView::default(),
        }
    }
}

impl SpellBar {
    /// Two tabs of real spells when the table is given (one slot shown as
    /// missing its components), open.
    pub fn demo(table: Option<&SpellTable>) -> Self {
        let spell = |id: u32, blocked: Option<&str>| {
            let sp = table.and_then(|t| t.get(id));
            BarSpell {
                id,
                name: sp
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| format!("spell {id}")),
                icon: sp.map(|s| s.icon_id).unwrap_or(0),
                blocked: blocked.map(String::from),
            }
        };
        let mut v = BarView::default();
        v.bars[0] = vec![
            spell(1, None),
            spell(5, None),
            spell(9, None),
            spell(17, None),
            spell(
                60,
                Some("missing components: Copper Scarab, Prismatic Taper x3"),
            ),
        ];
        v.bars[1] = vec![
            spell(2, None),
            spell(6, None),
            spell(20, None),
            spell(24, None),
        ];
        SpellBar {
            source: Source::Demo(v.clone()),
            show: true,
            shown: 0,
            selected: Some(1),
            visible: true,
            ctrl: false,
            shift: false,
            view: v,
        }
    }

    fn show_tab(&mut self, tab: usize) {
        self.shown = tab.min(SPELL_BARS - 1);
        self.selected = None;
    }

    fn shown_len(&self) -> usize {
        self.view.bars.get(self.shown).map_or(0, |b| b.len())
    }

    /// Cast `spell` (the server decides; a client-side reason is only
    /// logged).
    fn cast(&self, cx: &mut Ctx, spell: u32) {
        let blocked = self
            .view
            .bars
            .iter()
            .flatten()
            .find(|s| s.id == spell)
            .and_then(|s| s.blocked.clone());
        let Source::Live = self.source else { return };
        let Some(c) = cx.try_client() else { return };
        c.cast(spell);
        if let Some(b) = blocked {
            cx.log(format!("spell bar: cast sent, but {b}"));
        }
    }

    fn remove_selected(&mut self, cx: &mut Ctx) {
        let Some(s) = self
            .selected
            .and_then(|i| self.view.bars.get(self.shown).and_then(|b| b.get(i)))
        else {
            return;
        };
        let (tab, id) = (self.shown, s.id);
        self.selected = None;
        match &mut self.source {
            Source::Demo(v) => v.bars[tab].retain(|s| s.id != id),
            Source::Live => {
                if let Some(c) = cx.try_client() {
                    c.remove_from_spell_bar(tab, id);
                }
            }
        }
    }

    fn apply(&mut self, cx: &mut Ctx, a: Actions) {
        if let Some(t) = a.show {
            self.show_tab(t);
        }
        if let Some(i) = a.select {
            self.selected = Some(i);
        }
        for (tab, pos, spell) in a.add {
            match &mut self.source {
                Source::Demo(v) => {
                    let list = &mut v.bars[tab];
                    if let Some(name) = self
                        .view
                        .bars
                        .iter()
                        .flatten()
                        .find(|s| s.id == spell)
                        .cloned()
                    {
                        list.retain(|s| s.id != spell);
                        let pos = pos.min(list.len());
                        list.insert(pos, name);
                    }
                }
                Source::Live => {
                    if let Some(c) = cx.try_client() {
                        c.add_to_spell_bar(tab, pos, spell);
                    }
                }
            }
        }
        if a.remove.is_some() {
            self.remove_selected(cx);
        }
        if let Some(spell) = a.cast {
            self.cast(cx, spell);
        }
    }

    /// Known spells by name, for `/bar add|remove NAME`.
    fn known_names(c: &Client) -> Vec<(u32, String)> {
        c.known_spell_ids()
            .into_iter()
            .filter_map(|id| c.spell(id).map(|s| (id, s.name)))
            .collect()
    }
}

impl Plugin for SpellBar {
    fn name(&self) -> &str {
        "spellbar"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        (self.ctrl, self.shift) =
            egui.input(|i| (i.modifiers.ctrl || i.modifiers.command, i.modifiers.shift));
        let spellbook_open = cx
            .board
            .get(super::spellbook::OPEN_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let view = match &self.source {
            Source::Demo(v) => {
                self.visible = self.show;
                Some(v.clone())
            }
            Source::Live => match cx.try_client() {
                Some(c) if has_sheet(c) => {
                    self.visible = self.show || spellbook_open || c.magic;
                    Some(view(c))
                }
                _ => {
                    self.visible = false;
                    None
                }
            },
        };
        cx.board.set(SHOWN_KEY, self.shown as u64);
        cx.board.set(VISIBLE_KEY, self.visible);
        let Some(view) = view else { return };
        self.view = view;
        if !self.visible {
            return;
        }
        if self.selected.is_some_and(|i| i >= self.shown_len()) {
            self.selected = None;
        }
        let a = draw(egui, cx.icons(), &self.view, self.shown, self.selected);
        self.apply(cx, a);
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        if key == egui::Key::B {
            self.show = !self.show;
            return true;
        }
        if !self.visible {
            return false;
        }
        if let Some(n) = number_key(key) {
            if let Some(s) = self.view.bars.get(self.shown).and_then(|b| b.get(n)) {
                let id = s.id;
                self.selected = Some(n);
                self.cast(cx, id);
            }
            return true;
        }
        if key == egui::Key::Delete && self.shift {
            self.remove_selected(cx);
            return true;
        }
        if let Some(step) = tab_step(key, self.ctrl) {
            let tab = cycle(Some(self.shown), SPELL_BARS, step).unwrap_or(0);
            self.show_tab(tab);
            return true;
        }
        if let Some(step) = spell_step(key, self.ctrl) {
            self.selected = cycle(self.selected, self.shown_len(), step);
            return true;
        }
        false
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        if name != "bar" {
            return false;
        }
        match parse_bar_args(args) {
            BarCmd::Status => {
                for (i, bar) in self.view.bars.iter().enumerate() {
                    if bar.is_empty() && i != self.shown {
                        continue;
                    }
                    let names: Vec<&str> = bar.iter().map(|s| s.name.as_str()).collect();
                    cx.log(format!(
                        "bar {}{}: {}",
                        i + 1,
                        if i == self.shown { " (shown)" } else { "" },
                        if names.is_empty() {
                            "(empty)".to_string()
                        } else {
                            names.join(", ")
                        }
                    ));
                }
            }
            BarCmd::Show(tab) => {
                self.show_tab(tab);
                self.show = true;
                cx.log(format!("spell bar: tab {}", tab + 1));
            }
            BarCmd::Add(query, tab) | BarCmd::Remove(query, tab)
                if matches!(self.source, Source::Live) =>
            {
                let tab = tab.unwrap_or(self.shown);
                let add = matches!(parse_bar_args(args), BarCmd::Add(..));
                let Some(c) = cx.try_client() else {
                    return true;
                };
                match resolve_spell(&Self::known_names(c), &query) {
                    Some(id) => {
                        let name = c.spell(id).map(|s| s.name).unwrap_or_default();
                        if add {
                            c.add_to_spell_bar(tab, usize::MAX, id);
                        } else {
                            c.remove_from_spell_bar(tab, id);
                        }
                        cx.log(format!(
                            "spell bar: {} {name} {} tab {}",
                            if add { "added" } else { "removed" },
                            if add { "to" } else { "from" },
                            tab + 1
                        ));
                    }
                    None => cx.log(format!("spell bar: no known spell called {query}")),
                }
            }
            BarCmd::Add(..) | BarCmd::Remove(..) => {
                cx.log("spell bar: no session");
            }
            BarCmd::Bad => {
                cx.log("usage: /bar | /bar N | /bar add NAME [N] | /bar remove NAME [N]");
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps() {
        assert_eq!(cycle(Some(0), 3, Step::Next), Some(1));
        assert_eq!(cycle(Some(2), 3, Step::Next), Some(0));
        assert_eq!(cycle(Some(0), 3, Step::Prev), Some(2));
        assert_eq!(cycle(Some(1), 3, Step::First), Some(0));
        assert_eq!(cycle(Some(1), 3, Step::Last), Some(2));
        assert_eq!(cycle(None, 3, Step::Next), Some(0));
        assert_eq!(cycle(None, 3, Step::Prev), Some(2));
        assert_eq!(cycle(None, 0, Step::Next), None);
        assert_eq!(cycle(Some(5), 0, Step::Last), None);
    }

    #[test]
    fn number_keys_map_to_slots() {
        assert_eq!(number_key(egui::Key::Num1), Some(0));
        assert_eq!(number_key(egui::Key::Num9), Some(8));
        assert_eq!(number_key(egui::Key::Num0), None);
        assert_eq!(number_key(egui::Key::A), None);
    }

    #[test]
    fn cycling_keys() {
        assert_eq!(tab_step(egui::Key::PageUp, false), Some(Step::Next));
        assert_eq!(tab_step(egui::Key::Insert, false), Some(Step::Prev));
        assert_eq!(tab_step(egui::Key::PageUp, true), Some(Step::Last));
        assert_eq!(tab_step(egui::Key::Insert, true), Some(Step::First));
        assert_eq!(tab_step(egui::Key::Delete, false), None);
        assert_eq!(spell_step(egui::Key::PageDown, false), Some(Step::Next));
        assert_eq!(spell_step(egui::Key::Delete, false), Some(Step::Prev));
        assert_eq!(spell_step(egui::Key::PageDown, true), Some(Step::Last));
        assert_eq!(spell_step(egui::Key::Delete, true), Some(Step::First));
        assert_eq!(spell_step(egui::Key::PageUp, false), None);
    }

    #[test]
    fn bar_commands_parse() {
        assert_eq!(parse_bar_args(""), BarCmd::Status);
        assert_eq!(parse_bar_args("3"), BarCmd::Show(2));
        assert_eq!(parse_bar_args("9"), BarCmd::Bad);
        assert_eq!(
            parse_bar_args("add Strength Self I"),
            BarCmd::Add("Strength Self I".into(), None)
        );
        assert_eq!(
            parse_bar_args("add Heal Self II 2"),
            BarCmd::Add("Heal Self II".into(), Some(1))
        );
        assert_eq!(
            parse_bar_args("remove Heal Self II"),
            BarCmd::Remove("Heal Self II".into(), None)
        );
        assert_eq!(parse_bar_args("add"), BarCmd::Bad);
        assert_eq!(parse_bar_args("frob x"), BarCmd::Bad);
    }

    #[test]
    fn spell_names_resolve_exact_then_prefix() {
        let names = vec![
            (1, "Strength Self I".to_string()),
            (2, "Strength Self II".to_string()),
            (3, "Heal Self I".to_string()),
        ];
        assert_eq!(resolve_spell(&names, "strength self ii"), Some(2));
        assert_eq!(resolve_spell(&names, "Strength"), Some(1));
        assert_eq!(resolve_spell(&names, "heal"), Some(3));
        assert_eq!(resolve_spell(&names, "Blade"), None);
        assert_eq!(resolve_spell(&names, ""), None);
    }

    #[test]
    fn reasons_read_well() {
        assert_eq!(blocked_reason(&CastCheck::Ok, None), None);
        assert_eq!(
            blocked_reason(&CastCheck::NoCaster, None).as_deref(),
            Some("no magic caster wielded")
        );
        assert_eq!(
            blocked_reason(&CastCheck::MissingComponents(vec![(1, 1), (188, 3)]), None).as_deref(),
            Some("missing components: component 1, component 188 x3")
        );
        assert_eq!(
            blocked_reason(&CastCheck::NotEnoughMana { need: 30, have: 5 }, None).as_deref(),
            Some("not enough mana (5/30)")
        );
    }

    #[test]
    fn demo_has_two_tabs() {
        let bar = SpellBar::demo(None);
        assert_eq!(bar.view.bars.len(), SPELL_BARS);
        assert_eq!(bar.view.bars[0].len(), 5);
        assert_eq!(bar.view.bars[1].len(), 4);
        assert!(bar.view.bars[2].is_empty());
    }
}
