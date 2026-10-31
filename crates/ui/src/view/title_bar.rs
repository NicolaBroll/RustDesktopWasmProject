//! The bar at the top of the window, VS Code style: the app icon, the "File" menu (open a repo, the
//! recent ones), the "Help" menu (keyboard shortcuts, about) and, when the app draws the window frame itself, the title,
//! the drag area, the minimize, maximize and close buttons and the edges to resize the window.

use super::*;
use crate::keys::{self, SHORTCUTS};
use crate::settings;
use crate::tabs;
use egui::{CursorIcon, Pos2, Rect, ResizeDirection, Stroke, StrokeKind, ViewportCommand};

/// The app icon in the bar (source in `assets/icon.svg`).
const ICON_SMALL: &[u8] = include_bytes!("../../../../assets/favicon-32.png");
/// The app icon in the "About" dialog.
const ICON_LARGE: &[u8] = include_bytes!("../../../../assets/icon-256.png");
/// Where the project lives, shown in the "About" dialog.
const HOMEPAGE: &str = "https://github.com/NicolaBroll/RustDesktopWasmProject";

/// Who draws the window frame. The UI has no `cfg(target_os)`: the app chooses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowFrame {
    /// The platform (the browser, or a desktop with its own decorations): only the menus.
    #[default]
    Native,
    /// No decorations: the bar is the title bar, with the window buttons on the right; the app
    /// also handles the drag and the edges to resize.
    Custom,
    /// macOS with a transparent title bar: the native buttons stay on the left, over the bar.
    MacOs,
}

/// Height of the bar with a custom frame (as VS Code).
const BAR_HEIGHT: f32 = 30.0;
/// Width of a window button.
const BUTTON_WIDTH: f32 = 46.0;
/// Room left for the macOS buttons.
const MAC_BUTTONS_WIDTH: f32 = 76.0;
/// Width of the edges that resize the window.
const RESIZE_EDGE: f32 = 5.0;

/// What the bar shows and enables.
pub(crate) struct TitleBar<'a> {
    pub(crate) frame: WindowFrame,
    /// The repo of the tab shown.
    pub(crate) repo: Option<&'a RepoView>,
    pub(crate) can_pick: bool,
    /// The platform opens the file manager and the terminal.
    pub(crate) can_launch: bool,
    pub(crate) recent: &'a [RecentRepo],
}

/// The bar in its own panel, above everything else.
pub(crate) fn title_bar_panel(ui: &mut Ui, bar: &TitleBar<'_>, actions: &mut Vec<Action>) {
    let visuals = ui.visuals();
    let mut frame = egui::Frame::new().fill(visuals.extreme_bg_color);
    let panel = egui::Panel::top("title-bar").show_separator_line(false);
    let panel = if bar.frame == WindowFrame::Native {
        frame = frame.inner_margin(egui::Margin::symmetric(4, 2));
        panel
    } else {
        panel.exact_size(BAR_HEIGHT)
    };
    panel
        .frame(frame)
        .show(ui, |ui| title_bar(ui, bar, actions));
    about_dialog(ui.ctx());
    shortcuts_dialog(ui.ctx());
}

fn title_bar(ui: &mut Ui, bar: &TitleBar<'_>, actions: &mut Vec<Action>) {
    let rect = ui.max_rect();
    if bar.frame != WindowFrame::Native {
        // First, so the menus and the buttons drawn afterwards are above it.
        let drag = ui.interact(rect, ui.scope_id().with("drag"), Sense::click_and_drag());
        if drag.double_clicked() {
            toggle_maximized(ui.ctx());
        } else if drag.drag_started_by(egui::PointerButton::Primary)
            && resize_direction(ui.ctx(), bar.frame).is_none()
        {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            handed_to_system(ui.ctx());
        }
    }
    ui.horizontal_centered(|ui| {
        ui.add_space(match bar.frame {
            WindowFrame::MacOs => MAC_BUTTONS_WIDTH,
            WindowFrame::Native | WindowFrame::Custom => 8.0,
        });
        app_icon(ui, ICON_SMALL, 16.0);
        // The end of the last menu: the bar itself takes the whole width.
        let menus_end = egui::MenuBar::new()
            .ui(ui, |ui| {
                menus(ui, bar, actions);
                ui.cursor().min.x
            })
            .inner;
        if bar.frame == WindowFrame::Native {
            return;
        }
        let mut free = rect;
        free.min.x = menus_end;
        if bar.frame == WindowFrame::Custom {
            // At the right edge of the bar, not of the layout: its margin would push them out of
            // the window.
            let buttons = Rect::from_min_max(
                Pos2::new(rect.max.x - 3.0 * BUTTON_WIDTH, rect.min.y),
                rect.max,
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(buttons)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    window_buttons(ui);
                },
            );
            free.max.x = buttons.min.x;
        }
        title(ui, bar, rect, free);
    });
}

/// "repo — BinariGit" at the center of the window, if it fits between the menus and the buttons.
fn title(ui: &Ui, bar: &TitleBar<'_>, bar_rect: Rect, free: Rect) {
    let text = match bar.repo {
        Some(repo) => tr!("title-bar-title-repo", repo = repo_name(&repo.path)),
        None => tr!("title-bar-title"),
    };
    let color = if ui.ctx().input(|i| i.viewport().focused.unwrap_or(true)) {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let margin = 12.0;
    let width = galley.size().x;
    if width + 2.0 * margin > free.width() {
        return;
    }
    let x = (bar_rect.center().x - width / 2.0)
        .max(free.min.x + margin)
        .min(free.max.x - margin - width);
    let pos = Pos2::new(x, bar_rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

/// Only what is not a git command on the repo shown (those are in the toolbar): opening repos,
/// its folder in the file manager or a terminal, the settings.
fn menus(ui: &mut Ui, bar: &TitleBar<'_>, actions: &mut Vec<Action>) {
    ui.menu_button(tr!("title-bar-file"), |ui| {
        // Room for the shortcut of "Settings…" after the longest item.
        ui.set_min_width(190.0);
        if bar.can_pick
            && ui
                .button(tr!("title-bar-open"))
                .on_hover_text(tr!("title-bar-open-hint"))
                .clicked()
        {
            actions.push(Action::Tabs(tabs::Action::PickFolder));
        }
        create_buttons(ui, actions);
        recent_menu(ui, bar.recent, actions);
        if bar.can_launch {
            ui.separator();
            launch_buttons(ui, bar.repo.is_some(), actions);
        }
        ui.separator();
        let shortcut = keys::shortcut_text(ui.ctx(), &settings::SHORTCUT);
        if ui
            .add(egui::Button::new(tr!("title-bar-settings")).shortcut_text(shortcut))
            .clicked()
        {
            actions.push(Action::Settings(settings::Action::Open));
            ui.close();
        }
    });
    ui.menu_button(tr!("title-bar-help"), |ui| {
        if ui.button(tr!("title-bar-shortcuts")).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(shortcuts_id(), true));
            ui.close();
        }
        if ui.button(tr!("title-bar-about")).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(about_id(), true));
            ui.close();
        }
    });
}

/// "Show in file manager" and "Open terminal here", on the folder of the repo shown: off without
/// a repo.
fn launch_buttons(ui: &mut Ui, has_repo: bool, actions: &mut Vec<Action>) {
    let items = [
        (
            tr!("title-bar-show-folder"),
            tr!("title-bar-show-folder-hint"),
            tabs::Action::ShowFolder,
        ),
        (
            tr!("title-bar-open-terminal"),
            tr!("title-bar-open-terminal-hint"),
            tabs::Action::OpenTerminal,
        ),
    ];
    for (label, hint, action) in items {
        if ui
            .add_enabled(has_repo, egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(tr!("title-bar-launch-no-repo"))
            .clicked()
        {
            actions.push(Action::Tabs(action));
            ui.close();
        }
    }
}

/// The app icon, `size` points wide, decoded once per image.
fn app_icon(ui: &mut Ui, png: &'static [u8], size: f32) {
    let id = egui::Id::unique(("app-icon", png.len()));
    let texture = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(id));
    let texture = texture.or_else(|| {
        let icon = eframe::icon_data::from_png_bytes(png).ok()?;
        let size = [icon.width as usize, icon.height as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
        let texture = ui
            .ctx()
            .load_texture("app-icon", image, egui::TextureOptions::LINEAR);
        ui.ctx().data_mut(|d| d.insert_temp(id, texture.clone()));
        Some(texture)
    });
    if let Some(texture) = texture {
        ui.add(egui::Image::new(&texture).fit_to_exact_size(vec2(size, size)));
    }
}

fn about_id() -> egui::Id {
    egui::Id::unique("title-bar-about")
}

/// "About": icon, name, version, license and where the project lives.
fn about_dialog(ctx: &egui::Context) {
    if !ctx.data(|d| d.get_temp::<bool>(about_id()).unwrap_or(false)) {
        return;
    }
    let modal = egui::Modal::new(egui::Id::unique("about-dialog")).show(ctx, |ui| {
        ui.set_width(300.0);
        let mut close = false;
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            app_icon(ui, ICON_LARGE, 64.0);
            ui.add_space(4.0);
            ui.heading(tr!("title-bar-title"));
            ui.label(tr!(
                "title-bar-about-version",
                version = env!("CARGO_PKG_VERSION")
            ));
            ui.add_space(4.0);
            ui.label(tr!("title-bar-about-description"));
            ui.weak(tr!(
                "title-bar-about-license",
                license = env!("CARGO_PKG_LICENSE")
            ));
            ui.hyperlink(HOMEPAGE);
            ui.add_space(8.0);
            close = ui.button(tr!("title-bar-about-close")).clicked();
        });
        close
    });
    if modal.inner || modal.should_close() {
        ctx.data_mut(|d| d.remove_temp::<bool>(about_id()));
    }
}

fn shortcuts_id() -> egui::Id {
    egui::Id::unique("title-bar-shortcuts")
}

/// Help → "Keyboard shortcuts": every shortcut of [`SHORTCUTS`], those of one command on one row.
fn shortcuts_dialog(ctx: &egui::Context) {
    if !ctx.data(|d| d.get_temp::<bool>(shortcuts_id()).unwrap_or(false)) {
        return;
    }
    let modal = egui::Modal::new(egui::Id::unique("shortcuts-dialog")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(tr!("keys-title"));
        ui.add_space(6.0);
        egui::Grid::new("shortcuts-grid")
            .num_columns(2)
            .spacing(vec2(24.0, 6.0))
            .striped(true)
            .show(ui, |ui| {
                let mut shown = Vec::new();
                for (command, _) in SHORTCUTS {
                    if shown.contains(&command) {
                        continue;
                    }
                    shown.push(command);
                    let keys: Vec<String> = SHORTCUTS
                        .iter()
                        .filter(|(c, _)| *c == command)
                        .map(|(_, s)| keys::shortcut_text(ctx, s))
                        .collect();
                    ui.label(RichText::new(keys.join(" · ")).monospace());
                    ui.label(command.label());
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.weak(tr!("keys-list-hint"));
        ui.add_space(8.0);
        egui::Sides::new()
            .show(ui, |_| {}, |ui| ui.button(tr!("keys-close")).clicked())
            .1
    });
    if modal.inner || modal.should_close() {
        ctx.data_mut(|d| d.remove_temp::<bool>(shortcuts_id()));
    }
}

/// Set when the app hands the move or the resize of the window to the system: from then on the
/// system has the mouse, and the release of the button never reaches the app.
fn moving_id() -> egui::Id {
    egui::Id::unique("title-bar-moving")
}

/// The system now moves or resizes the window: the next frame gets the release it keeps.
fn handed_to_system(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(moving_id(), true));
    ctx.request_repaint();
}

/// After a move of the window started from the bar, or a resize from its edges, gives egui the
/// release of the button that the system kept: otherwise egui still sees a drag, and no widget
/// lights up under the mouse until the next click. The release happens away from the window, so
/// it is never a click on the widget under the edge (a resize starts on the press, without
/// moving); the next real movement brings the pointer back. Called before each frame with its
/// input.
pub(crate) fn end_window_move(ctx: &egui::Context, input: &mut egui::RawInput) {
    if ctx
        .data_mut(|d| d.remove_temp::<bool>(moving_id()))
        .is_none()
    {
        return;
    }
    let away = ctx.content_rect().min - vec2(10_000.0, 10_000.0);
    input.events.extend([
        egui::Event::PointerMoved(away),
        egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: ctx.input(|i| i.modifiers),
        },
        egui::Event::PointerGone,
    ]);
}

#[derive(Debug, Clone, Copy)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Close, maximize (or restore) and minimize, from the right.
fn window_buttons(ui: &mut Ui) {
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    if window_button(ui, WindowButton::Close)
        .on_hover_text(tr!("title-bar-close"))
        .clicked()
    {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let (kind, hint) = if maximized {
        (WindowButton::Restore, tr!("title-bar-restore"))
    } else {
        (WindowButton::Maximize, tr!("title-bar-maximize"))
    };
    if window_button(ui, kind).on_hover_text(hint).clicked() {
        toggle_maximized(ui.ctx());
    }
    if window_button(ui, WindowButton::Minimize)
        .on_hover_text(tr!("title-bar-minimize"))
        .clicked()
    {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// A flat button as tall as the bar, with the symbol drawn in 1 px lines (as Windows 11).
fn window_button(ui: &mut Ui, kind: WindowButton) -> egui::Response {
    let size = vec2(BUTTON_WIDTH, ui.available_height());
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.visuals();
    let mut color = visuals.text_color();
    if response.hovered() {
        let fill = match kind {
            WindowButton::Close => {
                color = Color32::WHITE;
                if response.is_pointer_button_down_on() {
                    theme::CLOSE_HOVER.gamma_multiply(0.8)
                } else {
                    theme::CLOSE_HOVER
                }
            }
            _ if response.is_pointer_button_down_on() => visuals.widgets.active.weak_bg_fill,
            _ => visuals.widgets.hovered.weak_bg_fill,
        };
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    let painter = ui.painter();
    let stroke = Stroke::new(1.0, color);
    // On the pixel grid, so the 1 px lines stay sharp.
    let center = rect.center();
    let center = Pos2::new(
        painter.round_to_pixel_center(center.x),
        painter.round_to_pixel_center(center.y),
    );
    let half = 5.0;
    match kind {
        WindowButton::Minimize => {
            painter.line_segment([center - vec2(half, 0.0), center + vec2(half, 0.0)], stroke);
        }
        WindowButton::Maximize => {
            let square = Rect::from_center_size(center, vec2(2.0 * half, 2.0 * half));
            painter.rect_stroke(square, 1.0, stroke, StrokeKind::Middle);
        }
        WindowButton::Restore => {
            // A square in front and the corner of the one behind it.
            let front = Rect::from_center_size(
                center + vec2(-1.0, 1.0),
                vec2(2.0 * half - 2.0, 2.0 * half - 2.0),
            );
            painter.rect_stroke(front, 1.0, stroke, StrokeKind::Middle);
            let back = front.translate(vec2(2.0, -2.0));
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
        }
        WindowButton::Close => {
            let d = half - 0.5;
            painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], stroke);
            painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
        }
    }
    response
}

fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

/// The edges of a window without decorations: the cursor changes over them and a click starts the
/// resize. Called at the end of the frame, so its cursor wins over the widgets'. A thin border
/// separates the window from what is behind it.
pub(crate) fn window_edges(ctx: &egui::Context, frame: WindowFrame) {
    if frame != WindowFrame::Custom || maximized_or_fullscreen(ctx) {
        return;
    }
    let border = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("window-border"),
    ));
    let stroke = ctx.global_style().visuals.widgets.noninteractive.bg_stroke;
    border.rect_stroke(ctx.content_rect(), 0.0, stroke, StrokeKind::Inside);
    let Some(direction) = resize_direction(ctx, frame) else {
        return;
    };
    ctx.set_cursor_icon(resize_cursor(direction));
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        handed_to_system(ctx);
    }
}

fn maximized_or_fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        let viewport = i.viewport();
        viewport.maximized.unwrap_or(false) || viewport.fullscreen.unwrap_or(false)
    })
}

/// The edge under the mouse, if the app resizes the window itself.
fn resize_direction(ctx: &egui::Context, frame: WindowFrame) -> Option<ResizeDirection> {
    if frame != WindowFrame::Custom || maximized_or_fullscreen(ctx) {
        return None;
    }
    let pos = ctx.input(|i| i.pointer.hover_pos())?;
    edge(ctx.content_rect(), pos, RESIZE_EDGE)
}

/// The edge (or corner) of `window` within `width` of `pos`. The corners are wider, as in the
/// native windows: easier to grab.
fn edge(window: Rect, pos: Pos2, width: f32) -> Option<ResizeDirection> {
    if !window.contains(pos) {
        return None;
    }
    let corner = 3.0 * width;
    let near = |distance: f32, limit: f32| distance <= limit;
    let left = pos.x - window.min.x;
    let right = window.max.x - pos.x;
    let top = pos.y - window.min.y;
    let bottom = window.max.y - pos.y;
    let west =
        near(left, width) || (near(left, corner) && (near(top, width) || near(bottom, width)));
    let east =
        near(right, width) || (near(right, corner) && (near(top, width) || near(bottom, width)));
    let north =
        near(top, width) || (near(top, corner) && (near(left, width) || near(right, width)));
    let south =
        near(bottom, width) || (near(bottom, corner) && (near(left, width) || near(right, width)));
    match (north, south, west, east) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (true, _, _, true) => Some(ResizeDirection::NorthEast),
        (_, true, true, _) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, ..) => Some(ResizeDirection::North),
        (_, true, ..) => Some(ResizeDirection::South),
        (_, _, true, _) => Some(ResizeDirection::West),
        (_, _, _, true) => Some(ResizeDirection::East),
        _ => None,
    }
}

fn resize_cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> Option<ResizeDirection> {
        let window = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        edge(window, Pos2::new(x, y), 5.0)
    }

    #[test]
    fn edges_resize_in_their_direction() {
        assert_eq!(at(400.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 598.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(799.0, 300.0), Some(ResizeDirection::East));
    }

    #[test]
    fn corners_are_wider_than_the_edges() {
        assert_eq!(at(1.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(12.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(797.0, 12.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 590.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(790.0, 599.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn inside_and_outside_do_not_resize() {
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(10.0, 300.0), None);
        assert_eq!(at(-1.0, 300.0), None);
        assert_eq!(at(400.0, 601.0), None);
    }
}
