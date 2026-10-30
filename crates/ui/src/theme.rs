//! The colors of the app that are not egui's: change types, the text on colored badges, the
//! window's close button, the lanes' color used as text. One place, for the dark and the light
//! theme (egui follows the system's, or the one chosen in the File menu).

use egui::{Color32, FontId, TextStyle};
use project_domain::Change;

/// The text sizes of the app, in points: egui's text styles and the views that paint text
/// themselves (commit rows, diff, blame) use these and no others.
pub(crate) mod size {
    /// Secondary text: status bar, sub-lines, hints, counters, ref badges, toolbar labels.
    pub const SMALL: f32 = 11.0;
    /// Body text, buttons, list rows.
    pub const BODY: f32 = 13.0;
    /// Code, hashes, diff lines.
    pub const MONO: f32 = 12.5;
    /// Panel titles.
    pub const HEADING: f32 = 18.0;
}

pub(crate) fn body() -> FontId {
    FontId::proportional(size::BODY)
}

pub(crate) fn small() -> FontId {
    FontId::proportional(size::SMALL)
}

pub(crate) fn mono() -> FontId {
    FontId::monospace(size::MONO)
}

/// egui's text styles on the scale above, in the dark and in the light theme (egui's `Small`
/// is 9 pt: too small next to the body text).
pub(crate) fn install_text_styles(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, small()),
            (TextStyle::Body, body()),
            (TextStyle::Button, body()),
            (TextStyle::Heading, FontId::proportional(size::HEADING)),
            (TextStyle::Monospace, mono()),
        ]
        .into();
    });
}

/// Fill and text of the primary button of a panel (Commit) when it is enabled: the accent of
/// the selected rows, so it stands out from the grey of the other buttons and of itself disabled.
pub(crate) fn primary(visuals: &egui::Visuals) -> (Color32, Color32) {
    (visuals.selection.bg_fill, visuals.selection.stroke.color)
}

/// Text on a colored badge (change letters, branch labels): dark on every badge color.
pub(crate) const ON_BADGE: Color32 = Color32::from_gray(20);

/// Background of "Close" of the window under the mouse (as Windows).
pub(crate) const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// Merge commits in the details.
pub(crate) const MERGE: Color32 = Color32::from_rgb(0x9b, 0x6b, 0xdf);

/// Background of the commit rows that match the search (S48): a warm tint, lighter than the
/// selection.
pub(crate) fn search_match(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgba_unmultiplied(0xf5, 0xc5, 0x18, 48)
    } else {
        Color32::from_rgba_unmultiplied(0xf5, 0xc5, 0x18, 90)
    }
}

/// Badge color of a change type.
pub(crate) fn change(change: Change) -> Color32 {
    match change {
        Change::Added => Color32::from_rgb(0x3f, 0xb9, 0x50),
        Change::Deleted => Color32::from_rgb(0xe5, 0x53, 0x4b),
        Change::Modified => Color32::from_rgb(0xd2, 0x99, 0x22),
        Change::TypeChanged => Color32::from_rgb(0xb0, 0x7c, 0xe8),
        Change::Renamed => Color32::from_rgb(0x4c, 0x9a, 0xf0),
        Change::Copied => Color32::from_rgb(0x2a, 0xb7, 0xa9),
        Change::Unmerged => Color32::from_rgb(0xf5, 0x7c, 0x00),
        _ => Color32::GRAY,
    }
}

/// A lane color used as text or outline on the panel: as is on dark, darker on light, where
/// the yellow and the orange would not read.
pub(crate) fn as_text(color: Color32, dark_mode: bool) -> Color32 {
    if dark_mode {
        color
    } else {
        color.lerp_to_gamma(Color32::BLACK, 0.35)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance (WCAG), 0 black, 1 white.
    fn luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn lane_text_reads_on_the_light_panel() {
        let panel = egui::Visuals::light().panel_fill;
        for lane in 0..8 {
            let color = as_text(crate::graph::lane_color(lane), false);
            assert!(
                contrast(color, panel) >= 3.0,
                "lane {lane}: {:.2}",
                contrast(color, panel)
            );
        }
    }

    #[test]
    fn badge_text_reads_on_every_badge() {
        for c in [
            Change::Added,
            Change::Deleted,
            Change::Modified,
            Change::Renamed,
        ] {
            assert!(contrast(ON_BADGE, change(c)) >= 4.5, "{c:?}");
        }
    }
}
