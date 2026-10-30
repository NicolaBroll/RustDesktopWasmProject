//! The "Settings…" window (S49): the sections on the left, the one chosen on the right. Every
//! change is applied at once (no "Save"): it leaves as an [`settings::Action::Change`].

use super::*;
use crate::settings::{
    self, Change, FetchInterval, Language, Section, Settings, SettingsBox, Theme,
};
use project_i18n::Lang;

/// Width of the column with the sections.
const SECTIONS_WIDTH: f32 = 130.0;
/// Space between the sections and the content, with the line in the middle.
const GAP: f32 = 16.0;

pub(crate) fn theme_label(theme: Theme) -> String {
    match theme {
        Theme::System => tr!("settings-theme-system"),
        Theme::Light => tr!("settings-theme-light"),
        Theme::Dark => tr!("settings-theme-dark"),
    }
}

/// The language names are written in their own language, the same in every UI language.
fn language_label(language: Language) -> String {
    match language {
        Language::System => tr!("settings-language-system"),
        Language::Fixed(Lang::It) => "Italiano".to_owned(),
        Language::Fixed(Lang::En) => "English".to_owned(),
    }
}

fn section_title(section: Section) -> String {
    match section {
        Section::General => tr!("settings-general"),
        Section::Remotes => tr!("settings-remotes"),
    }
}

/// The open window; nothing if it is closed.
pub(crate) fn settings_window(
    ctx: &egui::Context,
    window: &SettingsBox,
    settings: &Settings,
    actions: &mut Vec<Action>,
) {
    if !window.open {
        return;
    }
    let modal = egui::Modal::new(egui::Id::unique("settings-window")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(tr!("settings-title"));
        ui.add_space(6.0);
        let row = ui.horizontal_top(|ui| {
            let sections = ui.vertical(|ui| {
                ui.set_width(SECTIONS_WIDTH);
                for section in Section::ALL {
                    let label = section_title(section);
                    if ui
                        .selectable_label(window.section == section, label)
                        .clicked()
                    {
                        actions.push(Action::Settings(settings::Action::Section(section)));
                    }
                }
            });
            ui.add_space(GAP);
            ui.vertical(|ui| match window.section {
                Section::General => general(ui, settings, actions),
                Section::Remotes => remotes(ui, settings, actions),
            });
            sections.response.rect.max.x + GAP / 2.0
        });
        // The line between the sections and the content, as tall as the taller of the two (a
        // separator would take all the height the modal offers).
        let rect = row.response.rect;
        let x = row.inner;
        ui.painter().vline(
            x,
            rect.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.add_space(8.0);
        ui.separator();
        egui::Sides::new()
            .show(ui, |_| {}, |ui| ui.button(tr!("settings-close")).clicked())
            .1
    });
    if modal.inner || modal.should_close() {
        actions.push(Action::Settings(settings::Action::Close));
    }
}

/// Theme and language.
fn general(ui: &mut Ui, settings: &Settings, actions: &mut Vec<Action>) {
    ui.strong(tr!("settings-theme"));
    ui.horizontal(|ui| {
        for theme in Theme::ALL {
            if ui
                .radio(settings.theme == theme, theme_label(theme))
                .clicked()
                && settings.theme != theme
            {
                actions.push(Action::Settings(settings::Action::Change(Change::Theme(
                    theme,
                ))));
            }
        }
    });
    ui.add_space(10.0);
    ui.strong(tr!("settings-language"));
    ui.horizontal(|ui| {
        for language in Language::ALL {
            if ui
                .radio(settings.language == language, language_label(language))
                .clicked()
                && settings.language != language
            {
                actions.push(Action::Settings(settings::Action::Change(
                    Change::Language(language),
                )));
            }
        }
    });
    ui.weak(tr!("settings-language-hint"));
}

fn interval_label(interval: FetchInterval) -> String {
    match interval {
        FetchInterval::Off => tr!("settings-fetch-off"),
        other => tr!("settings-fetch-minutes", n = other.minutes()),
    }
}

/// The automatic fetch (S50).
fn remotes(ui: &mut Ui, settings: &Settings, actions: &mut Vec<Action>) {
    ui.strong(tr!("settings-fetch"));
    ui.horizontal_wrapped(|ui| {
        for interval in FetchInterval::ALL {
            if ui
                .radio(settings.fetch_minutes == interval, interval_label(interval))
                .clicked()
                && settings.fetch_minutes != interval
            {
                actions.push(Action::Settings(settings::Action::Change(
                    Change::FetchInterval(interval),
                )));
            }
        }
    });
    ui.weak(tr!("settings-fetch-hint"));
}
