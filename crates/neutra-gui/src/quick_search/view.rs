use super::Spotlight;
use crate::ui::widgets::*;
use egui::{Align, Frame, Key, Layout, Margin, RichText, Stroke, Ui, Vec2};

impl Spotlight {
    pub(super) fn show(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0_f32, LINE_STRONG))
            .corner_radius(12)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.search_field(ui);
                if !self.has_details() {
                    return;
                }
                ui.add_space(12.0);
                let heading = if self.state.error.is_some() {
                    "Search unavailable"
                } else {
                    "Matching files"
                };
                ui.label(
                    RichText::new(tracked(heading))
                        .font(sans(MICRO))
                        .color(MUTED),
                );
                ui.add_space(6.0);
                let height = (ui.available_height() - 32.0).max(0.0);
                ui.allocate_ui(Vec2::new(ui.available_width(), height), |ui| {
                    ui.set_min_height(height);
                    self.results(ui);
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("↑ ↓  Select")
                            .font(sans(CAPTION))
                            .color(MUTED),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new("Enter  Open")
                            .font(sans(CAPTION))
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("File names · All indexed drives")
                                .font(sans(CAPTION))
                                .color(MUTED),
                        );
                    });
                });
            });
        self.resize(ui);
    }

    fn has_details(&self) -> bool {
        !self.state.query.trim().is_empty() || self.state.error.is_some()
    }

    fn resize(&mut self, ui: &Ui) {
        let expanded = self.has_details();
        if self.expanded == Some(expanded) {
            return;
        }
        let mut size = Vec2::new(
            super::WINDOW_SIZE[0],
            if expanded {
                super::WINDOW_SIZE[1]
            } else {
                super::IDLE_HEIGHT
            },
        );
        if let Some(monitor) = ui.input(|input| input.viewport().monitor_size) {
            size = size.min((monitor - Vec2::splat(32.0)).max(Vec2::splat(1.0)));
        }
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        self.expanded = Some(expanded);
    }

    fn search_field(&mut self, ui: &mut Ui) {
        let output = Frame::new()
            .fill(BLACK)
            .stroke(Stroke::new(1.0_f32, LINE_STRONG))
            .corner_radius(6)
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    paint_search_icon(ui, MUTED);
                    let field = ui
                        .allocate_ui(
                            Vec2::new((ui.available_width() - 48.0).max(0.0), 24.0),
                            |ui| {
                                egui::TextEdit::singleline(&mut self.state.query)
                                    .desired_width(ui.available_width())
                                    .font(sans(14.0))
                                    .frame(Frame::NONE)
                                    .hint_text("Search file names…")
                                    .show(ui)
                            },
                        )
                        .inner;
                    if ui
                        .button(RichText::new("Esc").font(mono(CAPTION)))
                        .on_hover_text("Close quick search")
                        .clicked()
                    {
                        self.close_requested = true;
                    }
                    field
                })
                .inner
            });
        let field = output.inner;
        if field.response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            field.response.request_focus();
        }
        if !self.focused {
            field.response.request_focus();
            if let Some(size) = ui.input(|input| input.viewport().monitor_size) {
                let window = Vec2::new(
                    super::WINDOW_SIZE[0].min((size.x - 32.0).max(1.0)),
                    super::IDLE_HEIGHT.min((size.y - 32.0).max(1.0)),
                );
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        (size.x - window.x).max(0.0) / 2.0,
                        (size.y * 0.2).min((size.y - window.y).max(0.0) / 2.0),
                    )));
            }
            self.focused = true;
        }
        // Repeat shortcut refocus (Alt+K while open): focus the query and
        // select its text so typing replaces it. Runs only for that event;
        // initial open leaves the empty query alone, and results stay until
        // an actual edit triggers a new search.
        if self.select_query_on_focus && field.response.has_focus() {
            if !self.state.query.is_empty() {
                let mut edit_state = field.state.clone();
                edit_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::select_all(&field.galley)));
                edit_state.store(ui.ctx(), field.response.id);
            }
            self.select_query_on_focus = false;
        }
        if field.response.has_focus() {
            ui.painter().rect_stroke(
                output.response.rect,
                6,
                Stroke::new(2.0_f32, ACID),
                egui::StrokeKind::Inside,
            );
        }
        if field.response.changed() {
            self.search();
        }
        if ui.input(|input| input.key_pressed(Key::ArrowDown)) && !self.state.hits.is_empty() {
            self.state.selected = (self.state.selected + 1).min(self.state.hits.len() - 1);
        }
        if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
            self.state.selected = self.state.selected.saturating_sub(1);
        }
        if (field.response.has_focus() || field.response.lost_focus())
            && ui.input(|input| input.key_pressed(Key::Enter))
        {
            self.open();
        }
    }

    fn results(&mut self, ui: &mut Ui) {
        if self.state.pending {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Finding matching files…")
                    .font(sans(SMALL))
                    .color(MUTED),
            );
            ui.add_space(8.0);
            loading_bar(ui);
            return;
        }
        if let Some(error) = &self.state.error {
            ui.label(RichText::new(error).font(sans(SMALL)).color(ERROR));
            ui.label(
                RichText::new("Open Neutrasearch to check indexing, then reopen quick search.")
                    .font(sans(SMALL))
                    .color(MUTED),
            );
            return;
        }
        if self.state.hits.is_empty() {
            let text = if self.state.query.trim().is_empty() {
                "No indexed files yet. Open Neutrasearch to check indexing."
            } else {
                "No matching file names. Try a shorter or different name."
            };
            ui.label(RichText::new(text).font(sans(SMALL)).color(MUTED));
            return;
        }
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, hit) in self.state.hits.iter().enumerate() {
                    if super::rows::result_row(ui, hit, index == self.state.selected) {
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked {
            self.state.selected = index;
            self.open();
        }
    }
}

fn loading_bar(ui: &mut Ui) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            "Finding matching files",
        )
    });
    let phase = ui.input(|input| (input.time * 0.8).fract()) as f32;
    let width = rect.width() * 0.25;
    let left = rect.left() - width + (rect.width() + width) * phase;
    ui.painter().rect_filled(rect, 1, BLACK);
    let segment = egui::Rect::from_min_size(
        egui::pos2(left, rect.top()),
        Vec2::new(width, rect.height()),
    );
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .rect_filled(segment, 1, ACID);
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}
