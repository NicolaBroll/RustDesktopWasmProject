//! The toolbar icons: a few glyphs of Phosphor Regular (MIT) in a font of their own, rebuilt by
//! `scripts/icon-font.sh`. A new icon needs its codepoint here and in the script.

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
pub(crate) const FETCH: &str = "\u{E1AC}";
pub(crate) const STOP: &str = "\u{E46E}";

/// Adds the font, under its own family: the text keeps its fonts.
pub(crate) fn install(ctx: &egui::Context) {
    ctx.add_font(egui::epaint::text::FontInsert::new(
        FAMILY,
        egui::FontData::from_static(FONT),
        vec![egui::epaint::text::InsertFontFamily {
            family: family(),
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    ));
}

/// The font of the icons, `size` points tall.
pub(crate) fn font(size: f32) -> egui::FontId {
    egui::FontId::new(size, family())
}

fn family() -> egui::FontFamily {
    egui::FontFamily::Name(FAMILY.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_in_the_font() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for icon in [
            STASH, REDO, UNDO, PULL, PUSH, REFRESH, CARET_DOWN, FETCH, STOP,
        ] {
            let c = icon.chars().next().unwrap();
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&font(16.0), c)),
                "U+{:04X} missing",
                u32::from(c)
            );
        }
    }
}
