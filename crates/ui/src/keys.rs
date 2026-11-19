//! The keyboard shortcuts (U40): one table, read by the frame (which key does what), by the
//! toolbar tooltips and by Help → "Keyboard shortcuts". A shortcut only becomes the
//! [`Action`](crate::Action) of the button or menu entry it stands for: no logic of its own.
//!
//! - **Who**: [`App`](crate::App) applies the app's commands (tabs, settings, scale); the tab
//!   the others, with the same gates as its buttons.
//! - **When**: never with a popup or a menu open (the key belongs to it), nor with a modal
//!   window open; only the [`Command::global`] ones while a text field has the keyboard.
//! - **Focus** (U64): while a widget that is not a text field has the keyboard (Tab), the
//!   shortcuts work, but Space and Enter are that widget's, as a click, and the arrows move the
//!   focus.
//! - Ctrl+Enter ([`Command::Confirm`]) is read by the commit box and by each dialog
//!   ([`confirm`]), also from a text field and with the dialog open.
//! - The list selection keys (arrows, Page up/down, Home/End, Enter, Esc) stay in the tab's
//!   keyboard handling: they depend on what the center shows ([`list_keys`] for the Help list).

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
    /// Commits from the commit box, or confirms the dialog shown: never taken by [`take`], the
    /// box and the dialogs read it themselves ([`confirm`]).
    Confirm,
    /// The folder dialog of "Open repository…".
    OpenRepo,
    /// Closes the tab shown.
    CloseTab,
    /// The "New branch" dialog, from HEAD.
    NewBranch,
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
pub(crate) const SHORTCUTS: [(Command, KeyboardShortcut); 20] = [
    (Command::OpenRepo, key(CTRL, Key::O)),
    (Command::CloseTab, key(CTRL, Key::W)),
    (Command::Undo, key(CTRL, Key::Z)),
    (Command::Redo, key(CTRL_SHIFT, Key::Z)),
    (Command::Redo, key(CTRL, Key::Y)),
    (Command::Refresh, key(Modifiers::NONE, Key::F5)),
    (Command::Fetch, key(CTRL_SHIFT, Key::T)),
    (Command::Pull, key(CTRL_SHIFT, Key::L)),
    (Command::Push, key(CTRL_SHIFT, Key::P)),
    (Command::NewBranch, key(CTRL, Key::B)),
    (Command::ToggleStage, key(Modifiers::NONE, Key::Space)),
    (Command::Confirm, key(CTRL, Key::Enter)),
    (Command::Find, key(CTRL, Key::F)),
    (Command::Settings, crate::settings::SHORTCUT),
    (Command::NextTab, key(CTRL, Key::Tab)),
    (Command::PreviousTab, key(CTRL_SHIFT, Key::Tab)),
    (Command::ZoomIn, key(CTRL, Key::Equals)),
    (Command::ZoomIn, key(CTRL, Key::Plus)),
    (Command::ZoomOut, key(CTRL, Key::Minus)),
    (Command::ZoomReset, key(CTRL, Key::Num0)),
];

/// The keys of the lists, for the Help list: the keys and what they do. They are handled by
/// the tab's keyboard ([`crate::Tab`]), not through [`SHORTCUTS`].
pub(crate) fn list_keys() -> [(String, String); 4] {
    [
        (tr!("keys-list-arrows-keys"), tr!("keys-list-arrows")),
        (tr!("keys-list-enter-keys"), tr!("keys-list-enter")),
        (tr!("keys-list-escape-keys"), tr!("keys-list-escape")),
        (tr!("keys-list-tab-keys"), tr!("keys-list-tab")),
    ]
}

impl Command {
    /// Also while a text field has the keyboard: none of them is a key a text field uses
    /// (Ctrl+W deletes a word there, so closing the tab is not one of these).
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
            Command::Confirm => tr!("keys-confirm"),
            Command::OpenRepo => tr!("keys-open-repo"),
            Command::CloseTab => tr!("keys-close-tab"),
            Command::NewBranch => tr!("keys-new-branch"),
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

/// A text field has the keyboard: the keys are its own, but for the [`Command::global`] ones.
/// Another focused widget (a button or a row reached with Tab) is not typing: the shortcuts
/// work, and only Space, Enter and the arrows are that widget's.
pub(crate) fn typing(ctx: &egui::Context) -> bool {
    ctx.text_edit_focused()
}

/// Ctrl+Enter pressed in this frame, taken out of the input: the dialog shown confirms as with
/// its default button (the caller checks that the button is enabled). Called before the
/// dialog's text fields are drawn.
pub(crate) fn confirm(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&Command::Confirm.shortcut()))
}

/// The commands pressed in this frame, taken out of the input so no widget sees them too.
/// `text`: a text field has the keyboard, only the [`Command::global`] ones. While another
/// widget has it (Tab), Space is left to that widget: it is its click.
pub(crate) fn take(ctx: &egui::Context, text: bool) -> Vec<Command> {
    let mut commands = Vec::new();
    let focused = ctx.memory(|m| m.focused().is_some());
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
                Some(Command::Confirm) => true,
                Some(Command::ToggleStage) if focused => true,
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
            (Key::O, CTRL, Command::OpenRepo),
            (Key::W, CTRL, Command::CloseTab),
            (Key::B, CTRL, Command::NewBranch),
            (Key::Enter, CTRL, Command::Confirm),
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
        assert_eq!(command(Key::Enter, Modifiers::NONE), None, "Enter alone");
        assert_eq!(command(Key::O, Modifiers::NONE), None);
    }

    #[test]
    fn the_new_shortcuts_are_not_for_text_fields() {
        // Ctrl+W deletes a word in a field, Ctrl+O and Ctrl+B are left alone there too.
        for command in [Command::OpenRepo, Command::CloseTab, Command::NewBranch] {
            assert!(!command.global(), "{command:?}");
        }
    }

    #[test]
    fn the_help_list_has_the_list_keys_too() {
        for (keys, label) in list_keys() {
            assert!(!keys.is_empty() && !label.is_empty());
        }
        assert_eq!(
            shortcut_text(&egui::Context::default(), &Command::Confirm.shortcut()),
            "Ctrl+Enter"
        );
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
        frame_in(&egui::Context::default(), text, events)
    }

    fn frame_in(
        ctx: &egui::Context,
        text: bool,
        events: Vec<egui::Event>,
    ) -> (Vec<Command>, usize) {
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
    fn ctrl_enter_is_left_to_the_box_or_the_dialog_that_reads_it() {
        let ctx = egui::Context::default();
        let mut confirmed = (false, false);
        let input = egui::RawInput {
            events: vec![press(Key::Enter, CTRL)],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            assert!(take(ui.ctx(), false).is_empty());
            // The first reader takes it: a second one in the same frame sees nothing.
            confirmed = (confirm(ui.ctx()), confirm(ui.ctx()));
        });
        output.textures_delta.clear();
        assert_eq!(confirmed, (true, false));
        // Enter alone is not a confirmation.
        let input = egui::RawInput {
            events: vec![press(Key::Enter, Modifiers::NONE)],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| confirmed.0 = confirm(ui.ctx()));
        output.textures_delta.clear();
        assert!(!confirmed.0);
    }

    #[test]
    fn space_is_the_click_of_the_widget_that_has_the_keyboard() {
        let ctx = egui::Context::default();
        ctx.memory_mut(|m| m.request_focus(egui::Id::unique("a focused row")));
        let events = vec![
            press(Key::Space, Modifiers::NONE),
            press(Key::F5, Modifiers::NONE),
        ];
        let (taken, left) = frame_in(&ctx, false, events);
        assert_eq!((taken, left), (vec![Command::Refresh], 1));
    }

    #[test]
    fn a_text_field_keeps_its_keys() {
        let events = vec![
            press(Key::Z, CTRL),
            press(Key::Space, Modifiers::NONE),
            press(Key::F, CTRL),
            press(Key::W, CTRL),
            press(Key::O, CTRL),
            press(Key::B, CTRL),
        ];
        let (taken, left) = frame(true, events);
        assert_eq!((taken, left), (vec![Command::Find], 5));
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tab_tests;
