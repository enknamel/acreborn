//! Allegiance (L): the patron/vassal tree the server sends us (monarch,
//! patron, ourselves and our direct vassals), with what each member
//! passes up. Swear to the selected player (they answer a confirmation),
//! break with the patron or a vassal, refresh, and as monarch name the
//! allegiance; officers set the message of the day. Vassals, patron,
//! monarch and co-vassal chat go through `/v`, `/p`, `/m`, `/c`.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRow {
    pub guid: u32,
    pub name: String,
    pub level: u32,
    pub rank: u32,
    pub loyalty: u32,
    pub leadership: u32,
    pub online: bool,
    /// XP waiting for this patron from their vassals.
    pub xp_cached: u64,
    /// XP this member has passed up so far.
    pub xp_tithed: u64,
    pub may_passup: bool,
    /// 0 none, 1 speaker, 2 seneschal, 3 castellan.
    pub officer: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllegianceView {
    /// False until the server has answered the profile request.
    pub loaded: bool,
    /// None when not in an allegiance.
    pub name: Option<String>,
    pub rank: u32,
    pub total_members: u32,
    pub total_vassals: u32,
    pub motd: String,
    pub motd_set_by: String,
    pub i_am_monarch: bool,
    /// Officer level of ourselves (monarch counts as 3).
    pub my_officer: u32,
    pub monarch: Option<MemberRow>,
    pub patron: Option<MemberRow>,
    pub me: Option<MemberRow>,
    pub vassals: Vec<MemberRow>,
    /// The selected player, who Swear would pledge to.
    pub selected: Option<(u32, String)>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub swear: Option<u32>,
    pub break_with: Option<u32>,
    pub refresh: bool,
    pub set_name: Option<String>,
    pub set_motd: Option<String>,
}

pub const OFFICER_TITLES: [&str; 4] = ["", "Speaker", "Seneschal", "Castellan"];

fn row(a: &ac_world::allegiance::Allegiance, m: &ac_world::allegiance::Member) -> MemberRow {
    MemberRow {
        guid: m.guid,
        name: m.name.clone(),
        level: m.level,
        rank: m.rank,
        loyalty: m.loyalty,
        leadership: m.leadership,
        online: m.online,
        xp_cached: m.xp_cached,
        xp_tithed: m.xp_tithed,
        may_passup: m.may_passup,
        officer: a.officer_level(m.guid),
    }
}

pub fn view(c: &Client) -> AllegianceView {
    let me = c.world.player_guid;
    let selected = c
        .selected
        .and_then(|g| c.world.objects.get(&g))
        .filter(|o| {
            o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0 && Some(o.guid) != me
        })
        .map(|o| (o.guid, o.name.clone()));
    match c.world.allegiance.as_ref() {
        Some(a) if a.is_member() => {
            let i_am_monarch = a.is_monarch();
            AllegianceView {
                loaded: true,
                name: Some(if a.name.is_empty() {
                    a.monarch
                        .as_ref()
                        .map(|m| format!("{}'s allegiance", m.name))
                        .unwrap_or_default()
                } else {
                    a.name.clone()
                }),
                rank: a.rank,
                total_members: a.total_members,
                total_vassals: a.total_vassals,
                motd: a.motd.clone(),
                motd_set_by: a.motd_set_by.clone(),
                i_am_monarch,
                my_officer: if i_am_monarch {
                    3
                } else {
                    me.map(|g| a.officer_level(g)).unwrap_or(0)
                },
                monarch: a.monarch.as_ref().map(|m| row(a, m)),
                patron: a.patron.as_ref().map(|m| row(a, m)),
                me: a.me.as_ref().map(|m| row(a, m)),
                vassals: a.vassals.iter().map(|m| row(a, m)).collect(),
                selected,
            }
        }
        other => AllegianceView {
            loaded: other.is_some(),
            name: None,
            rank: 0,
            total_members: 0,
            total_vassals: 0,
            motd: String::new(),
            motd_set_by: String::new(),
            i_am_monarch: false,
            my_officer: 0,
            monarch: None,
            patron: None,
            me: None,
            vassals: Vec::new(),
            selected,
        },
    }
}

/// `12.3k` style XP text.
pub fn xp_text(xp: u64) -> String {
    match xp {
        0 => "-".into(),
        x if x < 10_000 => x.to_string(),
        x if x < 10_000_000 => format!("{:.1}k", x as f64 / 1000.0),
        x => format!("{:.1}M", x as f64 / 1_000_000.0),
    }
}

/// The columns of one member line: role, name, level, rank, loyalty,
/// leadership, XP passed up (tithed) and waiting (cached), online.
pub fn member_columns(role: &str, m: &MemberRow) -> [String; 8] {
    let name = if m.officer > 0 {
        format!("{} ({})", m.name, OFFICER_TITLES[m.officer.min(3) as usize])
    } else {
        m.name.clone()
    };
    [
        role.to_string(),
        name,
        m.level.to_string(),
        m.rank.to_string(),
        m.loyalty.to_string(),
        m.leadership.to_string(),
        if !m.may_passup && !role.starts_with("Monarch") {
            "no passup".to_string()
        } else {
            format!("{} / {}", xp_text(m.xp_tithed), xp_text(m.xp_cached))
        },
        if m.online { "online" } else { "" }.to_string(),
    ]
}

pub fn draw(
    egui: &egui::Context,
    v: &AllegianceView,
    new_name: &mut String,
    new_motd: &mut String,
) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "allegiance",
        // Top centre, where the options and vendor windows also go; the
        // fellowship window sits to its right below the radar.
        egui::pos2(w * 0.5 - 280.0, 60.0),
        egui::vec2(560.0, 260.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(544.0, 244.0));
        match &v.name {
            None => {
                title(ui, "Allegiance");
                ui.label(
                    egui::RichText::new(if v.loaded {
                        "Not in an allegiance. Select a player near you and swear to them; \
                         they must accept allegiance requests and answer yes."
                    } else {
                        "Waiting for the server..."
                    })
                    .color(egui::Color32::from_gray(180))
                    .small(),
                );
            }
            Some(name) => {
                title(
                    ui,
                    format!(
                        "{name}: {} members, {} below you, rank {}",
                        v.total_members, v.total_vassals, v.rank
                    ),
                );
                if !v.motd.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("\"{}\" -- {}", v.motd, v.motd_set_by))
                            .color(egui::Color32::from_rgb(220, 200, 150))
                            .small(),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(130.0)
                    .show(ui, |ui| {
                        egui::Grid::new("allegiance_members")
                            .num_columns(9)
                            .spacing([10.0, 2.0])
                            .show(ui, |ui| {
                                for h in [
                                    "",
                                    "Name",
                                    "Lvl",
                                    "Rank",
                                    "Loy",
                                    "Lead",
                                    "XP up / waiting",
                                    "",
                                    "",
                                ] {
                                    caption(ui, h);
                                }
                                ui.end_row();
                                let mut lines: Vec<(&str, &MemberRow, bool)> = Vec::new();
                                // One line per member: the monarch's doubles
                                // as the patron's when they are the same.
                                let patron_is_monarch = match (&v.patron, &v.monarch) {
                                    (Some(p), Some(k)) => p.guid == k.guid,
                                    _ => false,
                                };
                                if let Some(m) = &v.monarch {
                                    if patron_is_monarch {
                                        lines.push(("Monarch, patron", m, true));
                                    } else {
                                        lines.push(("Monarch", m, false));
                                    }
                                }
                                if let (Some(m), false) = (&v.patron, patron_is_monarch) {
                                    lines.push(("Patron", m, true));
                                }
                                if let Some(m) = &v.me {
                                    if !v.i_am_monarch {
                                        lines.push(("You", m, false));
                                    }
                                }
                                for m in &v.vassals {
                                    lines.push(("Vassal", m, true));
                                }
                                for (role, m, breakable) in lines {
                                    let color = match role {
                                        "Monarch" | "Monarch, patron" => {
                                            egui::Color32::from_rgb(255, 215, 120)
                                        }
                                        "You" => egui::Color32::from_rgb(180, 230, 180),
                                        _ => egui::Color32::WHITE,
                                    };
                                    for col in member_columns(role, m) {
                                        ui.label(egui::RichText::new(col).color(color));
                                    }
                                    if breakable {
                                        if ui.small_button("Break").clicked() {
                                            actions.break_with = Some(m.guid);
                                        }
                                    } else {
                                        ui.label("");
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        }
        ui.horizontal(|ui| {
            if let (Some((g, n)), true) = (&v.selected, v.patron.is_none()) {
                if ui.button(format!("Swear to {n}")).clicked() {
                    actions.swear = Some(*g);
                }
            }
            if ui.button("Refresh").clicked() {
                actions.refresh = true;
            }
        });
        if v.i_am_monarch {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(new_name).desired_width(160.0));
                if ui.button("Set").clicked() {
                    actions.set_name = Some(new_name.trim().to_string());
                }
            });
        }
        if v.my_officer >= 2 {
            ui.horizontal(|ui| {
                ui.label("Motd");
                ui.add(egui::TextEdit::singleline(new_motd).desired_width(260.0));
                if ui.button("Set").clicked() {
                    actions.set_motd = Some(new_motd.trim().to_string());
                }
            });
        }
    });
    actions
}

pub struct Allegiance {
    source: Source<AllegianceView>,
    pub show: bool,
    new_name: String,
    new_motd: String,
}

impl Default for Allegiance {
    fn default() -> Self {
        Allegiance {
            source: Source::Live,
            show: false,
            new_name: String::new(),
            new_motd: String::new(),
        }
    }
}

impl Allegiance {
    pub fn demo() -> Self {
        let m = |guid, name: &str, level, rank, online, tithed, cached, officer| MemberRow {
            guid,
            name: name.into(),
            level,
            rank,
            loyalty: 120,
            leadership: 80,
            online,
            xp_cached: cached,
            xp_tithed: tithed,
            may_passup: true,
            officer,
        };
        Allegiance {
            source: Source::Demo(AllegianceView {
                loaded: true,
                name: Some("Demo Realm".into()),
                rank: 2,
                total_members: 5,
                total_vassals: 2,
                motd: "Hunt in pairs.".into(),
                motd_set_by: "King Demo".into(),
                i_am_monarch: false,
                my_officer: 0,
                monarch: Some(m(1, "King Demo", 60, 4, true, 0, 812_000, 0)),
                patron: Some(m(2, "Patron Demo", 30, 3, false, 40_000, 15_500, 2)),
                me: Some(m(3, "Demo", 12, 2, true, 3_200, 900, 0)),
                vassals: vec![
                    m(4, "Reborn", 3, 1, true, 400, 0, 0),
                    m(5, "Test Mage", 1, 1, false, 0, 0, 0),
                ],
                selected: None,
            }),
            show: true,
            new_name: String::new(),
            new_motd: String::new(),
        }
    }
}

impl Plugin for Allegiance {
    fn name(&self) -> &str {
        "allegiance"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("allegiance.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("allegiance.show", self.show);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(v) = v else { return };
        let a = draw(egui, &v, &mut self.new_name, &mut self.new_motd);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some(g) = a.swear {
                c.swear_allegiance(g);
            }
            if let Some(g) = a.break_with {
                c.break_allegiance(g);
            }
            if a.refresh {
                c.allegiance_update_request(true);
            }
            if let Some(n) = a.set_name {
                c.set_allegiance_name(&n);
                self.new_name.clear();
            }
            if let Some(m) = a.set_motd {
                c.set_allegiance_motd(&m);
                self.new_motd.clear();
            }
        }
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::L && pressed {
            self.show = !self.show;
            if self.show {
                if let Some(c) = cx.try_client() {
                    c.allegiance_update_request(true);
                }
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xp_and_columns_format() {
        assert_eq!(xp_text(0), "-");
        assert_eq!(xp_text(999), "999");
        assert_eq!(xp_text(15_500), "15.5k");
        assert_eq!(xp_text(812_000_000), "812.0M");
        let m = MemberRow {
            guid: 2,
            name: "Patron Demo".into(),
            level: 30,
            rank: 3,
            loyalty: 120,
            leadership: 80,
            online: true,
            xp_cached: 15_500,
            xp_tithed: 40_000,
            may_passup: true,
            officer: 2,
        };
        let cols = member_columns("Patron", &m);
        assert_eq!(cols[1], "Patron Demo (Seneschal)");
        assert_eq!(cols[6], "40.0k / 15.5k");
        assert_eq!(cols[7], "online");
        let stuck = MemberRow {
            may_passup: false,
            ..m
        };
        assert_eq!(member_columns("Vassal", &stuck)[6], "no passup");
        assert_eq!(member_columns("Monarch", &stuck)[6], "40.0k / 15.5k");
    }
}
