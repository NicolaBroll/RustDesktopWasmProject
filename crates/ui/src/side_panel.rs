//! Width of the side panels. egui stores the width a panel was drawn at, already clamped to
//! its range: after the window shrank and grew again the panel would stay narrow, and a frame
//! whose content is wider than the panel would leave it wide. Here the width chosen by the user
//! is kept apart, changed only by a drag of the panel's edge and clamped only while drawing.

use egui::{Id, PanelState, Rangef};

/// The share of the window the center keeps at least: each side panel gets at most half of the
/// rest, never less than its minimum.
const CENTER_SHARE: f32 = 0.4;

/// What is remembered of a side panel between frames.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Width {
    /// The width chosen by the user (or the default).
    preferred: f32,
    /// The width asked to egui last frame.
    asked: f32,
    /// The panel's edge was dragged or released last frame: only then a drawn width other than
    /// the asked one is the user's (egui also draws a panel wider when its content overflows).
    #[serde(default)]
    dragged: bool,
}

/// The largest width of a side panel in a window `available` wide.
pub(crate) fn max_width(available: f32, min: f32) -> f32 {
    (available * (1.0 - CENTER_SHARE) / 2.0).max(min)
}

/// The width to draw at, given what was remembered and what egui drew last frame. `dragged`:
/// the panel's edge is being dragged or was released in this frame.
fn next(
    remembered: Option<Width>,
    drawn: Option<f32>,
    dragged: bool,
    default: f32,
    range: Rangef,
) -> Width {
    let mut preferred = remembered.map_or(default, |w| w.preferred);
    if let (Some(w), Some(drawn)) = (remembered, drawn)
        && w.dragged
        && (drawn - w.asked).abs() > 0.5
    {
        preferred = drawn;
    }
    Width {
        preferred,
        asked: preferred.clamp(range.min, range.max),
        dragged,
    }
}

/// Before showing the panel `salt` of `ui` (left or right): sets the width egui will draw it at
/// to the user's one, clamped to `range` (the range the panel is then shown with).
pub(crate) fn prepare(ui: &egui::Ui, salt: &str, default: f32, range: Rangef) {
    pass_focus(ui, salt);
    let panel = panel_id(ui, salt);
    let memory = panel.with("preferred-width");
    let ctx = ui.ctx();
    let state = PanelState::load(ctx, panel);
    let remembered = ctx.data_mut(|d| d.get_persisted::<Width>(memory));
    // egui keeps the stored width as it was during a drag and stores the new one in the frame
    // of the release: it is read here in the next frame, hence the flag kept in `Width`.
    let dragged = ctx
        .read_response(resize_id(panel))
        .is_some_and(|r| r.dragged() || r.drag_stopped());
    let width = next(
        remembered,
        state.map(|s| s.size().x),
        dragged,
        default,
        range,
    );
    ctx.data_mut(|d| d.insert_persisted(memory, width));
    if let Some(mut state) = state {
        // Only the size counts: the panel places itself on its side.
        state.outer_rect.set_width(width.asked);
        ctx.data_mut(|d| d.insert_persisted::<PanelState>(panel, state));
    }
}

/// Before showing the resizable panel `salt` of `ui`: egui makes a Tab stop of the panel's edge,
/// which the keyboard cannot use and which shows no focus. When the focus lands there it goes
/// on to the next widget, or back to the one before with Shift+Tab.
pub(crate) fn pass_focus(ui: &egui::Ui, salt: &str) {
    let ctx = ui.ctx();
    let direction = Id::unique("side-panel-focus-back");
    let tab = ctx.input(|i| {
        i.events.iter().rev().find_map(|event| match event {
            egui::Event::Key {
                key: egui::Key::Tab,
                pressed: true,
                modifiers,
                ..
            } => Some(modifiers.shift),
            _ => None,
        })
    });
    if let Some(back) = tab {
        // The edge takes the focus during this frame or at the start of the next one.
        ctx.data_mut(|d| d.insert_temp(direction, back));
        ctx.request_repaint();
    }
    if ctx.memory(|m| m.has_focus(resize_id(panel_id(ui, salt)))) {
        let back = ctx.data(|d| d.get_temp::<bool>(direction)).unwrap_or(false);
        ctx.memory_mut(|m| {
            m.move_focus(if back {
                egui::FocusDirection::Previous
            } else {
                egui::FocusDirection::Next
            });
        });
        ctx.request_repaint();
    }
}

/// The id egui gives the panel `salt` of `ui` (`Panel::id`).
fn panel_id(ui: &egui::Ui, salt: &str) -> Id {
    ui.scope_id().with(salt)
}

/// The id egui gives the resize edge of the panel `panel` (`resize_widget_id` in egui's
/// `panel.rs`).
fn resize_id(panel: Id) -> Id {
    panel.with("__resize")
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
        let first = next(None, None, false, 320.0, RANGE);
        assert_eq!(first.asked, 320.0);
        // The window shrinks: drawn narrower because asked narrower, not dragged.
        let narrow = Rangef::new(200.0, 228.0);
        let small = next(Some(first), Some(320.0), false, 320.0, narrow);
        assert_eq!((small.preferred, small.asked), (320.0, 228.0));
        let back = next(Some(small), Some(228.0), false, 320.0, RANGE);
        assert_eq!(back.asked, 320.0, "the user's width again");
    }

    #[test]
    fn a_drag_becomes_the_preferred_width() {
        let first = next(None, None, false, 320.0, RANGE);
        // While the edge is dragged egui keeps the stored width; it stores the new one in the
        // frame of the release, read in the frame after.
        let dragging = next(Some(first), Some(320.0), true, 320.0, RANGE);
        assert_eq!((dragging.preferred, dragging.asked), (320.0, 320.0));
        let released = next(Some(dragging), Some(320.0), true, 320.0, RANGE);
        let after = next(Some(released), Some(250.0), false, 320.0, RANGE);
        assert_eq!((after.preferred, after.asked), (250.0, 250.0));
        let still = next(Some(after), Some(250.0), false, 320.0, RANGE);
        assert_eq!((still.preferred, still.asked), (250.0, 250.0));
    }

    #[test]
    fn a_wider_frame_without_a_drag_keeps_the_preferred_width() {
        let first = next(None, None, false, 320.0, RANGE);
        let dragging = next(Some(first), Some(320.0), true, 320.0, RANGE);
        let chosen = next(Some(dragging), Some(260.0), false, 320.0, RANGE);
        assert_eq!((chosen.preferred, chosen.asked), (260.0, 260.0));
        // A frame whose content overflows: egui draws the panel at its maximum.
        let wide = next(Some(chosen), Some(360.0), false, 320.0, RANGE);
        assert_eq!((wide.preferred, wide.asked), (260.0, 260.0));
        // It lasts some frames, then the content fits again.
        let again = next(Some(wide), Some(360.0), false, 320.0, RANGE);
        let fits = next(Some(again), Some(260.0), false, 320.0, RANGE);
        assert_eq!((fits.preferred, fits.asked), (260.0, 260.0));
    }

    #[test]
    fn a_width_saved_before_the_drag_flag_is_read() {
        let old: Width = serde_json::from_str(r#"{"preferred":250.0,"asked":250.0}"#).unwrap();
        assert_eq!((old.preferred, old.dragged), (250.0, false));
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

    /// The whole path through egui: `content` wide content in a panel asked 260 wide, a frame
    /// at a time; `pointer` events drag the edge.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, content: f32) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 760.0),
            )),
            events,
            ..Default::default()
        };
        let mut width = 0.0;
        let mut output = ctx.run_ui(input, |ui| {
            prepare(ui, "details", 260.0, RANGE);
            width = egui::Panel::right("details")
                .resizable(true)
                .default_size(260.0)
                .size_range(RANGE)
                .show(ui, |ui| {
                    // Like the real panels the content fills the width, or is wider.
                    let width = ui.available_width().max(content);
                    ui.allocate_space(egui::vec2(width, 20.0));
                })
                .response
                .rect
                .width();
            egui::CentralPanel::default().show(ui, |_| {});
        });
        output.textures_delta.clear();
        width
    }

    fn pointer(x: f32, pressed: Option<bool>) -> Vec<egui::Event> {
        let pos = egui::pos2(x, 400.0);
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        events
    }

    #[test]
    fn content_wider_than_the_panel_does_not_widen_it_for_good() {
        let ctx = egui::Context::default();
        assert_eq!(frame(&ctx, vec![], 100.0), 260.0);
        assert_eq!(frame(&ctx, vec![], 100.0), 260.0);
        // One frame of content that does not fit: egui grows the panel to its maximum.
        assert_eq!(frame(&ctx, vec![], 600.0), 360.0);
        assert_eq!(
            frame(&ctx, vec![], 100.0),
            260.0,
            "back to the user's width"
        );
        assert_eq!(frame(&ctx, vec![], 100.0), 260.0);
    }

    #[test]
    fn dragging_the_edge_through_egui_changes_the_width() {
        let ctx = egui::Context::default();
        frame(&ctx, vec![], 100.0);
        frame(&ctx, vec![], 100.0);
        // The edge is at 1200 - 260 = 940: dragged 40 points to the left.
        frame(&ctx, pointer(940.0, None), 100.0);
        frame(&ctx, pointer(940.0, Some(true)), 100.0);
        frame(&ctx, pointer(920.0, None), 100.0);
        frame(&ctx, pointer(900.0, None), 100.0);
        frame(&ctx, pointer(900.0, Some(false)), 100.0);
        for _ in 0..3 {
            assert_eq!(frame(&ctx, vec![], 100.0), 300.0, "the dragged width stays");
        }
    }
}
