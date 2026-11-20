//! The "Settings…" window (S49): the sections on the left, the one chosen on the right. Every
//! change is applied at once (no "Save"): it leaves as an [`settings::Action::Change`]. The
//! "Git" section (S65) is the exception: git's name and email are git's configuration, written
//! only by its "Save" button ([`crate::identity`]).

use super::*;
use crate::identity::{self, IdentityBox};
use crate::keys::{self, Command};
use crate::settings::{
    self, Change, DateFormat, FetchInterval, Language, Scale, Section, Settings, SettingsBox, Theme,
};
use project_engine::{IdentityKey, IdentityScope};
use project_i18n::Lang;

/// What the pages of the window show.
pub(crate) struct SettingsPages<'a> {
    pub settings: &'a Settings,
    /// The "Git" section.
    pub identity: &'a IdentityBox,
    /// The tab shown is writing: "Save" of the "Git" section waits.
    pub writing: bool,
}

/// Width of the column with the sections.
const SECTIONS_WIDTH: f32 = 130.0;
/// Space between the sections and the content, with the line in the middle.
const GAP: f32 = 16.0;
/// What the window has around a page, top to bottom: the title, the line, "Close", the margins.
const CHROME_HEIGHT: f32 = 130.0;

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
        Section::Git => tr!("settings-git"),
    }
}

/// The open window; nothing if it is closed.
pub(crate) fn settings_window(
    ctx: &egui::Context,
    window: &SettingsBox,
    pages: &SettingsPages<'_>,
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
            ui.vertical(|ui| {
                // One size for every page: the window is as tall as its tallest page, so it
                // neither resizes nor moves (a modal is centered) when the page changes.
                let tallest = tallest_page(ui, pages);
                // In a low window (or at a large scale) the page scrolls: "Close" stays in
                // view. The "Git" page grows with its notes and with the error of a save.
                let room = (ctx.content_rect().height() - CHROME_HEIGHT).max(120.0);
                let height = tallest.min(room);
                egui::ScrollArea::vertical()
                    .id_salt("settings-page")
                    .max_height(height)
                    // egui gives a scrolled area in a modal 64 px: the height above wins.
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| page(ui, window.section, pages, actions));
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

/// The content of a page of the window.
fn page(ui: &mut Ui, section: Section, pages: &SettingsPages<'_>, actions: &mut Vec<Action>) {
    match section {
        Section::General => general(ui, pages.settings, actions),
        Section::Remotes => remotes(ui, pages.settings, actions),
        Section::Git => git(ui, pages, actions),
    }
}

/// The height of the tallest page in the room `ui` has: every page is laid out without being
/// painted (and without taking clicks), since their heights depend on the language, the scale
/// and the width.
fn tallest_page(ui: &mut Ui, pages: &SettingsPages<'_>) -> f32 {
    let room = ui.available_rect_before_wrap();
    Section::ALL
        .into_iter()
        .enumerate()
        .map(|(index, section)| {
            let builder = egui::UiBuilder::new()
                .id_salt(("settings-measure", index))
                .max_rect(room)
                .invisible();
            let mut child = ui.new_child(builder);
            page(&mut child, section, pages, &mut Vec::new());
            child.min_rect().height()
        })
        .fold(0.0, f32::max)
}

/// Theme, language, UI scale, dates, the detached HEAD confirmation and the external editor.
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
    ui.add_space(10.0);
    scale(ui, settings.scale, actions);
    ui.add_space(10.0);
    ui.strong(tr!("settings-dates"));
    ui.horizontal(|ui| {
        for format in DateFormat::ALL {
            let label = match format {
                DateFormat::Absolute => tr!("settings-dates-absolute"),
                DateFormat::Relative => tr!("settings-dates-relative"),
            };
            if ui.radio(settings.dates == format, label).clicked() && settings.dates != format {
                actions.push(Action::Settings(settings::Action::Change(Change::Dates(
                    format,
                ))));
            }
        }
    });
    ui.add_space(10.0);
    let mut ask = !settings.skip_detach_confirm;
    if ui
        .checkbox(&mut ask, tr!("settings-ask-detach"))
        .on_hover_text(tr!("settings-ask-detach-hint"))
        .changed()
    {
        actions.push(Action::Settings(settings::Action::Change(
            Change::SkipDetachConfirm(!ask),
        )));
    }
    ui.add_space(10.0);
    editor(ui, &settings.editor, actions);
}

/// The "External editor" command line (S60): every key leaves as a change, applied at once.
fn editor(ui: &mut Ui, current: &str, actions: &mut Vec<Action>) {
    ui.strong(tr!("settings-editor"));
    let mut text = current.to_owned();
    let edit = egui::TextEdit::singleline(&mut text)
        .id_salt("settings-editor")
        .hint_text(tr!("settings-editor-placeholder"))
        .desired_width(f32::INFINITY);
    if ui.add(edit).changed() && text != current {
        actions.push(Action::Settings(settings::Action::Change(Change::Editor(
            text,
        ))));
    }
    ui.weak(tr!("settings-editor-hint"));
}

/// The UI scale, with the keys that change it too.
fn scale(ui: &mut Ui, current: Scale, actions: &mut Vec<Action>) {
    ui.strong(tr!("settings-scale"));
    egui::ComboBox::from_id_salt("settings-scale")
        .selected_text(tr!("settings-scale-value", n = current.percent()))
        .show_ui(ui, |ui| {
            for scale in Scale::all() {
                let label = tr!("settings-scale-value", n = scale.percent());
                if ui.selectable_label(current == scale, label).clicked() && current != scale {
                    actions.push(Action::Settings(settings::Action::Change(Change::Scale(
                        scale,
                    ))));
                }
            }
        });
    let key = |command: Command| keys::shortcut_text(ui.ctx(), &command.shortcut());
    ui.weak(tr!(
        "settings-scale-hint",
        larger = key(Command::ZoomIn),
        smaller = key(Command::ZoomOut),
        reset = key(Command::ZoomReset)
    ));
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

/// Git's name and email (S65): the form with where "Save" writes, then what git uses and why.
fn git(ui: &mut Ui, pages: &SettingsPages<'_>, actions: &mut Vec<Action>) {
    let form = pages.identity;
    ui.strong(tr!("settings-identity"));
    ui.weak(tr!("settings-identity-hint"));
    ui.add_space(8.0);
    let Some(values) = form.identity() else {
        // Nothing to show yet: the read is in progress, or it failed.
        match form.read_error() {
            Some(error) => {
                let text = tr!("settings-identity-read-failed", error = error);
                ui.add(
                    egui::Label::new(RichText::new(text).color(ui.visuals().error_fg_color)).wrap(),
                );
                if ui.button(tr!("settings-identity-retry")).clicked() {
                    push_identity(actions, identity::Action::Reload);
                }
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr!("settings-identity-reading"));
                });
            }
        }
        return;
    };
    // Before the fields: a single-line field gives up the focus when it sees Enter.
    let enter = default_enter(ui);
    let idle = !form.is_reading() && !form.is_saving();
    ui.add_enabled_ui(idle, |ui| {
        ui.horizontal_wrapped(|ui| {
            let mut choice = |ui: &mut Ui, scope: IdentityScope, label: String, hint: String| {
                let chosen = form.scope() == scope;
                if ui.radio(chosen, label).on_hover_text(hint).clicked() && !chosen {
                    push_identity(actions, identity::Action::Scope(scope));
                }
            };
            choice(
                ui,
                IdentityScope::Global,
                tr!("settings-identity-global"),
                tr!("settings-identity-global-hint"),
            );
            // "This repository" is the one open in the tab shown.
            if let Some(path) = form.repo() {
                choice(
                    ui,
                    IdentityScope::Local,
                    tr!("settings-identity-local"),
                    tr!("settings-identity-local-hint", repo = repo_name(path)),
                );
            }
        });
        ui.add_space(4.0);
        identity_field(ui, form, IdentityKey::Name, actions);
        ui.add_space(4.0);
        identity_field(ui, form, IdentityKey::Email, actions);
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let can = form.can_save() && !pages.writing;
        let hint = if pages.writing || !idle {
            wait()
        } else {
            tr!("settings-identity-nothing")
        };
        let save = ui
            .add_enabled(can, primary_button(ui, tr!("settings-identity-save"), can))
            .on_disabled_hover_text(hint);
        if can && (save.clicked() || enter) {
            push_identity(actions, identity::Action::Save);
        }
        if form.is_saving() {
            ui.spinner();
            ui.label(tr!("settings-identity-saving"));
        } else if form.is_reading() {
            ui.spinner();
            ui.label(tr!("settings-identity-reading"));
        }
    });
    match form.outcome() {
        Some(Ok(notice)) => {
            ui.weak(notice);
        }
        Some(Err(error)) => {
            let text = tr!("settings-identity-save-failed", error = error);
            ui.add(egui::Label::new(RichText::new(text).color(ui.visuals().error_fg_color)).wrap());
        }
        None => {}
    }
    ui.add_space(10.0);
    identity_in_use(ui, values, form.repo());
}

fn push_identity(actions: &mut Vec<Action>, action: identity::Action) {
    actions.push(Action::Settings(settings::Action::Identity(action)));
}

/// The name or the email field: every key leaves as an action, nothing is written until
/// "Save". Left empty with "Only for this repository" it shows the value it falls back to.
fn identity_field(ui: &mut Ui, form: &IdentityBox, key: IdentityKey, actions: &mut Vec<Action>) {
    let (label, salt, placeholder) = match key {
        IdentityKey::Name => (
            tr!("settings-identity-name"),
            "settings-identity-name",
            tr!("settings-identity-name-placeholder"),
        ),
        IdentityKey::Email => (
            tr!("settings-identity-email"),
            "settings-identity-email",
            tr!("settings-identity-email-placeholder"),
        ),
    };
    let hint = match form.inherited(key) {
        Some(value) => tr!("settings-identity-inherited", value = value),
        None => placeholder,
    };
    ui.label(label);
    let current = form.text(key);
    let mut text = current.to_owned();
    let edit = egui::TextEdit::singleline(&mut text)
        .id_salt(salt)
        .hint_text(hint)
        .desired_width(f32::INFINITY);
    let response = ui.add(edit);
    // Asked once by "Set name and email…"; never on the page laid out only to be measured.
    if form.focus() == Some(key) && ui.is_enabled() && ui.is_visible() {
        response.request_focus();
        push_identity(actions, identity::Action::Focused);
    }
    if response.changed() && text != current {
        push_identity(
            actions,
            match key {
                IdentityKey::Name => identity::Action::Name(text),
                IdentityKey::Email => identity::Action::Email(text),
            },
        );
    }
}

/// What git uses for the commits and where each value comes from, a repo's own value that
/// hides the one for all repositories, and the environment variables that win over both.
fn identity_in_use(ui: &mut Ui, values: &project_engine::Identity, repo: Option<&str>) {
    ui.strong(match repo {
        Some(path) => tr!("settings-identity-used-repo", repo = repo_name(path)),
        None => tr!("settings-identity-used"),
    });
    let rows = [
        (tr!("settings-identity-name"), &values.name),
        (tr!("settings-identity-email"), &values.email),
    ];
    for (label, used) in rows {
        ui.horizontal_wrapped(|ui| {
            ui.weak(label);
            match used.value.as_deref() {
                // Set to nothing: it still hides the scopes below it.
                Some("") => ui.label(tr!("settings-identity-empty")),
                Some(value) => ui.label(value),
                None => ui.label(tr!("settings-identity-not-set")),
            };
            if let Some(scope) = used.scope {
                ui.weak(identity::source_text(scope));
            }
        });
    }
    if let Some(global) = identity::hidden(values, IdentityKey::Name) {
        ui.label(tr!("settings-identity-hidden-name", global = global));
    }
    if let Some(global) = identity::hidden(values, IdentityKey::Email) {
        ui.label(tr!("settings-identity-hidden-email", global = global));
    }
    let (wins, fallback) = identity::environment(values);
    let warn = ui.visuals().warn_fg_color;
    if !wins.is_empty() {
        let text = tr!(
            "settings-identity-env",
            n = wins.len(),
            names = wins.join(", ")
        );
        ui.add(egui::Label::new(RichText::new(text).color(warn)).wrap());
    }
    if fallback {
        ui.add(
            egui::Label::new(RichText::new(tr!("settings-identity-env-email")).color(warn)).wrap(),
        );
    }
}
