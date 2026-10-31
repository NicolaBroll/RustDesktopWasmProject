//! Git LFS in the view (S63): the "LFS" badge of the stage rows and the diff bar, and the notice
//! when the repo uses LFS but git-lfs is not installed.

use super::*;
use crate::lfs;

/// The badge's color: the same in both themes, with [`theme::ON_BADGE`] text.
const BADGE: Color32 = Color32::from_rgb(0x6f, 0x42, 0xc1);

/// The "LFS" badge as a widget (the diff bar), with its explanation on hover.
pub(crate) fn lfs_badge(ui: &mut Ui) -> egui::Response {
    let font = FontId::monospace(10.0);
    let galley = ui
        .painter()
        .layout_no_wrap(tr!("lfs-badge"), font, theme::ON_BADGE);
    let size = galley.size() + vec2(8.0, 2.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 3.0, BADGE);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme::ON_BADGE);
    response.on_hover_text(tr!("lfs-badge-hint"))
}

/// The "LFS" badge painted at the right end of `right` (a stage row): returns its left edge.
pub(super) fn paint_lfs_badge(painter: &egui::Painter, right: f32, center_y: f32) -> f32 {
    let font = FontId::monospace(10.0);
    let galley = painter.layout_no_wrap(tr!("lfs-badge"), font, theme::ON_BADGE);
    let size = galley.size() + vec2(8.0, 2.0);
    let rect = egui::Rect::from_min_size(egui::pos2(right - size.x, center_y - size.y / 2.0), size);
    painter.rect_filled(rect, 3.0, BADGE);
    painter.galley(rect.center() - galley.size() / 2.0, galley, theme::ON_BADGE);
    rect.left()
}

/// The notice under the bar: the repo uses Git LFS and git-lfs is not installed, so the LFS
/// files stay as pointer text; "Close" hides it (it is not shown again for this repo in the tab).
pub(crate) fn lfs_notice(ui: &mut Ui, actions: &mut Vec<Action>) {
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
                let hint = tr!("lfs-missing-hint");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(tr!("lfs-missing-dismiss"))
                        .on_hover_text(tr!("lfs-missing-dismiss-hint"))
                        .clicked()
                    {
                        actions.push(Action::Lfs(lfs::Action::Dismiss));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(tr!("lfs-missing")).strong()).truncate(),
                        )
                        .on_hover_text(hint);
                    });
                });
            });
        });
}
