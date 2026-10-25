//! L0 — UI translations with [Fluent](https://projectfluent.org/): Italian and English.
//!
//! The texts live in `locales/<language>/<area>.ftl`, one file per UI area; keys are
//! `<area>-<what>` in kebab-case. In code the [`tr!`] macro is always used with a literal key
//! (a UI test checks that every key used exists):
//!
//! ```
//! use project_i18n::tr;
//! let text = tr!("merge-conflict-files", n = 3);
//! ```
//!
//! There is a single language for the whole app ([`set_lang`]): it changes at once, without a
//! restart; texts already composed (a status bar message) stay in the language they were
//! created in.
//! Guide: `impl-docs/i18n.md`.

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod areas {
    include!(concat!(env!("OUT_DIR"), "/areas.rs"));
}

/// A UI language.
/// English by default; the app's "Settings…" window chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    It,
    #[default]
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::It, Lang::En];

    /// Il codice ISO 639-1: `it`, `en`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::It => "it",
            Lang::En => "en",
        }
    }

    fn index(self) -> usize {
        match self {
            Lang::It => 0,
            Lang::En => 1,
        }
    }

    /// The language's `.ftl` files: (area, content).
    fn sources(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Lang::It => areas::IT,
            Lang::En => areas::EN,
        }
    }
}

/// A parameter of a text: string or number (numbers choose the plural form).
pub struct Arg<'a>(FluentValue<'a>);

impl<'a> From<&'a str> for Arg<'a> {
    fn from(value: &'a str) -> Self {
        Arg(FluentValue::from(value))
    }
}

impl<'a> From<&'a String> for Arg<'a> {
    fn from(value: &'a String) -> Self {
        Arg(FluentValue::from(value.as_str()))
    }
}

impl From<String> for Arg<'_> {
    fn from(value: String) -> Self {
        Arg(FluentValue::from(value))
    }
}

macro_rules! number_arg {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg<'_> {
            fn from(value: $t) -> Self {
                Arg(FluentValue::from(value))
            }
        }
    )*};
}
number_arg!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f64);

/// The texts of a language.
pub struct I18n {
    lang: Lang,
    bundle: FluentBundle<FluentResource>,
}

impl I18n {
    /// Loads all the areas of the language. A file that does not parse or a repeated key
    /// stops debug builds (and tests); in release what was read is used.
    pub fn new(lang: Lang) -> Self {
        let id = lang.code().parse().expect("valid language code");
        let mut bundle = FluentBundle::new_concurrent(vec![id]);
        // Without the Unicode isolation characters around parameters: egui would show them.
        bundle.set_use_isolating(false);
        for (area, source) in lang.sources() {
            let resource = match FluentResource::try_new((*source).to_owned()) {
                Ok(resource) => resource,
                Err((resource, errors)) => {
                    debug_assert!(false, "{}/{area}.ftl: {errors:?}", lang.code());
                    resource
                }
            };
            if let Err(errors) = bundle.add_resource(resource) {
                debug_assert!(false, "{}/{area}.ftl: {errors:?}", lang.code());
            }
        }
        I18n { lang, bundle }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// True if the key exists.
    pub fn has(&self, key: &str) -> bool {
        self.bundle.has_message(key)
    }

    /// The text of the key, without parameters.
    pub fn tr(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// The text of the key with parameters (`$name` in the `.ftl` file).
    pub fn tr_args<'a>(
        &self,
        key: &str,
        args: impl IntoIterator<Item = (&'a str, Arg<'a>)>,
    ) -> String {
        let mut fluent = FluentArgs::new();
        for (name, Arg(value)) in args {
            fluent.set(name, value);
        }
        self.format(key, Some(&fluent))
    }

    /// A missing key (or a missing parameter) stops debug builds; in release the key is
    /// shown.
    fn format(&self, key: &str, args: Option<&FluentArgs>) -> String {
        let Some(pattern) = self.bundle.get_message(key).and_then(|m| m.value()) else {
            debug_assert!(false, "missing key in {}: {key}", self.lang.code());
            return key.to_owned();
        };
        let mut errors = Vec::new();
        let text = self.bundle.format_pattern(pattern, args, &mut errors);
        debug_assert!(errors.is_empty(), "{}: {key}: {errors:?}", self.lang.code());
        text.into_owned()
    }
}

/// The active language, as an index of [`Lang`]. English at startup.
static CURRENT: AtomicU8 = AtomicU8::new(1);

/// Changes the language of the whole app, at once.
pub fn set_lang(lang: Lang) {
    CURRENT.store(lang.index() as u8, Ordering::Relaxed);
}

/// La lingua attiva.
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::It,
        _ => Lang::En,
    }
}

/// The texts of `lang`, loaded on first request.
pub fn get(lang: Lang) -> &'static I18n {
    static ALL: OnceLock<[I18n; 2]> = OnceLock::new();
    &ALL.get_or_init(|| Lang::ALL.map(I18n::new))[lang.index()]
}

/// The texts of the active language.
pub fn current() -> &'static I18n {
    get(lang())
}

/// The text of a key in the active language: `tr!("merge-continue")`, with parameters
/// `tr!("merge-files", n = 3, name = "main")`. The key is always a literal.
#[macro_export]
macro_rules! tr {
    ($key:literal) => {
        $crate::current().tr($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::current().tr_args($key, [$((stringify!($name), $crate::Arg::from($value))),+])
    };
}

#[cfg(test)]
mod tests;
