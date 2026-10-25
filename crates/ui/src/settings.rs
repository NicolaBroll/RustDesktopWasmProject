//! The app's preferences (theme, language, diff layout), the "Settings…" window that changes them
//! and their persistence between launches.
//!
//! They live in the eframe storage under their own key, next to the tabs, and never go through
//! the `Registry`: they do not touch the repo. Every field has a default and unknown fields or
//! values are ignored, so a save from an older or newer version keeps working.
//!
//! Adding a setting: a field of [`Settings`], a variant of [`Change`] (the window, a menu or a
//! view push it in an [`crate::Action::Setting`]) and its row in the section that shows it. Adding
//! a section: a variant of [`Section`] and its arm in `view/settings_view.rs`.

use project_engine::Platform;
use project_i18n::Lang;
use serde::{Deserialize, Serialize};

/// Opens the window (Ctrl+, or Cmd+, on macOS).
pub(crate) const SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Comma);

/// Key of the settings in the eframe storage.
pub(crate) const KEY: &str = "settings";

/// Where egui kept the diff layout before it was a setting (read once, to carry it over).
const OLD_DIFF_SPLIT: &str = "diff-split";

/// Light, dark or as the system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "&'static str")]
pub(crate) enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub(crate) const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub(crate) fn preference(self) -> egui::ThemePreference {
        match self {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        }
    }

    fn from_preference(preference: egui::ThemePreference) -> Self {
        match preference {
            egui::ThemePreference::System => Theme::System,
            egui::ThemePreference::Light => Theme::Light,
            egui::ThemePreference::Dark => Theme::Dark,
        }
    }
}

/// An unknown value (a newer version's) is the default.
impl From<String> for Theme {
    fn from(value: String) -> Self {
        match value.as_str() {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

impl From<Theme> for &'static str {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// The UI language: as the system, or one of [`Lang`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "&'static str")]
pub(crate) enum Language {
    #[default]
    System,
    Fixed(Lang),
}

impl Language {
    pub(crate) const ALL: [Language; 3] = [
        Language::System,
        Language::Fixed(Lang::It),
        Language::Fixed(Lang::En),
    ];

    /// The language to show: the system's if it is one of ours, otherwise English.
    pub(crate) fn resolve(self, system: Option<&str>) -> Lang {
        match self {
            Language::Fixed(lang) => lang,
            Language::System => system
                .and_then(|code| Lang::ALL.into_iter().find(|l| l.code() == code))
                .unwrap_or(Lang::En),
        }
    }
}

impl From<String> for Language {
    fn from(value: String) -> Self {
        Lang::ALL
            .into_iter()
            .find(|l| l.code() == value)
            .map_or(Language::System, Language::Fixed)
    }
}

impl From<Language> for &'static str {
    fn from(language: Language) -> Self {
        match language {
            Language::System => "system",
            Language::Fixed(lang) => lang.code(),
        }
    }
}

/// What survives between launches; missing fields take their default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub theme: Theme,
    pub language: Language,
    /// The diff side by side (`true`) or unified.
    pub diff_split: bool,
}

/// One setting changed, from the window, the theme menu or the diff view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Change {
    Theme(Theme),
    Language(Language),
    DiffSplit(bool),
}

impl Settings {
    /// The saved settings; without a save (first launch, or a version before the settings) the
    /// theme and diff layout egui kept until then.
    pub(crate) fn load(storage: Option<&dyn eframe::Storage>, ctx: &egui::Context) -> Self {
        if let Some(settings) = storage.and_then(|s| eframe::get_value::<Settings>(s, KEY)) {
            return settings;
        }
        Settings {
            theme: Theme::from_preference(ctx.options(|o| o.theme_preference)),
            language: Language::System,
            diff_split: ctx.data_mut(|d| {
                d.get_persisted::<bool>(egui::Id::unique(OLD_DIFF_SPLIT))
                    .unwrap_or(false)
            }),
        }
    }

    pub(crate) fn change(&mut self, change: Change) {
        match change {
            Change::Theme(theme) => self.theme = theme,
            Change::Language(language) => self.language = language,
            Change::DiffSplit(split) => self.diff_split = split,
        }
    }

    /// Theme and language take effect at once: the texts change from the next frame.
    pub(crate) fn apply(&self, ctx: &egui::Context, platform: &dyn Platform) {
        ctx.set_theme(self.theme.preference());
        let lang = self.language.resolve(platform.system_language().as_deref());
        project_i18n::set_lang(lang);
        ctx.request_repaint();
    }
}

/// A part of the window, listed on its left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Section {
    #[default]
    General,
}

impl Section {
    pub(crate) const ALL: [Section; 1] = [Section::General];
}

/// The "Settings…" window: whether it is open and the section shown.
#[derive(Debug, Default)]
pub(crate) struct SettingsBox {
    pub open: bool,
    pub section: Section,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_saved_as_plain_names() {
        let settings = Settings {
            theme: Theme::Dark,
            language: Language::Fixed(Lang::It),
            diff_split: true,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            json,
            r#"{"theme":"dark","language":"it","diff_split":true}"#
        );
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), settings);
    }

    #[test]
    fn an_old_or_newer_save_keeps_what_it_can() {
        // Before the language and the diff layout existed.
        let old: Settings = serde_json::from_str(r#"{"theme":"light"}"#).unwrap();
        assert_eq!(
            old,
            Settings {
                theme: Theme::Light,
                ..Settings::default()
            }
        );
        // From a newer version: unknown fields and values are ignored.
        let newer: Settings = serde_json::from_str(
            r#"{"theme":"sepia","language":"de","diff_split":true,"fetch_minutes":5}"#,
        )
        .unwrap();
        assert_eq!(
            newer,
            Settings {
                diff_split: true,
                ..Settings::default()
            }
        );
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn the_system_language_falls_back_to_english() {
        assert_eq!(Language::System.resolve(Some("it")), Lang::It);
        assert_eq!(Language::System.resolve(Some("de")), Lang::En);
        assert_eq!(Language::System.resolve(None), Lang::En);
        assert_eq!(Language::Fixed(Lang::En).resolve(Some("it")), Lang::En);
    }

    #[test]
    fn without_a_save_the_theme_and_layout_egui_kept_carry_over() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.data_mut(|d| d.insert_persisted(egui::Id::unique(OLD_DIFF_SPLIT), true));
        let settings = Settings::load(None, &ctx);
        assert_eq!((settings.theme, settings.diff_split), (Theme::Dark, true));
        assert_eq!(settings.language, Language::System);
    }
}
