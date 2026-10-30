//! The app's preferences (theme, language, diff layout, automatic fetch, UI scale, dates of the
//! commit list), the "Settings…" window that changes them and their persistence between launches.
//!
//! They live in the eframe storage under their own key, next to the tabs, and never go through
//! the `Registry`: they do not touch the repo. Every field has a default and unknown fields or
//! values are ignored, so a save from an older or newer version keeps working.
//!
//! Adding a setting: a field of [`Settings`], a variant of [`Change`] (the window, a menu or a
//! view push it in an [`Action::Change`]) and its row in the section that shows it. Adding
//! a section: a variant of [`Section`] and its arm in `view/settings_view.rs`.

use project_engine::Platform;
use project_i18n::Lang;
use serde::{Deserialize, Serialize};

/// The actions of this area, applied by [`App`](crate::App).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Opens the "Settings…" window.
    Open,
    /// Closes it.
    Close,
    /// Shows that section of the window.
    Section(Section),
    /// Changes a setting (from the window, the theme menu or the diff view).
    Change(Change),
}

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

/// How often every open tab runs `remote.fetch` on its own (S50): off, 1, 5 or 15 minutes. Saved
/// as minutes (`0`: off); an unknown number (a newer version's) is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u32", into = "u32")]
pub(crate) enum FetchInterval {
    Off,
    One,
    #[default]
    Five,
    Fifteen,
}

impl FetchInterval {
    pub(crate) const ALL: [FetchInterval; 4] = [
        FetchInterval::Off,
        FetchInterval::One,
        FetchInterval::Five,
        FetchInterval::Fifteen,
    ];

    pub(crate) fn minutes(self) -> u32 {
        match self {
            FetchInterval::Off => 0,
            FetchInterval::One => 1,
            FetchInterval::Five => 5,
            FetchInterval::Fifteen => 15,
        }
    }

    /// The wait between two automatic fetches; `None`: off.
    pub(crate) fn duration(self) -> Option<std::time::Duration> {
        match self.minutes() {
            0 => None,
            m => Some(std::time::Duration::from_secs(u64::from(m) * 60)),
        }
    }
}

impl From<u32> for FetchInterval {
    fn from(minutes: u32) -> Self {
        FetchInterval::ALL
            .into_iter()
            .find(|i| i.minutes() == minutes)
            .unwrap_or_default()
    }
}

impl From<FetchInterval> for u32 {
    fn from(interval: FetchInterval) -> Self {
        interval.minutes()
    }
}

/// The size of the whole UI in percent, from [`Scale::MIN`] to [`Scale::MAX`] in steps of
/// [`Scale::STEP`] (Ctrl+= / Ctrl+- / Ctrl+0 too). Saved as the percent; a value out of range or
/// off the steps (a newer version's) is the default, 100%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u32", into = "u32")]
pub(crate) struct Scale(u32);

impl Scale {
    pub(crate) const MIN: u32 = 90;
    pub(crate) const MAX: u32 = 150;
    pub(crate) const STEP: u32 = 10;
    /// 100%.
    pub(crate) const NORMAL: Scale = Scale(100);

    pub(crate) fn percent(self) -> u32 {
        self.0
    }

    /// Every value, from the smallest.
    pub(crate) fn all() -> impl Iterator<Item = Scale> {
        (Self::MIN..=Self::MAX)
            .step_by(Self::STEP as usize)
            .map(Scale)
    }

    /// The egui zoom factor.
    pub(crate) fn factor(self) -> f32 {
        // At most 150: exact in an f32.
        u16::try_from(self.0).map_or(1.0, f32::from) / 100.0
    }

    /// One step larger, up to the maximum.
    pub(crate) fn larger(self) -> Scale {
        Scale((self.0 + Self::STEP).min(Self::MAX))
    }

    /// One step smaller, down to the minimum.
    pub(crate) fn smaller(self) -> Scale {
        Scale(self.0.saturating_sub(Self::STEP).max(Self::MIN))
    }
}

impl Default for Scale {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl From<u32> for Scale {
    fn from(percent: u32) -> Self {
        Scale::all().find(|s| s.0 == percent).unwrap_or_default()
    }
}

impl From<Scale> for u32 {
    fn from(scale: Scale) -> Self {
        scale.0
    }
}

/// How the commit list writes dates: `2023-11-14 22:13` or "3 h ago".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "&'static str")]
pub(crate) enum DateFormat {
    #[default]
    Absolute,
    Relative,
}

impl DateFormat {
    pub(crate) const ALL: [DateFormat; 2] = [DateFormat::Absolute, DateFormat::Relative];
}

impl From<String> for DateFormat {
    fn from(value: String) -> Self {
        match value.as_str() {
            "relative" => DateFormat::Relative,
            _ => DateFormat::Absolute,
        }
    }
}

impl From<DateFormat> for &'static str {
    fn from(format: DateFormat) -> Self {
        match format {
            DateFormat::Absolute => "absolute",
            DateFormat::Relative => "relative",
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
    /// The automatic fetch of every tab.
    pub fetch_minutes: FetchInterval,
    /// The size of the UI.
    pub scale: Scale,
    /// The dates of the commit list.
    pub dates: DateFormat,
}

/// One setting changed, from the window, the theme menu or the diff view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Change {
    Theme(Theme),
    Language(Language),
    DiffSplit(bool),
    FetchInterval(FetchInterval),
    Scale(Scale),
    Dates(DateFormat),
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
            fetch_minutes: FetchInterval::default(),
            scale: Scale::default(),
            dates: DateFormat::default(),
        }
    }

    pub(crate) fn change(&mut self, change: Change) {
        match change {
            Change::Theme(theme) => self.theme = theme,
            Change::Language(language) => self.language = language,
            Change::DiffSplit(split) => self.diff_split = split,
            Change::FetchInterval(interval) => self.fetch_minutes = interval,
            Change::Scale(scale) => self.scale = scale,
            Change::Dates(dates) => self.dates = dates,
        }
    }

    /// Theme, language and scale take effect at once: the texts change from the next frame.
    /// egui's own zoom keys are off: Ctrl+= / Ctrl+- / Ctrl+0 change [`Settings::scale`], so the
    /// setting and the keys agree and the zoom is saved.
    pub(crate) fn apply(&self, ctx: &egui::Context, platform: &dyn Platform) {
        ctx.set_theme(self.theme.preference());
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_zoom_factor(self.scale.factor());
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
    Remotes,
}

impl Section {
    pub(crate) const ALL: [Section; 2] = [Section::General, Section::Remotes];
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
            fetch_minutes: FetchInterval::Fifteen,
            scale: Scale(120),
            dates: DateFormat::Relative,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            json,
            r#"{"theme":"dark","language":"it","diff_split":true,"fetch_minutes":15,"scale":120,"dates":"relative"}"#
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
        assert_eq!(old.fetch_minutes, FetchInterval::Five, "default 5 minutes");
        // From a newer version: unknown fields and values are ignored.
        let newer: Settings = serde_json::from_str(
            r#"{"theme":"sepia","language":"de","diff_split":true,"fetch_minutes":7,"scale":95,"dates":"fuzzy","x":1}"#,
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
    fn fetch_intervals() {
        assert_eq!(FetchInterval::Off.duration(), None);
        assert_eq!(
            FetchInterval::One.duration(),
            Some(std::time::Duration::from_secs(60))
        );
        let off: Settings = serde_json::from_str(r#"{"fetch_minutes":0}"#).unwrap();
        assert_eq!(off.fetch_minutes, FetchInterval::Off);
    }

    #[test]
    fn the_scale_moves_by_steps_within_its_range() {
        assert_eq!(Scale::default().percent(), 100);
        assert_eq!(Scale::NORMAL.larger().percent(), 110);
        assert_eq!(Scale(Scale::MAX).larger().percent(), Scale::MAX);
        assert_eq!(Scale(Scale::MIN).smaller().percent(), Scale::MIN);
        assert_eq!(Scale::NORMAL.smaller().percent(), 90);
        assert_eq!(Scale::all().count(), 7);
        assert!((Scale(150).factor() - 1.5).abs() < f32::EPSILON);
        assert_eq!(Scale::from(300), Scale::NORMAL, "out of range: the default");
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
