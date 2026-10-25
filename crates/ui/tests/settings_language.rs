//! The language chosen in the "Settings…" window applies without restarting: the texts change on
//! the next frame. In its own test binary: the language is global to the process, the unit tests
//! of the crate run in English.

use eframe::App as _;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Shape, vec2};
use project_engine::{Capabilities, MemoryOpener, Platform};
use project_i18n::Lang;
use std::sync::Arc;

/// No folder dialog, jobs run at once, no system language (English).
struct Inline;

impl Platform for Inline {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

struct Harness {
    ctx: egui::Context,
    app: project_ui::App,
    frame: eframe::Frame,
    /// The texts drawn in the last frame, with their rectangle.
    texts: Vec<(String, Rect)>,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = project_ui::App::new(&cc, Arc::new(Inline), Arc::new(MemoryOpener::new()), None);
        Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            texts: Vec::new(),
        }
    }

    fn step(&mut self, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            events,
            ..Default::default()
        };
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut output = self.ctx.run_ui(input, |ui| app.ui(ui, frame));
        output.textures_delta.clear();
        self.texts.clear();
        for clipped in output.shapes {
            collect(&clipped.shape, &mut self.texts);
        }
    }

    fn shows(&self, text: &str) -> bool {
        self.texts.iter().any(|(t, _)| t == text)
    }

    fn click(&mut self, text: &str) {
        let (_, rect) = self
            .texts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("{text:?} not shown: {:?}", self.texts));
        let pos = rect.center();
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.step(vec![Event::PointerMoved(pos), button(true)]);
        self.step(vec![button(false)]);
    }
}

fn collect(shape: &Shape, out: &mut Vec<(String, Rect)>) {
    match shape {
        Shape::Text(text) => out.push((
            text.galley.text().to_owned(),
            text.galley.rect.translate(text.pos.to_vec2()),
        )),
        Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

#[test]
fn the_language_changes_on_the_next_frame() {
    let mut h = Harness::new();
    h.step(Vec::new());
    assert_eq!(project_i18n::lang(), Lang::En);
    // Ctrl+, opens the window.
    h.step(vec![Event::Key {
        key: Key::Comma,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL | Modifiers::COMMAND,
    }]);
    // A new window is measured on its first frame, drawn from the second.
    h.step(Vec::new());
    h.step(Vec::new());
    assert!(h.shows("Settings") && h.shows("General"), "{:?}", h.texts);
    h.click("Italiano");
    h.step(Vec::new());
    assert_eq!(project_i18n::lang(), Lang::It);
    assert!(
        h.shows("Impostazioni") && h.shows("Generale"),
        "{:?}",
        h.texts
    );
    assert!(!h.shows("Settings"));
    // And back, still without restarting.
    h.click("English");
    h.step(Vec::new());
    assert!(h.shows("Settings") && !h.shows("Impostazioni"));
}
