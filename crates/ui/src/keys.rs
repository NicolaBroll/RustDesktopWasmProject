//! The keyboard shortcuts (U40): one table, read by the frame (which key does what), by the
//! toolbar tooltips and by Help → "Keyboard shortcuts". A shortcut only becomes the
//! [`Action`](crate::Action) of the button or menu entry it stands for: no logic of its own.
//!
//! - **Who**: [`App`](crate::App) applies the app's commands (tabs, settings, scale); the tab
//!   the others, with the same gates as its buttons.
//! - **When**: never with a popup or a menu open (the key belongs to it), nor with a modal
//!   window open; only the [`Command::global`] ones while a text field has the keyboard.
//! - The list selection keys (arrows, Page up/down, Home/End, Esc) stay in the tab's keyboard
//!   handling: they depend on what the center shows.

use egui::{Key, KeyboardShortcut, Modifiers};
use project_i18n::tr;

/// What a shortcut asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Undo,
    Redo,
    Refresh,
    Fetch,
    Pull,
    Push,
    /// Stages or unstages the file of the stage panel whose diff is open.
    ToggleStage,
    Find,
    Settings,
    NextTab,
    PreviousTab,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

/// The keys of a frame, for the tab shown.
#[derive(Debug, Default)]
pub(crate) struct Pressed {
    /// The commands pressed: the tab applies its own, [`App`](crate::App) the others.
    pub commands: Vec<Command>,
    /// A menu or popup was open when the frame started: the keys, Esc too, are its own (U31).
    pub popup: bool,
}

const fn key(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);

/// Every shortcut, in the order of the Help list; a command with two keys has the one shown
/// first.
pub(crate) const SHORTCUTS: [(Command, KeyboardShortcut); 16] = [
    (Command::Undo, key(CTRL, Key::Z)),
    (Command::Redo, key(CTRL_SHIFT, Key::Z)),
    (Command::Redo, key(CTRL, Key::Y)),
    (Command::Refresh, key(Modifiers::NONE, Key::F5)),
    (Command::Fetch, key(CTRL_SHIFT, Key::T)),
    (Command::Pull, key(CTRL_SHIFT, Key::L)),
    (Command::Push, key(CTRL_SHIFT, Key::P)),
    (Command::ToggleStage, key(Modifiers::NONE, Key::Space)),
    (Command::Find, key(CTRL, Key::F)),
    (Command::Settings, crate::settings::SHORTCUT),
    (Command::NextTab, key(CTRL, Key::Tab)),
    (Command::PreviousTab, key(CTRL_SHIFT, Key::Tab)),
    (Command::ZoomIn, key(CTRL, Key::Equals)),
    (Command::ZoomIn, key(CTRL, Key::Plus)),
    (Command::ZoomOut, key(CTRL, Key::Minus)),
    (Command::ZoomReset, key(CTRL, Key::Num0)),
];

impl Command {
    /// Also while a text field has the keyboard: none of them is a key a text field uses.
    pub(crate) fn global(self) -> bool {
        matches!(
            self,
            Command::Find
                | Command::Settings
                | Command::NextTab
                | Command::PreviousTab
                | Command::ZoomIn
                | Command::ZoomOut
                | Command::ZoomReset
        )
    }

    /// The main shortcut, as tooltips and menus show it.
    pub(crate) fn shortcut(self) -> KeyboardShortcut {
        SHORTCUTS
            .iter()
            .find(|(c, _)| *c == self)
            .map(|(_, s)| *s)
            .expect("every command has a shortcut")
    }

    /// What it does, for the Help list.
    pub(crate) fn label(self) -> String {
        match self {
            Command::Undo => tr!("keys-undo"),
            Command::Redo => tr!("keys-redo"),
            Command::Refresh => tr!("keys-refresh"),
            Command::Fetch => tr!("keys-fetch"),
            Command::Pull => tr!("keys-pull"),
            Command::Push => tr!("keys-push"),
            Command::ToggleStage => tr!("keys-toggle-stage"),
            Command::Find => tr!("keys-find"),
            Command::Settings => tr!("keys-settings"),
            Command::NextTab => tr!("keys-next-tab"),
            Command::PreviousTab => tr!("keys-previous-tab"),
            Command::ZoomIn => tr!("keys-zoom-in"),
            Command::ZoomOut => tr!("keys-zoom-out"),
            Command::ZoomReset => tr!("keys-zoom-reset"),
        }
    }
}

/// The command of `key` pressed with `modifiers`. The exact modifiers first (Ctrl+Shift+Z is
/// redo, not undo); then ignoring a Shift the key needs (Ctrl++ is Ctrl+Shift+= on many
/// keyboards).
pub(crate) fn command(key: Key, modifiers: Modifiers) -> Option<Command> {
    let exact = SHORTCUTS
        .iter()
        .find(|(_, s)| s.logical_key == key && modifiers.matches_exact(s.modifiers));
    exact
        .or_else(|| {
            SHORTCUTS.iter().find(|(_, s)| {
                s.logical_key == key
                    && !s.modifiers.is_none()
                    && modifiers.matches_logically(s.modifiers)
            })
        })
        .map(|(c, _)| *c)
}

/// The commands pressed in this frame, taken out of the input so no widget sees them too.
/// `text`: a text field has the keyboard, only the [`Command::global`] ones.
pub(crate) fn take(ctx: &egui::Context, text: bool) -> Vec<Command> {
    let mut commands = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            match command(*key, *modifiers) {
                Some(command) if !text || command.global() => {
                    commands.push(command);
                    false
                }
                _ => true,
            }
        });
    });
    commands
}

/// A shortcut as the menus show it: the key's symbol ("Ctrl+,"), where egui writes its name
/// ("Ctrl+Comma"); macOS with egui's symbols.
pub(crate) fn shortcut_text(ctx: &egui::Context, shortcut: &KeyboardShortcut) -> String {
    if ctx.os().is_mac() {
        return ctx.format_shortcut(shortcut);
    }
    let names = egui::ModifierNames {
        is_short: true,
        ..egui::ModifierNames::NAMES
    };
    shortcut.format(&names, false)
}

/// The tooltip `hint` with the command's shortcut after it.
pub(crate) fn with_shortcut(ctx: &egui::Context, hint: &str, command: Command) -> String {
    tr!(
        "keys-hint",
        hint = hint,
        key = shortcut_text(ctx, &command.shortcut())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: Modifiers = Modifiers::SHIFT;

    #[test]
    fn keys_map_to_their_commands() {
        let cases = [
            (Key::Z, CTRL, Command::Undo),
            (Key::Z, CTRL_SHIFT, Command::Redo),
            (Key::Y, CTRL, Command::Redo),
            (Key::F5, Modifiers::NONE, Command::Refresh),
            (Key::L, CTRL_SHIFT, Command::Pull),
            (Key::P, CTRL_SHIFT, Command::Push),
            (Key::T, CTRL_SHIFT, Command::Fetch),
            (Key::Tab, CTRL, Command::NextTab),
            (Key::Tab, CTRL_SHIFT, Command::PreviousTab),
            (Key::Space, Modifiers::NONE, Command::ToggleStage),
            (Key::F, CTRL, Command::Find),
            (Key::Comma, CTRL, Command::Settings),
            (Key::Equals, CTRL, Command::ZoomIn),
            (Key::Plus, CTRL_SHIFT, Command::ZoomIn),
            (Key::Minus, CTRL, Command::ZoomOut),
            (Key::Num0, CTRL, Command::ZoomReset),
        ];
        for (key, modifiers, expected) in cases {
            assert_eq!(
                command(key, modifiers),
                Some(expected),
                "{key:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn other_keys_do_nothing() {
        assert_eq!(command(Key::Space, SHIFT), None, "Shift+Space");
        assert_eq!(command(Key::Space, CTRL), None);
        assert_eq!(command(Key::L, CTRL), None, "Ctrl+L without Shift");
        assert_eq!(command(Key::Z, Modifiers::NONE), None);
        assert_eq!(
            command(Key::Tab, Modifiers::NONE),
            None,
            "Tab moves the focus"
        );
        assert_eq!(command(Key::A, CTRL), None);
    }

    #[test]
    fn every_command_has_a_label_and_a_shortcut() {
        for (command, _) in SHORTCUTS {
            assert!(!command.label().is_empty());
            assert_eq!(
                command.shortcut(),
                SHORTCUTS.iter().find(|(c, _)| *c == command).unwrap().1
            );
        }
        assert_eq!(Command::Redo.shortcut(), key(CTRL_SHIFT, Key::Z));
    }

    /// One frame with `events`: the commands taken and what is left to the widgets.
    fn frame(text: bool, events: Vec<egui::Event>) -> (Vec<Command>, usize) {
        let ctx = egui::Context::default();
        let mut taken = Vec::new();
        let mut left = 0;
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            taken = take(ui.ctx(), text);
            left = ui.input(|i| i.events.len());
        });
        output.textures_delta.clear();
        (taken, left)
    }

    fn press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn the_keys_taken_do_not_reach_the_widgets() {
        let (taken, left) = frame(
            false,
            vec![
                press(Key::F5, Modifiers::NONE),
                press(Key::A, Modifiers::NONE),
            ],
        );
        assert_eq!((taken, left), (vec![Command::Refresh], 1));
    }

    #[test]
    fn a_text_field_keeps_its_keys() {
        let events = vec![
            press(Key::Z, CTRL),
            press(Key::Space, Modifiers::NONE),
            press(Key::F, CTRL),
        ];
        let (taken, left) = frame(true, events);
        assert_eq!((taken, left), (vec![Command::Find], 2));
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tab_tests;
