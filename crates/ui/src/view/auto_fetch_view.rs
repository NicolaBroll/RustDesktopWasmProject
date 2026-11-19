//! The automatic fetch (S50) in the window: the notice under the bar when an authentication error
//! suspended it, the badge on the "Fetch" button while the remote is unreachable.

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

/// Why the automatic fetch is failing and when it tries again, while the remote cannot be
/// reached: the text the "Fetch" button adds to its tooltip under the [`offline_badge`].
pub(crate) fn offline_hint(auto: &AutoFetch) -> Option<String> {
    let detail = auto.offline()?;
    let minutes = auto.wait().map_or(0, |w| w.as_secs().div_ceil(60));
    Some(format!(
        "{}\n\n{detail}",
        tr!("fetch-auto-offline", minutes = minutes)
    ))
}

/// The orange cloud at the top right corner of the "Fetch" button (`button`: its rectangle)
/// while the remote cannot be reached. Painted over the button, like the counts of Pull and
/// Push: it takes no room, so the toolbar does not move when it appears or goes away.
pub(crate) fn offline_badge(ui: &Ui, button: egui::Rect) {
    let color = ui.visuals().warn_fg_color;
    let galley = ui
        .painter()
        .layout_no_wrap(icons::CLOUD.to_owned(), icons::font(BADGE), color);
    let pos = egui::pos2(button.right() - galley.size().x - 2.0, button.top() + 2.0);
    ui.painter().galley(pos, galley, color);
}

/// Size of the cloud of [`offline_badge`], in points.
const BADGE: f32 = 13.0;
