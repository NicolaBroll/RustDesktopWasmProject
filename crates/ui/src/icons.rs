//! The UI icons: a few glyphs of Phosphor Regular (MIT) in a font of their own, rebuilt by
//! `scripts/icon-font.py`. A new icon needs its codepoint here and in the script.
//!
//! The font is also the last fallback of the text fonts: it maps ↑ ↓ ⚠, which the default fonts
//! lack, onto its arrows and warning, so they render in any label.

/// The font, with only the glyphs below.
const FONT: &[u8] = include_bytes!("../assets/icons.ttf");

/// Name of the font family of the icons.
const FAMILY: &str = "icons";

pub(crate) const STASH: &str = "\u{E00E}";
pub(crate) const REDO: &str = "\u{E036}";
pub(crate) const UNDO: &str = "\u{E038}";
pub(crate) const PULL: &str = "\u{E05C}";
pub(crate) const PUSH: &str = "\u{E066}";
pub(crate) const REFRESH: &str = "\u{E094}";
pub(crate) const CARET_DOWN: &str = "\u{E136}";
pub(crate) const CARET_RIGHT: &str = "\u{E13A}";
pub(crate) const CHECK: &str = "\u{E182}";
pub(crate) const CLOUD: &str = "\u{E1AA}";
pub(crate) const FETCH: &str = "\u{E1AC}";
pub(crate) const COPY: &str = "\u{E1CA}";
pub(crate) const MORE: &str = "\u{E1FE}";
pub(crate) const FOLDER: &str = "\u{E24A}";
pub(crate) const BRANCH: &str = "\u{E278}";
pub(crate) const SEARCH: &str = "\u{E30C}";
pub(crate) const PLUS: &str = "\u{E3D4}";
pub(crate) const STOP: &str = "\u{E46E}";
pub(crate) const TAG: &str = "\u{E478}";
pub(crate) const CLOSE: &str = "\u{E4F6}";
pub(crate) const SPLIT: &str = "\u{E546}";
pub(crate) const UNIFIED: &str = "\u{E5A2}";

/// Every icon, for the test that checks the font has them.
#[cfg(test)]
const ALL: &[&str] = &[
    STASH,
    REDO,
    UNDO,
    PULL,
    PUSH,
    REFRESH,
    CARET_DOWN,
    CARET_RIGHT,
    CHECK,
    CLOUD,
    FETCH,
    COPY,
    MORE,
    FOLDER,
    BRANCH,
    SEARCH,
    PLUS,
    STOP,
    TAG,
    CLOSE,
    SPLIT,
    UNIFIED,
];

/// Adds the font under its own family, and as the last fallback of the text: the text keeps its
/// fonts, only the symbols they lack come from here.
pub(crate) fn install(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let family = |family, priority| InsertFontFamily { family, priority };
    ctx.add_font(FontInsert::new(
        FAMILY,
        egui::FontData::from_static(FONT),
        vec![
            family(self::family(), FontPriority::Highest),
            family(egui::FontFamily::Proportional, FontPriority::Lowest),
            family(egui::FontFamily::Monospace, FontPriority::Lowest),
        ],
    ));
}

/// The font of the icons, `size` points tall.
pub(crate) fn font(size: f32) -> egui::FontId {
    egui::FontId::new(size, family())
}

/// `icon` as text for a label or a button.
pub(crate) fn text(icon: &str, size: f32) -> egui::RichText {
    egui::RichText::new(icon).font(font(size))
}

fn family() -> egui::FontFamily {
    egui::FontFamily::Name(FAMILY.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_fonts(check: impl FnOnce(&egui::Context)) {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        check(&ctx);
    }

    #[test]
    fn every_icon_is_in_the_font() {
        with_fonts(|ctx| {
            for icon in ALL {
                let c = icon.chars().next().unwrap();
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&font(16.0), c)),
                    "U+{:04X} missing",
                    u32::from(c)
                );
            }
        });
    }

    #[test]
    fn the_text_has_the_arrows_and_the_warning() {
        with_fonts(|ctx| {
            for c in ['↑', '↓', '⚠'] {
                for text in [
                    egui::FontId::proportional(13.0),
                    egui::FontId::monospace(13.0),
                ] {
                    assert!(ctx.fonts_mut(|f| f.has_glyph(&text, c)), "{c} missing");
                }
            }
        });
    }
}
