//! The book window: what a used book, sign or plaque says. Opens when
//! the server sends the book's data, shows one page at a time with
//! Prev/Next (pages the server did not include are asked for when
//! turned to), the title and the scribe. Close hides it until the next
//! book.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookView {
    pub title: String,
    pub author: String,
    /// Page texts; None until read.
    pub pages: Vec<Option<String>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub close: bool,
    /// Turned to a page whose text is not here yet.
    pub want_page: Option<u32>,
}

pub fn view(c: &Client) -> Option<BookView> {
    let b = c.book.as_ref()?;
    let name = c
        .world
        .objects
        .get(&b.guid)
        .map(|o| o.name.clone())
        .unwrap_or_default();
    Some(BookView {
        title: if b.inscription.is_empty() {
            name
        } else {
            b.inscription.clone()
        },
        author: b.author.clone(),
        pages: b.pages.iter().map(|p| p.text.clone()).collect(),
    })
}

pub fn draw(egui: &egui::Context, v: &BookView, page: &mut usize) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    let n = v.pages.len();
    if *page >= n {
        *page = n.saturating_sub(1);
    }
    window(
        "book",
        egui::pos2(rect.width() * 0.5 - 230.0, rect.height() * 0.2),
        egui::vec2(460.0, 360.0),
        200,
        10,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(440.0, 340.0));
        ui.horizontal(|ui| {
            title(ui, &v.title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Close").clicked() {
                    actions.close = true;
                }
            });
        });
        if !v.author.is_empty() {
            caption(ui, format!("by {}", v.author));
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                ui.set_min_width(420.0);
                match v.pages.get(*page) {
                    Some(Some(text)) if !text.is_empty() => {
                        ui.label(
                            egui::RichText::new(text)
                                .color(egui::Color32::from_rgb(235, 225, 200))
                                .size(15.0),
                        );
                    }
                    Some(Some(_)) => {
                        caption(ui, "(blank page)");
                    }
                    Some(None) => {
                        caption(ui, "(reading...)");
                        actions.want_page = Some(*page as u32);
                    }
                    None => {
                        caption(ui, "(no pages)");
                    }
                }
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(*page > 0, egui::Button::new("Prev"))
                .clicked()
            {
                *page -= 1;
            }
            caption(
                ui,
                format!("page {} of {}", (*page + 1).min(n.max(1)), n.max(1)),
            );
            if ui
                .add_enabled(*page + 1 < n, egui::Button::new("Next"))
                .clicked()
            {
                *page += 1;
            }
        });
    });
    actions
}

#[derive(Default)]
pub struct Book {
    source: Source<BookView>,
    page: usize,
    seen: u64,
    closed: bool,
    /// The page last asked for, so a missing page is requested once.
    asked: Option<u32>,
}

impl Book {
    pub fn demo() -> Self {
        Book {
            source: Source::Demo(BookView {
                title: "BASICS OF MAGIC".into(),
                author: "Asheron".into(),
                pages: vec![
                    Some("Magic in Dereth is drawn from the four schools: War, Life, Creature and Item. Each spell needs its components and a focus.".into()),
                    None,
                ],
            }),
            page: 0,
            seen: 0,
            closed: false,
            asked: None,
        }
    }
}

impl Plugin for Book {
    fn name(&self) -> &str {
        "book"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => {
                let Some(c) = cx.try_client() else { return };
                if c.book_seq != self.seen {
                    let new_book = c.book.as_ref().map(|b| b.guid);
                    if self.seen == 0 || new_book.is_some() {
                        // A new book (or the first page of one): show it.
                        self.closed = false;
                    }
                    self.seen = c.book_seq;
                    self.asked = None;
                }
                if self.closed {
                    return;
                }
                view(c)
            }
        };
        let Some(v) = v else { return };
        let a = draw(egui, &v, &mut self.page);
        if a.close {
            self.closed = true;
            self.page = 0;
        }
        if let (Some(p), Source::Live, Some(c)) = (a.want_page, &self.source, cx.try_client()) {
            if self.asked != Some(p) {
                self.asked = Some(p);
                if let Some(guid) = c.book.as_ref().map(|b| b.guid) {
                    c.read_page(guid, p);
                }
            }
        }
    }
}
