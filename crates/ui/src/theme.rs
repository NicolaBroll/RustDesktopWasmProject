//! The colors of the app that are not egui's: change types, the text on colored badges, the
//! window's close button, the lanes' color used as text. One place, for the dark and the light
//! theme (egui follows the system's, or the one chosen in Settings).

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
/// is 9 pt: too small next to the body text), and the contrast of the two themes ([`dark`],
/// [`light`]).
pub(crate) fn install_text_styles(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |style| dark(&mut style.visuals));
    ctx.style_mut_of(egui::Theme::Light, |style| light(&mut style.visuals));
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

/// egui's dark theme with more contrast: its text is 5:1 against the panel, weak text 2.7:1
/// and disabled text 2.3:1. Text gets at least 7:1, weak text 4.5:1 and disabled text 3:1, the
/// three still apart; red reads on the panel, on a button and on a menu entry under the mouse,
/// and the text of a selected row is white, so its weak half reads on the blue too.
fn dark(visuals: &mut egui::Visuals) {
    visuals.widgets.noninteractive.fg_stroke.color = Color32::from_gray(190);
    visuals.weak_text_alpha = 0.72;
    visuals.disabled_alpha = 0.55;
    visuals.error_fg_color = Color32::from_rgb(0xff, 0x8a, 0x80);
    visuals.selection.stroke.color = Color32::WHITE;
}

/// egui's light theme with more contrast: text fields are white on a white-grey panel without a
/// border, weak or disabled text fades below 3:1 and links, red and orange are below 4.5:1. The
/// fields (and buttons) get a grey border, labels the color of the buttons' text, weak and
/// disabled text a stronger alpha, and the colors are darkened until they read on the panel,
/// on a button and on a row under the mouse.
fn light(visuals: &mut egui::Visuals) {
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LIGHT_BORDER);
    visuals.text_edit_bg_color = Some(Color32::WHITE);
    visuals.widgets.noninteractive.fg_stroke.color = visuals.widgets.inactive.text_color();
    visuals.weak_text_alpha = 0.76;
    visuals.disabled_alpha = 0.62;
    visuals.hyperlink_color = Color32::from_rgb(0x00, 0x5f, 0xcc);
    visuals.warn_fg_color = Color32::from_rgb(0x9a, 0x44, 0x00);
    visuals.error_fg_color = Color32::from_rgb(0xb4, 0x14, 0x1e);
    visuals.selection.stroke.color = Color32::from_rgb(0, 40, 70);
}

/// The border of fields and buttons in the light theme: at least 3:1 against the panel, as
/// a field's border should be.
const LIGHT_BORDER: Color32 = Color32::from_gray(140);

/// Fill and text of the primary button of a panel (Commit) when it is enabled: the accent of
/// the selected rows, so it stands out from the grey of the other buttons and of itself disabled.
pub(crate) fn primary(visuals: &egui::Visuals) -> (Color32, Color32) {
    (visuals.selection.bg_fill, visuals.selection.stroke.color)
}

/// The color of the focus ring: the text color of the selection, light on the dark theme and
/// dark on the light one, so it reads on the panel, on a row under the mouse and on a selected
/// row.
pub(crate) fn focus_color(visuals: &egui::Visuals) -> Color32 {
    visuals.selection.stroke.color
}

/// Width of the focus ring, in points.
pub(crate) const FOCUS_WIDTH: f32 = 1.5;

/// The focus ring of a widget drawn by hand: a line inside `rect` while `response` has the
/// keyboard (Tab). egui's own widgets show the focus themselves; every widget made with
/// `allocate_exact_size` or `Ui::interact` calls this, or Tab stops on it without a sign.
pub(crate) fn focus_ring(ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
    if response.has_focus() {
        paint_focus_ring(ui, rect);
    }
}

/// The ring of [`focus_ring`], for who knows by itself that the widget has the keyboard.
pub(crate) fn paint_focus_ring(ui: &egui::Ui, rect: egui::Rect) {
    let stroke = egui::Stroke::new(FOCUS_WIDTH, focus_color(ui.visuals()));
    ui.painter()
        .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
}

/// Text on a colored badge (change letters, branch labels): dark on every badge color.
pub(crate) const ON_BADGE: Color32 = Color32::from_gray(20);

/// Background of "Close" of the window under the mouse (as Windows).
pub(crate) const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// Merge commits in the details: a purple that reads on the panel of each theme.
pub(crate) fn merge(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(0xb3, 0x8c, 0xf0)
    } else {
        Color32::from_rgb(0x6f, 0x42, 0xc1)
    }
}

/// Text and weak text on a selected row: the selection's text color, gray on blue is hard to
/// read. The weak one is blended, as egui's weak text is.
pub(crate) fn on_selection(visuals: &egui::Visuals) -> (Color32, Color32) {
    let text = visuals.selection.stroke.color;
    (text, text.gamma_multiply(ON_SELECTION_WEAK))
}

/// Alpha of the weak text of a selected row.
const ON_SELECTION_WEAK: f32 = 0.75;

/// The small numbers beside a row (refs in a folder, commits to pull): weak on the dark theme;
/// on the light one, where a thin "1" at this size reaches only four fifths of its color, the
/// text color, and the size alone sets them apart.
pub(crate) fn counter(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    }
}

/// The text of a button that destroys something (discard, delete, drop, abort): red while it
/// can be pressed, a button's own text otherwise: egui fades a disabled widget by itself, and
/// a color already weak would fade twice.
pub(crate) fn destructive(visuals: &egui::Visuals, enabled: bool) -> Color32 {
    if enabled {
        visuals.error_fg_color
    } else {
        visuals.widgets.inactive.text_color()
    }
}

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
/// the yellow and the orange would not read (4.5:1 for every lane).
pub(crate) fn as_text(color: Color32, dark_mode: bool) -> Color32 {
    if dark_mode {
        color
    } else {
        color.lerp_to_gamma(Color32::BLACK, 0.4)
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

    /// `color` with `alpha` over `background`, blended as egui does (in gamma space).
    fn over(color: Color32, alpha: f32, background: Color32) -> Color32 {
        let mix =
            |c: u8, b: u8| (f32::from(c) * alpha + f32::from(b) * (1.0 - alpha)).round() as u8;
        Color32::from_rgb(
            mix(color.r(), background.r()),
            mix(color.g(), background.g()),
            mix(color.b(), background.b()),
        )
    }

    fn dark_theme() -> egui::Visuals {
        let mut visuals = egui::Visuals::dark();
        dark(&mut visuals);
        visuals
    }

    fn light_theme() -> egui::Visuals {
        let mut visuals = egui::Visuals::light();
        light(&mut visuals);
        visuals
    }

    #[track_caller]
    fn reads(what: &str, text: Color32, background: Color32, least: f32) -> f32 {
        let ratio = contrast(text, background);
        assert!(ratio >= least, "{what}: {ratio:.2} < {least}");
        ratio
    }

    /// The thresholds of both themes: text 7:1 on the panel, weak text and the placeholder of
    /// a field 4.5:1, disabled text 3:1, the three apart; links and colored text 4.5:1 on
    /// the surfaces they are drawn on; text and weak text of a selected row 4.5:1.
    #[track_caller]
    fn text_levels_read(visuals: &egui::Visuals) {
        let panel = visuals.panel_fill;
        let field = visuals.text_edit_bg_color();
        let button = visuals.widgets.inactive.weak_bg_fill;
        let hovered = visuals.widgets.hovered.weak_bg_fill;
        let text = visuals.text_color();
        let weak = |background| over(text, visuals.weak_text_alpha, background);
        let disabled = |color, background| over(color, visuals.disabled_alpha, background);

        let normal = reads("text", text, panel, 7.0);
        let faded = reads("weak", weak(panel), panel, 4.5);
        reads("text in a field", text, field, 7.0);
        reads("placeholder", weak(field), field, 4.5);
        let off = reads("disabled", disabled(text, panel), panel, 3.0);
        // A disabled button: its fill fades into the panel, its text into the fill.
        let button_off = disabled(button, panel);
        let button_text = visuals.widgets.inactive.text_color();
        reads("button", button_text, button, 7.0);
        reads(
            "disabled button",
            disabled(button_text, button_off),
            button_off,
            3.0,
        );
        reads(
            "disabled destructive button",
            disabled(destructive(visuals, false), button_off),
            button_off,
            3.0,
        );
        assert!(
            normal >= faded * 1.5 && faded >= off * 1.3,
            "levels: {normal:.2} {faded:.2} {off:.2}"
        );

        reads("link", visuals.hyperlink_color, panel, 4.5);
        reads("merge", merge(visuals.dark_mode), panel, 4.5);
        reads("counter", counter(visuals), panel, 4.5);
        // Red and orange are the text of buttons and menu entries too.
        for (name, background) in [("panel", panel), ("button", button), ("hover", hovered)] {
            reads(
                &format!("error on {name}"),
                visuals.error_fg_color,
                background,
                4.5,
            );
            reads(
                &format!("warn on {name}"),
                visuals.warn_fg_color,
                background,
                4.5,
            );
        }
        // A notice: orange text on the panel tinted with the same orange.
        let notice = over(visuals.warn_fg_color, 0.12, panel);
        reads("warn on a notice", visuals.warn_fg_color, notice, 4.5);
        reads("text on a notice", text, notice, 7.0);

        let selection = visuals.selection.bg_fill;
        let (on_selection, weak_on_selection) = on_selection(visuals);
        assert_eq!(on_selection.a(), 255);
        let selected = reads("selected row", on_selection, selection, 7.0);
        let selected_weak = reads(
            "selected row, weak",
            over(on_selection, ON_SELECTION_WEAK, selection),
            selection,
            4.5,
        );
        assert!(selected >= selected_weak * 1.3);
        assert_eq!(
            weak_on_selection,
            on_selection.gamma_multiply(ON_SELECTION_WEAK)
        );
        // The primary button is drawn with the selection's colors.
        let (fill, primary_text) = primary(visuals);
        reads("primary button", primary_text, fill, 7.0);
        for lane in 0..8 {
            let color = as_text(crate::graph::lane_color(lane), visuals.dark_mode);
            reads(&format!("lane {lane}"), color, panel, 4.5);
        }
    }

    #[test]
    fn dark_text_weak_disabled_and_colored_text_read() {
        let visuals = dark_theme();
        text_levels_read(&visuals);
        // The strong text (headings, the current branch) still stands out from the text.
        assert!(contrast(visuals.strong_text_color(), visuals.text_color()) >= 1.5);
    }

    #[test]
    fn light_fields_and_faded_text_read_on_the_panel() {
        let visuals = light_theme();
        let panel = visuals.panel_fill;
        let border = visuals.widgets.inactive.bg_stroke;
        assert!(border.width >= 1.0);
        reads("border", border.color, panel, 3.0);
        text_levels_read(&visuals);
        assert!(contrast(visuals.strong_text_color(), visuals.text_color()) >= 1.5);
        // Glyphs are not emboldened on the light theme: the stem of a small "1" gets four
        // fifths of its color, and the counters and the links must read all the same.
        let thin = |color| over(color, 0.8, panel);
        reads("thin counter", thin(counter(&visuals)), panel, 4.5);
        reads("thin link", thin(visuals.hyperlink_color), panel, 3.0);
    }

    #[test]
    fn a_disabled_destructive_button_fades_once() {
        for visuals in [dark_theme(), light_theme()] {
            assert_eq!(destructive(&visuals, true), visuals.error_fg_color);
            assert_eq!(
                destructive(&visuals, false),
                visuals.widgets.inactive.text_color()
            );
        }
    }

    #[test]
    fn the_focus_ring_reads_on_the_panel_and_on_a_selected_row() {
        for visuals in [dark_theme(), light_theme()] {
            let ring = focus_color(&visuals);
            let hovered = visuals.widgets.hovered.weak_bg_fill;
            for (name, background) in [
                ("panel", visuals.panel_fill),
                ("selection", visuals.selection.bg_fill),
                ("hover", hovered),
            ] {
                assert!(
                    contrast(ring, background) >= 3.0,
                    "{name}, dark {}: {:.2}",
                    visuals.dark_mode,
                    contrast(ring, background)
                );
            }
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
