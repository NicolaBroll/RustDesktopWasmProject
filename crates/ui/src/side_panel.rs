//! Width of the side panels. egui stores the width a panel was drawn at, already clamped to
//! its range: after the window shrank and grew again the panel would stay narrow. Here the
//! width chosen by the user is kept apart and clamped only while drawing.

use egui::{Id, PanelState, Rangef};

/// The share of the window the center keeps at least: each side panel gets at most half of the
/// rest, never less than its minimum.
const CENTER_SHARE: f32 = 0.4;

/// What is remembered of a side panel between frames.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Width {
    /// The width chosen by the user (or the default).
    preferred: f32,
    /// The width asked to egui last frame: if egui drew another one, the user dragged it.
    asked: f32,
}

/// The largest width of a side panel in a window `available` wide.
pub(crate) fn max_width(available: f32, min: f32) -> f32 {
    (available * (1.0 - CENTER_SHARE) / 2.0).max(min)
}

/// The width to draw at, given what was remembered and what egui drew last frame.
fn next(remembered: Option<Width>, drawn: Option<f32>, default: f32, range: Rangef) -> Width {
    let mut preferred = remembered.map_or(default, |w| w.preferred);
    if let (Some(w), Some(drawn)) = (remembered, drawn)
        && (drawn - w.asked).abs() > 0.5
    {
        preferred = drawn;
    }
    Width {
        preferred,
        asked: preferred.clamp(range.min, range.max),
    }
}

/// Before showing the panel `salt` of `ui` (left or right): sets the width egui will draw it at
/// to the user's one, clamped to `range` (the range the panel is then shown with).
pub(crate) fn prepare(ui: &egui::Ui, salt: &str, default: f32, range: Rangef) {
    let panel = panel_id(ui, salt);
    let memory = panel.with("preferred-width");
    let ctx = ui.ctx();
    let state = PanelState::load(ctx, panel);
    let remembered = ctx.data_mut(|d| d.get_persisted::<Width>(memory));
    let width = next(remembered, state.map(|s| s.size().x), default, range);
    ctx.data_mut(|d| d.insert_persisted(memory, width));
    if let Some(mut state) = state {
        // Only the size counts: the panel places itself on its side.
        state.outer_rect.set_width(width.asked);
        ctx.data_mut(|d| d.insert_persisted::<PanelState>(panel, state));
    }
}

/// The id egui gives the panel `salt` of `ui` (`Panel::id`).
fn panel_id(ui: &egui::Ui, salt: &str) -> Id {
    ui.scope_id().with(salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: Rangef = Rangef {
        min: 200.0,
        max: 360.0,
    };

    #[test]
    fn the_width_comes_back_when_the_window_grows() {
        let first = next(None, None, 320.0, RANGE);
        assert_eq!(first.asked, 320.0);
        // The window shrinks: drawn narrower because asked narrower, not dragged.
        let narrow = Rangef::new(200.0, 228.0);
        let small = next(Some(first), Some(320.0), 320.0, narrow);
        assert_eq!((small.preferred, small.asked), (320.0, 228.0));
        let back = next(Some(small), Some(228.0), 320.0, RANGE);
        assert_eq!(back.asked, 320.0, "the user's width again");
    }

    #[test]
    fn a_drag_becomes_the_preferred_width() {
        let first = next(None, None, 320.0, RANGE);
        let dragged = next(Some(first), Some(250.0), 320.0, RANGE);
        assert_eq!((dragged.preferred, dragged.asked), (250.0, 250.0));
    }

    #[test]
    fn the_center_keeps_its_share() {
        assert!((max_width(1200.0, 200.0) - 360.0).abs() < 0.01);
        assert!((max_width(760.0, 200.0) - 228.0).abs() < 0.01);
        assert_eq!(max_width(500.0, 200.0), 200.0, "never under the minimum");
    }

    #[test]
    fn the_panel_id_is_the_one_egui_uses() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ours = panel_id(ui, "details");
            egui::Panel::right("details").show(ui, |_| {});
            assert!(
                PanelState::load(ui.ctx(), ours).is_some(),
                "egui stored its state under our id"
            );
        });
        output.textures_delta.clear();
    }
}
