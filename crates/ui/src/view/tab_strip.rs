//! The strip of the open repos, VS Code style: flat tabs side by side; the active one has the
//! toolbar's background, so it opens onto the repo below, and a colored line on top. The "×"
//! shows only on the active tab and under the mouse.

use super::*;
use crate::tabs;
use egui::{Pos2, Rect, Stroke};

/// Height of the strip.
const HEIGHT: f32 = 32.0;
/// Space before the title and after the "×".
const PADDING: f32 = 12.0;
/// Side of the "×" (and of the spinner in its place).
const CLOSE_SIZE: f32 = 18.0;
/// The colored line on top of the active tab.
const ACCENT: f32 = 2.0;

/// A tab of the strip, as [`tab_strip_panel`] shows it.
pub(crate) struct TabLabel {
    pub(crate) title: String,
    /// The repo open or opening, on hover.
    pub(crate) path: Option<String>,
    /// `repo.open` in progress.
    pub(crate) opening: bool,
    /// A write in progress: the tab cannot be closed.
    pub(crate) writing: bool,
}

/// The open repos, one tab each, with "×" (or the middle button) to close it and "+" to open
/// another.
pub(crate) fn tab_strip_panel(
    ui: &mut Ui,
    tabs: &[TabLabel],
    active: usize,
    can_pick: bool,
    actions: &mut Vec<Action>,
) {
    let fill = ui.visuals().extreme_bg_color;
    egui::Panel::top("tabs")
        .exact_size(HEIGHT)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(fill))
        .show(ui, |ui| tab_strip(ui, tabs, active, can_pick, actions));
}

fn tab_strip(
    ui: &mut Ui,
    tabs: &[TabLabel],
    active: usize,
    can_pick: bool,
    actions: &mut Vec<Action>,
) {
    let strip = ui.max_rect();
    let mut active_rect = None;
    egui::ScrollArea::horizontal()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (index, label) in tabs.iter().enumerate() {
                    let rect = tab(ui, label, index, index == active, actions);
                    if index == active {
                        active_rect = Some(rect);
                    }
                }
                if can_pick && new_tab_button(ui).clicked() {
                    actions.push(Action::Tabs(tabs::Action::PickFolder));
                }
            });
        });
    // The line under the strip, open under the active tab.
    let painter = ui.painter_at(strip);
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let y = painter.round_to_pixel_center(strip.bottom() - 0.5);
    match active_rect {
        Some(active) => {
            painter.hline(strip.left()..=active.left(), y, stroke);
            painter.hline(active.right()..=strip.right(), y, stroke);
        }
        None => {
            painter.hline(strip.x_range(), y, stroke);
        }
    }
}

/// A tab; returns where it was drawn.
fn tab(
    ui: &mut Ui,
    label: &TabLabel,
    index: usize,
    active: bool,
    actions: &mut Vec<Action>,
) -> Rect {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let title = ui
        .painter()
        .layout_no_wrap(label.title.clone(), font, Color32::PLACEHOLDER);
    let width = PADDING + title.size().x + 8.0 + CLOSE_SIZE + PADDING / 2.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
    let close_rect = Rect::from_center_size(
        Pos2::new(
            rect.right() - PADDING / 2.0 - CLOSE_SIZE / 2.0,
            rect.center().y,
        ),
        vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let close = ui.interact(close_rect, response.id.with("close"), Sense::click());
    let hovered = response.hovered() || close.hovered();

    let visuals = ui.visuals();
    let painter = ui.painter();
    let fill = if active {
        visuals.panel_fill
    } else if hovered {
        visuals
            .extreme_bg_color
            .lerp_to_gamma(visuals.panel_fill, 0.5)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 0.0, fill);
    if active {
        let accent = Rect::from_min_size(rect.min, vec2(rect.width(), ACCENT));
        // The bright blue of the links: the selection's is too dark for a 2 px line.
        painter.rect_filled(accent, 0.0, visuals.hyperlink_color);
    }
    let border = visuals.widgets.noninteractive.bg_stroke;
    let x = painter.round_to_pixel_center(rect.right() - 0.5);
    painter.vline(x, rect.y_range(), border);
    let color = if active || hovered {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let title_pos = Pos2::new(
        rect.left() + PADDING,
        rect.center().y - title.size().y / 2.0,
    );
    painter.galley(title_pos, title, color);

    if label.opening && !hovered {
        egui::Spinner::new().size(12.0).paint_at(
            ui,
            Rect::from_center_size(close_rect.center(), vec2(12.0, 12.0)),
        );
    } else if active || hovered {
        close_icon(ui, close_rect, &close, label.writing);
    }

    let closes = close.clicked() || response.middle_clicked();
    if closes && !label.writing {
        actions.push(Action::Tabs(tabs::Action::CloseTab(index)));
    } else if response.clicked() {
        actions.push(Action::Tabs(tabs::Action::SwitchTab(index)));
    }
    close.on_hover_text(if label.writing {
        tr!("tabs-close-writing")
    } else {
        tr!("tabs-close-hint")
    });
    if let Some(path) = &label.path {
        response.on_hover_text(path);
    }
    rect
}

/// The "×" of a tab: a square background under the mouse, faded while it cannot close.
fn close_icon(ui: &Ui, rect: Rect, close: &egui::Response, writing: bool) {
    let visuals = ui.visuals();
    let painter = ui.painter();
    if close.hovered() && !writing {
        painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let color = if writing {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    let stroke = Stroke::new(1.0, color);
    let center = rect.center();
    let d = 3.5;
    painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], stroke);
    painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
}

/// "+": opens a repository in a new tab.
fn new_tab_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(HEIGHT, HEIGHT), Sense::click());
    let visuals = ui.visuals();
    let painter = ui.painter();
    let square = Rect::from_center_size(rect.center(), vec2(24.0, 24.0));
    if response.hovered() {
        painter.rect_filled(square, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let stroke = Stroke::new(1.0, visuals.text_color());
    let center = rect.center();
    let d = 5.0;
    painter.line_segment([center - vec2(d, 0.0), center + vec2(d, 0.0)], stroke);
    painter.line_segment([center - vec2(0.0, d), center + vec2(0.0, d)], stroke);
    response.on_hover_text(tr!("tabs-new-hint"))
}
