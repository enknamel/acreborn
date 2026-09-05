//! Questions from the server (a fellowship invitation, swearing
//! allegiance, a crafting check...) as Yes/No popups. Each is answered
//! with `Client::confirm`; unanswered ones time out server-side.

use super::{title, Source};
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub kind: u32,
    pub context: u32,
    pub text: String,
}

/// A short label for the popup title by ACE `ConfirmationType`.
pub fn kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "Allegiance",
        2 => "Skill",
        3 => "Attribute",
        4 => "Fellowship",
        5 => "Crafting",
        6 => "Augmentation",
        _ => "Question",
    }
}

/// The text to show: ACE sends only the other player's name for an
/// allegiance oath (kind 1); the client wrote the sentence.
pub fn question_text(q: &Question) -> String {
    match q.kind {
        1 => format!(
            "{} wants to swear allegiance to you. Accept the oath?",
            q.text
        ),
        _ => q.text.clone(),
    }
}

pub fn view(c: &Client) -> Vec<Question> {
    c.world
        .confirmations
        .iter()
        .map(|q| Question {
            kind: q.kind,
            context: q.context,
            text: q.text.clone(),
        })
        .collect()
}

/// Returns (kind, context, answer) for each popup answered this frame.
pub fn draw(egui: &egui::Context, questions: &[Question]) -> Vec<(u32, u32, bool)> {
    let mut answers = Vec::new();
    let rect = egui.viewport_rect();
    for (i, q) in questions.iter().enumerate() {
        egui::Window::new(format!("confirm_{}_{}", q.kind, q.context))
            .title_bar(false)
            .resizable(false)
            .fade_in(false)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(220))
                    .inner_margin(10),
            )
            .fixed_pos(egui::pos2(
                rect.width() * 0.5 - 180.0,
                rect.height() * 0.35 + i as f32 * 110.0,
            ))
            .fixed_size(egui::vec2(360.0, 90.0))
            .show(egui, |ui| {
                title(ui, kind_name(q.kind));
                ui.label(egui::RichText::new(question_text(q)).color(egui::Color32::WHITE));
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answers.push((q.kind, q.context, true));
                    }
                    if ui.button("No").clicked() {
                        answers.push((q.kind, q.context, false));
                    }
                });
            });
    }
    answers
}

#[derive(Default)]
pub struct Confirm {
    source: Source<Vec<Question>>,
}

impl Confirm {
    pub fn demo() -> Self {
        Confirm {
            source: Source::Demo(vec![Question {
                kind: 4,
                context: 1,
                text: "Demo would like you to join the fellowship Demo Fellows.".into(),
            }]),
        }
    }
}

impl Plugin for Confirm {
    fn name(&self) -> &str {
        "confirm"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let qs = match &self.source {
            Source::Demo(d) => d.clone(),
            Source::Live => cx.try_client().map(|c| view(c)).unwrap_or_default(),
        };
        if qs.is_empty() {
            return;
        }
        let answers = draw(egui, &qs);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for (kind, context, yes) in answers {
                c.confirm(kind, context, yes);
            }
        }
    }
}
