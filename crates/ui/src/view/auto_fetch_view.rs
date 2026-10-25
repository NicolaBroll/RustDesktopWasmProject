//! The automatic fetch (S50) in the window: the notice under the bar when an authentication error
//! suspended it, the icon in the bar while the remote is unreachable.

use crate::Action;
use crate::auto_fetch::{AutoFetch, Suspended};
use crate::icons;
use egui::{RichText, Ui};
use project_i18n::tr;

/// The notice with "Retry"; git's text on hover.
pub(crate) fn auto_fetch_notice(ui: &mut Ui, suspended: &Suspended, actions: &mut Vec<Action>) {
    let warn = ui.visuals().warn_fg_color;
    egui::Frame::new()
        .fill(warn.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, warn.gamma_multiply(0.5)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut hint = tr!("fetch-auto-suspended-hint");
                if let Some(detail) = &suspended.detail {
                    hint.push_str("\n\n");
                    hint.push_str(detail);
                }
                ui.add(
                    egui::Label::new(RichText::new(tr!("fetch-auto-suspended")).strong())
                        .truncate(),
                )
                .on_hover_text(hint);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(tr!("fetch-auto-retry"))
                        .on_hover_text(tr!("fetch-auto-retry-hint"))
                        .clicked()
                    {
                        actions.push(Action::RetryAutoFetch);
                    }
                });
            });
        });
}

/// The icon next to "Fetch" while the remote cannot be reached; the explanation on hover.
pub(crate) fn offline_icon(ui: &mut Ui, auto: &AutoFetch) {
    let Some(detail) = auto.offline() else {
        return;
    };
    let minutes = auto.wait().map_or(0, |w| w.as_secs().div_ceil(60));
    let icon = RichText::new(icons::CLOUD)
        .font(icons::font(16.0))
        .color(ui.visuals().warn_fg_color);
    ui.label(icon).on_hover_text(format!(
        "{}\n\n{detail}",
        tr!("fetch-auto-offline", minutes = minutes)
    ));
}
