//! The commit-graph notice under the bar (S36): why the history is slow, "Optimize" and "Not now".

use crate::Action;
use crate::optimize::OptimizeBox;
use egui::{RichText, Ui};
use project_i18n::tr;

/// The notice for a repo without commit-graph whose first log page was slow. While
/// `repo.optimize` runs the progress and "Stop" are in the bar; here only what is happening.
/// `can_write`: no write or dialog in progress.
pub(crate) fn optimize_notice(
    ui: &mut Ui,
    optimize: &OptimizeBox,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let accent = ui.visuals().hyperlink_color;
    egui::Frame::new()
        .fill(accent.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.5)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let running = optimize.running().is_some();
            ui.horizontal(|ui| {
                let seconds = optimize.first_page().map_or(0.0, |d| d.as_secs_f64());
                let text = if running {
                    tr!("optimize-running")
                } else {
                    tr!("optimize-slow", seconds = format!("{seconds:.1}"))
                };
                ui.add(egui::Label::new(RichText::new(text).strong()).truncate())
                    .on_hover_text(tr!("optimize-explanation"));
                if running {
                    return;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(tr!("optimize-not-now"))
                        .on_hover_text(tr!("optimize-not-now-hint"))
                        .clicked()
                    {
                        actions.push(Action::DismissOptimize);
                    }
                    let button = ui
                        .add_enabled(can_write, egui::Button::new(tr!("optimize-button")))
                        .on_hover_text(tr!("optimize-explanation"))
                        .on_disabled_hover_text(tr!("optimize-after-operation"));
                    if button.clicked() {
                        actions.push(Action::Optimize);
                    }
                });
            });
            if let Some(error) = optimize.error() {
                ui.add(
                    egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                        .wrap(),
                );
            }
        });
}
