//! Character creation: a [`CharacterBuild`] edited over five steps
//! (heritage and sex, appearance, attributes, skills, name and town) with
//! the rules from `ac_client::creation`. The 3D preview of the look is
//! drawn by the host (acviewer reads [`CreateState::build`]).

use std::rc::Rc;

use ac_client::creation::{
    self, valid_name, CharacterBuild, CreateError, Rules, SkillChoice, ATTRIBUTE_MAX,
    ATTRIBUTE_MIN, ATTRIBUTE_NAMES,
};
use ac_formats::chargen::{CharGen, SexCg};
use ac_scene::Assets;

use crate::egui;
use crate::panels::{caption, frame, title};

/// The panes of the creation screen, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Heritage,
    Appearance,
    Attributes,
    Skills,
    Finish,
}

impl Step {
    pub const ALL: [Step; 5] = [
        Step::Heritage,
        Step::Appearance,
        Step::Attributes,
        Step::Skills,
        Step::Finish,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Step::Heritage => "Heritage",
            Step::Appearance => "Appearance",
            Step::Attributes => "Attributes",
            Step::Skills => "Skills",
            Step::Finish => "Name & town",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The next step; the last stays put.
    pub fn next(self) -> Step {
        Step::ALL[(self.index() + 1).min(Step::ALL.len() - 1)]
    }

    /// The previous step; the first stays put.
    pub fn prev(self) -> Step {
        Step::ALL[self.index().saturating_sub(1)]
    }
}

/// One skill line of the skills pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillLine {
    pub id: u32,
    pub name: String,
    pub choice: SkillChoice,
    /// Credits to train, and to specialize (the total).
    pub train_cost: u32,
    pub specialize_cost: u32,
    pub can_specialize: bool,
    /// Works at its formula value while untrained (else untraining it is
    /// the same as dropping it).
    pub usable_untrained: bool,
    /// The lowest choice the rules allow (the sheet's default).
    pub min: SkillChoice,
    /// Always trained: cannot be lowered.
    pub locked: bool,
}

/// The skills pane, grouped by choice, each group sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillGroups {
    pub specialized: Vec<SkillLine>,
    pub trained: Vec<SkillLine>,
    pub untrained: Vec<SkillLine>,
    pub unusable: Vec<SkillLine>,
}

impl SkillGroups {
    /// `(heading, lines)` in display order.
    pub fn sections(&self) -> [(&'static str, &[SkillLine]); 4] {
        [
            ("Specialized", &self.specialized),
            ("Trained", &self.trained),
            ("Untrained", &self.untrained),
            ("Unusable", &self.unusable),
        ]
    }
}

/// Group every skill of the rules by the build's choice for it.
pub fn group_skills(build: &CharacterBuild, rules: &Rules) -> SkillGroups {
    let mut g = SkillGroups::default();
    for r in &rules.skills {
        let choice = build.skill(r.skill);
        let line = SkillLine {
            id: r.skill,
            name: r.name.clone(),
            choice,
            train_cost: CharacterBuild::skill_cost(rules, r.skill, SkillChoice::Trained),
            specialize_cost: CharacterBuild::skill_cost(rules, r.skill, SkillChoice::Specialized),
            can_specialize: r.can_specialize,
            usable_untrained: r.usable_untrained,
            min: r.default,
            locked: creation::ALWAYS_TRAINED.contains(&r.skill),
        };
        match choice {
            SkillChoice::Specialized => g.specialized.push(line),
            SkillChoice::Trained => g.trained.push(line),
            SkillChoice::Untrained => g.untrained.push(line),
            SkillChoice::Unusable => g.unusable.push(line),
        }
    }
    for list in [
        &mut g.specialized,
        &mut g.trained,
        &mut g.untrained,
        &mut g.unusable,
    ] {
        list.sort_by(|a, b| a.name.cmp(&b.name));
    }
    g
}

/// White with points to spare, dim at zero, red when over budget.
pub fn points_color(left: i64) -> egui::Color32 {
    if left < 0 {
        egui::Color32::from_rgb(255, 90, 90)
    } else if left == 0 {
        egui::Color32::from_gray(170)
    } else {
        egui::Color32::from_rgb(160, 255, 160)
    }
}

/// `idx + delta` wrapped into `0..count`.
pub fn cycle(idx: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    (idx as i64 + delta as i64).rem_euclid(count as i64) as usize
}

/// Heritage groups to offer: `(id, name)`, without the Olthoi variants
/// unless `show_all`.
pub fn heritage_choices(cg: &CharGen, show_all: bool) -> Vec<(u32, String)> {
    cg.heritage_groups
        .iter()
        .filter(|(_, h)| show_all || !h.name.to_ascii_lowercase().contains("olthoi"))
        .map(|(id, h)| (*id, h.name.clone()))
        .collect()
}

/// What a validation error means to the person at the keyboard.
pub fn describe_error(e: &CreateError) -> String {
    match e {
        CreateError::AttributeOutOfRange(i) => format!(
            "{} must be {ATTRIBUTE_MIN}..{ATTRIBUTE_MAX}",
            ATTRIBUTE_NAMES.get(*i).copied().unwrap_or("An attribute")
        ),
        CreateError::TooManyAttributePoints { used, allowed } => {
            format!(
                "{} attribute points over the {allowed} allowed",
                used - allowed
            )
        }
        CreateError::TooManySkillCredits { used, allowed } => {
            format!(
                "{} skill credits over the {allowed} allowed",
                used - allowed
            )
        }
        CreateError::SkillNotAllowed(id) => format!("Skill {id} cannot take that choice"),
        CreateError::InvalidName => {
            "Name: 3 to 32 letters, spaces, hyphens and apostrophes".to_string()
        }
        CreateError::InvalidStartArea => "Pick a starting town".to_string(),
        CreateError::UnknownHeritage => "Unknown heritage".to_string(),
    }
}

/// `1` male, `2` female, as the CharGen table names them.
pub fn sex_of(cg: &CharGen, heritage: u32, gender: u32) -> Option<&SexCg> {
    creation::heritage(cg, heritage)?
        .genders
        .iter()
        .find(|(g, _)| *g == gender as i32)
        .map(|(_, s)| s)
}

/// Position of a palette id in a colour list (0 when absent).
pub fn color_index(list: &[u32], value: u32) -> usize {
    list.iter().position(|v| *v == value).unwrap_or(0)
}

/// What the creation screen asked the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateAction {
    /// Send `build` to the server.
    Create,
    /// Back to the select screen.
    Cancel,
}

/// The creation screen's state.
pub struct CreateState {
    pub assets: Rc<Assets>,
    pub cg: Rc<CharGen>,
    pub build: CharacterBuild,
    pub rules: Rules,
    pub step: Step,
    pub show_all_heritages: bool,
    /// A refusal, from the rules or the server.
    pub message: Option<String>,
    /// Create was sent; waiting for the server's answer.
    pub pending: bool,
}

impl CreateState {
    /// The screen on `heritage` and `gender` (1 male, 2 female), first
    /// template, home town, unnamed.
    pub fn new(assets: Rc<Assets>, heritage: u32, gender: u32) -> Result<Self, CreateError> {
        let cg = assets.chargen().map_err(|_| CreateError::UnknownHeritage)?;
        let build = CharacterBuild::new(&assets, heritage, gender)?;
        let rules = creation::rules(&assets, heritage)?;
        Ok(CreateState {
            assets,
            cg,
            build,
            rules,
            step: Step::Heritage,
            show_all_heritages: false,
            message: None,
            pending: false,
        })
    }

    /// Start over on another heritage or sex, keeping the name.
    pub fn reset(&mut self, heritage: u32, gender: u32) {
        match (
            CharacterBuild::new(&self.assets, heritage, gender),
            creation::rules(&self.assets, heritage),
        ) {
            (Ok(mut b), Ok(r)) => {
                b.name = std::mem::take(&mut self.build.name);
                self.build = b;
                self.rules = r;
                self.message = None;
            }
            (Err(e), _) | (_, Err(e)) => self.message = Some(describe_error(&e)),
        }
    }

    pub fn apply_template(&mut self, index: usize) {
        let cg = self.cg.clone();
        self.build.apply_template(&cg, &self.rules, index);
    }

    /// Change a skill, keeping the refusal as the message.
    pub fn set_skill(&mut self, skill: u32, choice: SkillChoice) {
        match self.build.set_skill(&self.rules, skill, choice) {
            Ok(()) => self.message = None,
            Err(CreateError::SkillNotAllowed(_)) => {
                let name = self
                    .rules
                    .skills
                    .iter()
                    .find(|r| r.skill == skill)
                    .map(|r| r.name.as_str())
                    .unwrap_or("That skill");
                self.message = Some(match choice {
                    SkillChoice::Specialized => {
                        format!("{name} cannot be specialized at creation")
                    }
                    _ if creation::ALWAYS_TRAINED.contains(&skill) => {
                        format!("{name} is always trained")
                    }
                    _ => format!("{name} cannot go below its default"),
                });
            }
            Err(e) => self.message = Some(describe_error(&e)),
        }
    }

    /// The first thing wrong with the build, if anything.
    pub fn problem(&self) -> Option<String> {
        self.build
            .validate(&self.rules)
            .err()
            .map(|e| describe_error(&e))
    }

    fn sex(&self) -> Option<&SexCg> {
        sex_of(&self.cg, self.build.look.heritage, self.build.look.gender)
    }

    /// The heritage's name.
    pub fn heritage_name(&self) -> &str {
        &self.rules.heritage_name
    }
}

/// `label  < n / count >`; returns the new index when clicked.
fn stepper(ui: &mut egui::Ui, label: &str, idx: usize, count: usize) -> Option<usize> {
    let mut out = None;
    ui.label(label);
    ui.horizontal(|ui| {
        if ui.small_button("<").clicked() {
            out = Some(cycle(idx, count, -1));
        }
        ui.label(
            egui::RichText::new(format!(
                "{} / {}",
                idx.min(count.saturating_sub(1)) + 1,
                count
            ))
            .color(egui::Color32::WHITE),
        );
        if ui.small_button(">").clicked() {
            out = Some(cycle(idx, count, 1));
        }
    });
    ui.end_row();
    out
}

fn draw_heritage(ui: &mut egui::Ui, st: &mut CreateState) {
    let cg = st.cg.clone();
    ui.checkbox(&mut st.show_all_heritages, "Show all (Olthoi)");
    ui.add_space(4.0);
    caption(ui, "Heritage");
    let mut pick: Option<u32> = None;
    for (id, name) in heritage_choices(&cg, st.show_all_heritages) {
        let on = id == st.build.look.heritage;
        if ui.selectable_label(on, name).clicked() && !on {
            pick = Some(id);
        }
    }
    if let Some(id) = pick {
        // Keep the sex when the new heritage has it, else its first.
        let gender = sex_of(&cg, id, st.build.look.gender)
            .map(|_| st.build.look.gender)
            .or_else(|| {
                creation::heritage(&cg, id).and_then(|h| h.genders.first().map(|(g, _)| *g as u32))
            })
            .unwrap_or(1);
        st.reset(id, gender);
    }
    ui.add_space(6.0);
    caption(ui, "Sex");
    let genders: Vec<(u32, String)> = creation::heritage(&cg, st.build.look.heritage)
        .map(|h| {
            h.genders
                .iter()
                .map(|(g, s)| (*g as u32, s.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let mut pick: Option<u32> = None;
    ui.horizontal(|ui| {
        for (g, name) in &genders {
            let on = *g == st.build.look.gender;
            if ui.selectable_label(on, name).clicked() && !on {
                pick = Some(*g);
            }
        }
    });
    if let Some(g) = pick {
        let h = st.build.look.heritage;
        st.reset(h, g);
    }
    ui.add_space(6.0);
    caption(
        ui,
        format!(
            "{}: {} attribute points, {} skill credits, starts in {}",
            st.rules.heritage_name,
            st.rules.attribute_credits,
            st.rules.skill_credits,
            st.rules
                .start_area_names
                .first()
                .cloned()
                .unwrap_or_default()
        ),
    );
}

fn draw_appearance(ui: &mut egui::Ui, st: &mut CreateState) {
    let Some(sex) = st.sex().cloned() else {
        ui.label("No appearance options for this heritage and sex.");
        return;
    };
    let look = &mut st.build.look;
    caption(ui, "Face");
    egui::Grid::new("face_grid")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            if let Some(i) = stepper(ui, "Hair style", look.hair_style, sex.hair_styles.len()) {
                look.hair_style = i;
            }
            if let Some(i) = stepper(ui, "Hair colour", look.hair_color, sex.hair_colors.len()) {
                look.hair_color = i;
            }
            ui.label("Hair shade");
            ui.add(egui::Slider::new(&mut look.hair_shade, 0.0..=1.0).show_value(false));
            ui.end_row();
            if let Some(i) = stepper(ui, "Eyes", look.eyes, sex.eye_strips.len()) {
                look.eyes = i;
            }
            if let Some(i) = stepper(ui, "Eye colour", look.eye_color, sex.eye_colors.len()) {
                look.eye_color = i;
            }
            if let Some(i) = stepper(ui, "Nose", look.nose, sex.nose_strips.len()) {
                look.nose = i;
            }
            if let Some(i) = stepper(ui, "Mouth", look.mouth, sex.mouth_strips.len()) {
                look.mouth = i;
            }
            ui.label("Skin shade");
            ui.add(egui::Slider::new(&mut look.skin_shade, 0.0..=1.0).show_value(false));
            ui.end_row();
        });
    ui.add_space(6.0);
    caption(ui, "Clothing (not in the preview)");
    let colors = sex.clothing_colors.clone();
    egui::Grid::new("clothing_grid")
        .num_columns(4)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            for h in ["Item", "Style", "Colour", "Shade"] {
                caption(ui, h);
            }
            ui.end_row();
            let gear = |ui: &mut egui::Ui,
                        label: &str,
                        list: &[ac_formats::chargen::GearCg],
                        slot: &mut (u32, u32, f64),
                        optional: bool| {
                ui.label(label);
                // With `optional`, choice 0 is "none" (style u32::MAX).
                let count = list.len() + optional as usize;
                let cur = if slot.0 == u32::MAX {
                    0
                } else {
                    slot.0 as usize + optional as usize
                };
                ui.horizontal(|ui| {
                    let mut next: Option<usize> = None;
                    if ui.small_button("<").clicked() {
                        next = Some(cycle(cur, count, -1));
                    }
                    let name = if optional && cur == 0 {
                        "none".to_string()
                    } else {
                        list.get(cur - optional as usize)
                            .map(|g| g.name.clone())
                            .unwrap_or_else(|| "?".into())
                    };
                    ui.label(egui::RichText::new(name).color(egui::Color32::WHITE));
                    if ui.small_button(">").clicked() {
                        next = Some(cycle(cur, count, 1));
                    }
                    if let Some(n) = next {
                        slot.0 = if optional && n == 0 {
                            u32::MAX
                        } else {
                            (n - optional as usize) as u32
                        };
                    }
                });
                let ci = color_index(&colors, slot.1);
                ui.horizontal(|ui| {
                    if ui.small_button("<").clicked() {
                        slot.1 = colors
                            .get(cycle(ci, colors.len(), -1))
                            .copied()
                            .unwrap_or(0);
                    }
                    ui.label(
                        egui::RichText::new(format!("{} / {}", ci + 1, colors.len().max(1)))
                            .color(egui::Color32::WHITE),
                    );
                    if ui.small_button(">").clicked() {
                        slot.1 = colors.get(cycle(ci, colors.len(), 1)).copied().unwrap_or(0);
                    }
                });
                let mut shade = slot.2 as f32;
                if ui
                    .add(egui::Slider::new(&mut shade, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    slot.2 = shade as f64;
                }
                ui.end_row();
            };
            gear(ui, "Headgear", &sex.headgear, &mut st.build.headgear, true);
            gear(ui, "Shirt", &sex.shirts, &mut st.build.shirt, false);
            gear(ui, "Pants", &sex.pants, &mut st.build.pants, false);
            gear(ui, "Footwear", &sex.footwear, &mut st.build.footwear, false);
        });
}

fn draw_template(ui: &mut egui::Ui, st: &mut CreateState) {
    ui.horizontal(|ui| {
        ui.label("Template");
        let current = st
            .rules
            .templates
            .get(st.build.template)
            .cloned()
            .unwrap_or_else(|| "custom".into());
        let mut pick = None;
        egui::ComboBox::from_id_salt("template")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, name) in st.rules.templates.iter().enumerate() {
                    if ui.selectable_label(i == st.build.template, name).clicked() {
                        pick = Some(i);
                    }
                }
            });
        if let Some(i) = pick {
            st.apply_template(i);
        }
        caption(ui, "presets the attributes and skills");
    });
}

fn draw_attributes(ui: &mut egui::Ui, st: &mut CreateState) {
    draw_template(ui, st);
    ui.add_space(6.0);
    let left = st.build.attribute_points_left(&st.rules);
    ui.label(
        egui::RichText::new(format!(
            "Points left: {left}   ({} of {} spent)",
            st.build.attribute_points_used(),
            st.rules.attribute_credits
        ))
        .color(points_color(left))
        .strong(),
    );
    egui::Grid::new("attr_grid")
        .num_columns(6)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (i, name) in ATTRIBUTE_NAMES.iter().enumerate() {
                let cur = st.build.attributes[i];
                ui.label(egui::RichText::new(*name).color(egui::Color32::WHITE));
                let mut want = cur;
                if ui.small_button("-10").clicked() {
                    want = cur.saturating_sub(10);
                }
                if ui.small_button("-1").clicked() {
                    want = cur.saturating_sub(1);
                }
                let mut drag = cur;
                ui.add(
                    egui::DragValue::new(&mut drag)
                        .range(ATTRIBUTE_MIN..=ATTRIBUTE_MAX)
                        .speed(0.2),
                );
                if drag != cur {
                    want = drag;
                }
                if ui.small_button("+1").clicked() {
                    want = cur + 1;
                }
                if ui.small_button("+10").clicked() {
                    want = cur + 10;
                }
                if want != cur {
                    st.build.set_attribute(i, want);
                }
                ui.end_row();
            }
        });
    caption(
        ui,
        format!("Each attribute is {ATTRIBUTE_MIN}..{ATTRIBUTE_MAX}; drag the number or use the buttons."),
    );
}

fn draw_skills(ui: &mut egui::Ui, st: &mut CreateState) {
    draw_template(ui, st);
    ui.add_space(6.0);
    let left = st.build.credits_left(&st.rules);
    ui.label(
        egui::RichText::new(format!(
            "Credits left: {left}   ({} of {} spent)",
            st.build.credits_used(&st.rules),
            st.rules.skill_credits
        ))
        .color(points_color(left))
        .strong(),
    );
    let groups = group_skills(&st.build, &st.rules);
    let mut change: Option<(u32, SkillChoice)> = None;
    for (heading, lines) in groups.sections() {
        if lines.is_empty() {
            continue;
        }
        ui.add_space(4.0);
        caption(ui, format!("{heading} ({})", lines.len()));
        egui::Grid::new(format!("skills_{heading}"))
            .num_columns(3)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                for l in lines {
                    let color = match l.choice {
                        SkillChoice::Specialized => egui::Color32::from_rgb(255, 215, 120),
                        SkillChoice::Trained => egui::Color32::WHITE,
                        SkillChoice::Untrained => egui::Color32::from_gray(170),
                        SkillChoice::Unusable => egui::Color32::from_gray(120),
                    };
                    ui.label(egui::RichText::new(&l.name).color(color));
                    let mut cost = if l.locked {
                        "free".to_string()
                    } else if l.can_specialize {
                        format!("train {}, spec {}", l.train_cost, l.specialize_cost)
                    } else {
                        format!("train {}", l.train_cost)
                    };
                    if !l.usable_untrained && !l.locked {
                        cost += ", needs training";
                    }
                    caption(ui, cost);
                    ui.horizontal(|ui| {
                        let mut btn = |label: &str, to: SkillChoice, show: bool| {
                            if show && ui.small_button(label).clicked() {
                                change = Some((l.id, to));
                            }
                        };
                        btn(
                            "Specialize",
                            SkillChoice::Specialized,
                            l.can_specialize && l.choice != SkillChoice::Specialized,
                        );
                        btn(
                            "Train",
                            SkillChoice::Trained,
                            l.choice != SkillChoice::Trained,
                        );
                        btn(
                            "Untrain",
                            SkillChoice::Untrained,
                            l.min <= SkillChoice::Untrained
                                && l.usable_untrained
                                && l.choice != SkillChoice::Untrained,
                        );
                        btn(
                            "Drop",
                            SkillChoice::Unusable,
                            l.min == SkillChoice::Unusable && l.choice != SkillChoice::Unusable,
                        );
                    });
                    ui.end_row();
                }
            });
    }
    if let Some((id, choice)) = change {
        st.set_skill(id, choice);
    }
}

fn draw_finish(ui: &mut egui::Ui, st: &mut CreateState) -> bool {
    let mut create = false;
    ui.horizontal(|ui| {
        ui.label("Name");
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.build.name)
                .hint_text("3 to 32 letters")
                .desired_width(240.0),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            create = st.problem().is_none() && !st.pending;
        }
        if st.build.name.trim().is_empty() {
            caption(ui, "letters, spaces, hyphens and apostrophes");
        } else if valid_name(&st.build.name) {
            ui.label(egui::RichText::new("ok").color(egui::Color32::from_rgb(160, 255, 160)));
        } else {
            ui.label(
                egui::RichText::new("letters, spaces, hyphens and apostrophes; 3 to 32")
                    .color(egui::Color32::from_rgb(255, 120, 120)),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label("Starting town");
        let current = st
            .rules
            .start_areas
            .iter()
            .position(|a| *a == st.build.start_area)
            .and_then(|i| st.rules.start_area_names.get(i))
            .cloned()
            .unwrap_or_else(|| "?".into());
        egui::ComboBox::from_id_salt("start_area")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, name) in st.rules.start_area_names.iter().enumerate() {
                    let area = st.rules.start_areas[i];
                    if ui
                        .selectable_label(area == st.build.start_area, name)
                        .clicked()
                    {
                        st.build.start_area = area;
                    }
                }
            });
    });
    ui.add_space(6.0);
    caption(ui, "Summary");
    let sex = st.sex().map(|s| s.name.clone()).unwrap_or_default();
    let template = st
        .rules
        .templates
        .get(st.build.template)
        .cloned()
        .unwrap_or_else(|| "custom".into());
    ui.label(
        egui::RichText::new(format!("{sex} {} {template}", st.rules.heritage_name))
            .color(egui::Color32::WHITE),
    );
    let attrs: Vec<String> = ATTRIBUTE_NAMES
        .iter()
        .zip(st.build.attributes.iter())
        .map(|(n, v)| format!("{} {v}", &n[..3]))
        .collect();
    ui.label(egui::RichText::new(attrs.join("  ")).color(egui::Color32::from_gray(210)));
    let g = group_skills(&st.build, &st.rules);
    let names = |l: &[SkillLine]| {
        l.iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !g.specialized.is_empty() {
        ui.label(
            egui::RichText::new(format!("Specialized: {}", names(&g.specialized)))
                .color(egui::Color32::from_rgb(255, 215, 120)),
        );
    }
    ui.label(
        egui::RichText::new(format!("Trained: {}", names(&g.trained)))
            .color(egui::Color32::from_gray(210)),
    );
    create
}

/// Draw the creation screen; returns what the host should do.
pub fn draw(egui: &egui::Context, st: &mut CreateState) -> Vec<CreateAction> {
    let mut actions = Vec::new();
    egui::Window::new("character_create")
        .fade_in(false)
        .title_bar(false)
        .resizable(false)
        .frame(frame(200, 12))
        .fixed_pos(egui::pos2(8.0, 36.0))
        .fixed_size(egui::vec2(600.0, 500.0))
        .show(egui, |ui| {
            ui.set_min_size(egui::vec2(590.0, 488.0));
            ui.horizontal(|ui| {
                title(ui, "New character");
                ui.add_space(12.0);
                for s in Step::ALL {
                    if ui.selectable_label(s == st.step, s.label()).clicked() {
                        st.step = s;
                    }
                }
            });
            ui.separator();
            let mut create = false;
            egui::ScrollArea::vertical()
                .max_height(380.0)
                .min_scrolled_height(380.0)
                .show(ui, |ui| {
                    ui.set_min_size(egui::vec2(570.0, 380.0));
                    match st.step {
                        Step::Heritage => draw_heritage(ui, st),
                        Step::Appearance => draw_appearance(ui, st),
                        Step::Attributes => draw_attributes(ui, st),
                        Step::Skills => draw_skills(ui, st),
                        Step::Finish => create = draw_finish(ui, st),
                    }
                });
            ui.separator();
            let problem = st.problem();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(st.step != Step::Heritage, egui::Button::new("Back"))
                    .clicked()
                {
                    st.step = st.step.prev();
                }
                if ui
                    .add_enabled(st.step != Step::Finish, egui::Button::new("Next"))
                    .clicked()
                {
                    st.step = st.step.next();
                }
                let ready = problem.is_none() && !st.pending;
                if ui.add_enabled(ready, egui::Button::new("Create")).clicked() {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    actions.push(CreateAction::Cancel);
                }
                if st.pending {
                    ui.label(
                        egui::RichText::new("Creating...")
                            .color(egui::Color32::from_rgb(255, 220, 120)),
                    );
                } else if let Some(p) = &problem {
                    ui.label(egui::RichText::new(p).color(egui::Color32::from_rgb(255, 170, 120)));
                }
            });
            if let Some(m) = &st.message {
                ui.label(egui::RichText::new(m).color(egui::Color32::from_rgb(255, 120, 120)));
            }
            caption(
                ui,
                "Right-drag turns the preview; PageUp/PageDown or Left/Right change steps; Escape cancels",
            );
            if create && problem.is_none() && !st.pending {
                actions.push(CreateAction::Create);
            }
        });
    actions
}

/// A key while the creation screen is up: Left/PageUp and Right/PageDown
/// move between steps; Escape cancels. Returns the action, and whether
/// the key was used.
pub fn key(st: &mut CreateState, key: egui::Key) -> (Option<CreateAction>, bool) {
    match key {
        egui::Key::ArrowRight | egui::Key::PageDown => {
            st.step = st.step.next();
            (None, true)
        }
        egui::Key::ArrowLeft | egui::Key::PageUp => {
            st.step = st.step.prev();
            (None, true)
        }
        egui::Key::Escape => (Some(CreateAction::Cancel), true),
        _ => (None, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_client::creation::SkillRule;
    use ac_scene::chargen::Look;
    use std::collections::BTreeMap;

    fn rule(skill: u32, name: &str, train: u32, spec: u32, can_spec: bool) -> SkillRule {
        let always = creation::ALWAYS_TRAINED.contains(&skill);
        SkillRule {
            skill,
            name: name.into(),
            train_cost: if always { 0 } else { train },
            specialize_cost: spec,
            default: if always {
                SkillChoice::Trained
            } else {
                SkillChoice::Unusable
            },
            can_specialize: can_spec,
            usable_untrained: name != "Alchemy",
        }
    }

    fn rules() -> Rules {
        Rules {
            heritage: 1,
            heritage_name: "Aluvian".into(),
            attribute_credits: 330,
            skill_credits: 52,
            skills: vec![
                rule(6, "Melee Defense", 10, 6, true),
                rule(1, "Axe", 6, 6, true),
                rule(40, "Salvaging", 0, 0, false),
                rule(24, "Run", 0, 0, true),
                rule(31, "Alchemy", 4, 4, true),
            ],
            start_areas: vec![0, 1],
            start_area_names: vec!["Holtburg".into(), "Shoushi".into()],
            templates: vec!["Custom".into(), "Swordsman".into()],
        }
    }

    fn build() -> CharacterBuild {
        let mut skills = BTreeMap::new();
        skills.insert(40, SkillChoice::Trained);
        skills.insert(24, SkillChoice::Trained);
        skills.insert(1, SkillChoice::Specialized);
        skills.insert(6, SkillChoice::Trained);
        skills.insert(31, SkillChoice::Untrained);
        CharacterBuild {
            name: "Reborn".into(),
            look: Look::default(),
            headgear: (u32::MAX, 0, 0.0),
            shirt: (0, 0, 0.0),
            pants: (0, 0, 0.0),
            footwear: (0, 0, 0.0),
            template: 1,
            attributes: [100, 60, 60, 50, 30, 30],
            skills,
            start_area: 0,
        }
    }

    #[test]
    fn steps_walk_forward_and_back_and_stop_at_the_ends() {
        assert_eq!(Step::Heritage.prev(), Step::Heritage);
        assert_eq!(Step::Heritage.next(), Step::Appearance);
        assert_eq!(Step::Appearance.next(), Step::Attributes);
        assert_eq!(Step::Attributes.next(), Step::Skills);
        assert_eq!(Step::Skills.next(), Step::Finish);
        assert_eq!(Step::Finish.next(), Step::Finish);
        assert_eq!(Step::Finish.prev(), Step::Skills);
        let mut s = Step::Heritage;
        for _ in 0..10 {
            s = s.next();
        }
        assert_eq!(s, Step::Finish);
    }

    #[test]
    fn skills_group_by_choice_and_sort_by_name() {
        let g = group_skills(&build(), &rules());
        fn names(l: &[SkillLine]) -> Vec<&str> {
            l.iter().map(|s| s.name.as_str()).collect()
        }
        assert_eq!(names(&g.specialized), ["Axe"]);
        assert_eq!(names(&g.trained), ["Melee Defense", "Run", "Salvaging"]);
        assert_eq!(names(&g.untrained), ["Alchemy"]);
        assert!(g.unusable.is_empty());
        let axe = &g.specialized[0];
        assert_eq!((axe.train_cost, axe.specialize_cost), (6, 12));
        let run = g.trained.iter().find(|s| s.name == "Run").unwrap();
        assert!(run.locked && run.train_cost == 0);
        let salv = g.trained.iter().find(|s| s.name == "Salvaging").unwrap();
        assert!(!salv.can_specialize);
        assert!(!g.untrained[0].usable_untrained && axe.usable_untrained);
        assert_eq!(
            (run.min, axe.min),
            (SkillChoice::Trained, SkillChoice::Unusable)
        );
        // A skill the build never touched is unusable.
        let mut b = build();
        b.skills.remove(&31);
        let g = group_skills(&b, &rules());
        assert_eq!(names(&g.unusable), ["Alchemy"]);
    }

    #[test]
    fn points_left_reads_red_when_negative() {
        assert_eq!(points_color(-1), egui::Color32::from_rgb(255, 90, 90));
        assert_eq!(points_color(0), egui::Color32::from_gray(170));
        assert_eq!(points_color(12), egui::Color32::from_rgb(160, 255, 160));
        let mut b = build();
        assert_eq!(b.attribute_points_left(&rules()), 0);
        b.attributes[0] = 100;
        b.attributes[1] = 100;
        assert!(b.attribute_points_left(&rules()) < 0);
    }

    #[test]
    fn cycling_wraps() {
        assert_eq!(cycle(0, 5, -1), 4);
        assert_eq!(cycle(4, 5, 1), 0);
        assert_eq!(cycle(2, 5, 1), 3);
        assert_eq!(cycle(3, 0, 1), 0);
        assert_eq!(color_index(&[7, 9, 11], 11), 2);
        assert_eq!(color_index(&[7, 9, 11], 4), 0);
    }

    #[test]
    fn errors_are_explained() {
        assert_eq!(
            describe_error(&CreateError::TooManyAttributePoints {
                used: 340,
                allowed: 330
            }),
            "10 attribute points over the 330 allowed"
        );
        assert_eq!(
            describe_error(&CreateError::AttributeOutOfRange(4)),
            "Focus must be 10..100"
        );
        assert!(describe_error(&CreateError::InvalidName).contains("3 to 32"));
    }

    #[test]
    fn olthoi_hidden_unless_asked() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            eprintln!("AC_DATA_DIR unset; skipping");
            return;
        };
        let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
        let cg = assets.chargen().unwrap();
        let some = heritage_choices(&cg, false);
        let all = heritage_choices(&cg, true);
        assert!(some.iter().all(|(_, n)| !n.contains("Olthoi")));
        assert!(all.len() > some.len());
        assert!(some.iter().any(|(id, _)| *id == 1));
        let mut st = CreateState::new(Rc::new(assets), 1, 1).unwrap();
        assert_eq!(st.step, Step::Heritage);
        assert!(st.problem().is_some(), "unnamed builds are not valid");
        st.build.name = "Reborn".into();
        assert_eq!(st.problem(), None);
        assert!(st.sex().is_some());
        st.reset(3, 2);
        assert_eq!(st.build.look.heritage, 3);
        assert_eq!(st.build.name, "Reborn");
        assert_eq!(st.heritage_name(), "Sho");
    }
}
