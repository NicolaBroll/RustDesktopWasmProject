//! L0 — Traduzioni della UI con [Fluent](https://projectfluent.org/): italiano e inglese.
//!
//! I testi stanno in `locales/<lingua>/<area>.ftl`, un file per area della UI; le chiavi sono
//! `<area>-<cosa>` in kebab-case. Nel codice si usa sempre la macro [`tr!`] con la chiave
//! letterale (un test della UI controlla che ogni chiave usata esista):
//!
//! ```
//! use project_i18n::tr;
//! let text = tr!("merge-conflict-files", n = 3);
//! ```
//!
//! La lingua è una sola per tutta l'app ([`set_lang`]): cambia subito, senza riavvio; i testi
//! già composti (un messaggio nella barra di stato) restano nella lingua di quando sono nati.
//! Guida: `impl-docs/i18n.md`.

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod areas {
    include!(concat!(env!("OUT_DIR"), "/areas.rs"));
}

/// Una lingua della UI.
/// L'app è in inglese; l'italiano resta per i test della UI, che confrontano i testi italiani
/// di sempre, e non si sceglie dall'app.
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

    /// I file `.ftl` della lingua: (area, contenuto).
    fn sources(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Lang::It => areas::IT,
            Lang::En => areas::EN,
        }
    }
}

/// Un parametro di un testo: stringa o numero (i numeri scelgono la forma plurale).
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

/// I testi di una lingua.
pub struct I18n {
    lang: Lang,
    bundle: FluentBundle<FluentResource>,
}

impl I18n {
    /// Carica tutte le aree della lingua. Un file che non si analizza o una chiave ripetuta
    /// fermano le build di debug (e i test); in release si usa quello che si è letto.
    pub fn new(lang: Lang) -> Self {
        let id = lang.code().parse().expect("codice di lingua valido");
        let mut bundle = FluentBundle::new_concurrent(vec![id]);
        // Senza i caratteri di isolamento Unicode attorno ai parametri: egui li mostrerebbe.
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

    /// Vero se la chiave esiste.
    pub fn has(&self, key: &str) -> bool {
        self.bundle.has_message(key)
    }

    /// Il testo della chiave, senza parametri.
    pub fn tr(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Il testo della chiave con i parametri (`$nome` nel file `.ftl`).
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

    /// Una chiave mancante (o un parametro mancante) ferma le build di debug; in release si
    /// mostra la chiave.
    fn format(&self, key: &str, args: Option<&FluentArgs>) -> String {
        let Some(pattern) = self.bundle.get_message(key).and_then(|m| m.value()) else {
            debug_assert!(false, "chiave mancante in {}: {key}", self.lang.code());
            return key.to_owned();
        };
        let mut errors = Vec::new();
        let text = self.bundle.format_pattern(pattern, args, &mut errors);
        debug_assert!(errors.is_empty(), "{}: {key}: {errors:?}", self.lang.code());
        text.into_owned()
    }
}

/// La lingua attiva, come indice di [`Lang`]. All'avvio l'inglese (l'italiano con la feature
/// `test-italiano`, solo per i test della UI).
static CURRENT: AtomicU8 = AtomicU8::new(if cfg!(feature = "test-italiano") {
    0
} else {
    1
});

/// Cambia la lingua di tutta l'app, subito.
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

/// I testi di `lang`, caricati alla prima richiesta.
pub fn get(lang: Lang) -> &'static I18n {
    static ALL: OnceLock<[I18n; 2]> = OnceLock::new();
    &ALL.get_or_init(|| Lang::ALL.map(I18n::new))[lang.index()]
}

/// I testi della lingua attiva.
pub fn current() -> &'static I18n {
    get(lang())
}

/// Il testo di una chiave nella lingua attiva: `tr!("merge-continue")`, con parametri
/// `tr!("merge-files", n = 3, name = "main")`. La chiave è sempre un letterale.
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
