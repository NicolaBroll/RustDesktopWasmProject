use super::*;
use fluent_syntax::ast::Entry;
use std::collections::BTreeSet;

/// The keys (messages) of an `.ftl` file; the file must parse without errors.
fn keys(lang: Lang, area: &str, source: &str) -> BTreeSet<String> {
    let resource = FluentResource::try_new(source.to_owned())
        .unwrap_or_else(|(_, e)| panic!("{}/{area}.ftl does not parse: {e:?}", lang.code()));
    resource
        .entries()
        .filter_map(|entry| match entry {
            Entry::Message(m) => Some(m.id.name.to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn both_languages_have_the_same_areas_and_keys() {
    let areas = |lang: Lang| -> Vec<&str> { lang.sources().iter().map(|(a, _)| *a).collect() };
    assert_eq!(areas(Lang::It), areas(Lang::En), "stessi file in it/ e en/");
    for ((area, it), (_, en)) in Lang::It.sources().iter().zip(Lang::En.sources()) {
        let it = keys(Lang::It, area, it);
        let en = keys(Lang::En, area, en);
        let only_it: Vec<_> = it.difference(&en).collect();
        let only_en: Vec<_> = en.difference(&it).collect();
        assert!(
            only_it.is_empty() && only_en.is_empty(),
            "{area}.ftl: solo in it {only_it:?}, solo in en {only_en:?}"
        );
    }
}

#[test]
fn keys_start_with_their_area_and_load_without_errors() {
    for lang in Lang::ALL {
        for (area, source) in lang.sources() {
            for key in keys(lang, area, source) {
                assert!(
                    key.starts_with(&format!("{area}-")),
                    "{}/{area}.ftl: {key} does not start with «{area}-»",
                    lang.code()
                );
            }
        }
        // `new` stops the tests if a file does not parse or a key is repeated.
        let i18n = I18n::new(lang);
        assert_eq!(i18n.lang(), lang);
    }
}

#[test]
fn plurals_follow_the_language() {
    let it = get(Lang::It);
    let en = get(Lang::En);
    let files = |i18n: &I18n, n: usize| i18n.tr_args("merge-conflict-files", [("n", Arg::from(n))]);
    assert_eq!(files(it, 1), "1 file in conflitto");
    assert_eq!(files(it, 3), "3 file in conflitto");
    assert_eq!(files(en, 1), "1 conflicted file");
    assert_eq!(files(en, 3), "3 conflicted files");
}

#[test]
fn parameters_have_no_unicode_isolation_marks() {
    let text = get(Lang::En).tr_args("merge-continue-button", [("name", Arg::from("merge"))]);
    assert_eq!(text, "Continue merge…");
    assert!(!text.contains(['\u{2068}', '\u{2069}']));
}

#[test]
fn the_macro_uses_the_current_language() {
    // The other tests use `get`: only this one touches the global language.
    assert_eq!(lang(), Lang::En);
    let expected = "2 conflicted files";
    assert_eq!(tr!("merge-conflict-files", n = 2), expected);
    assert!(current().has("merge-conflict-files"));
    assert!(!current().has("merge-nope"));
}

#[test]
fn an_optional_count_can_disappear() {
    let notice = |n: usize| {
        get(Lang::En).tr_args(
            "merge-aborted-notice",
            [
                ("title", Arg::from("Merge")),
                ("backup", Arg::from("B")),
                ("n", Arg::from(n)),
            ],
        )
    };
    assert!(notice(0).starts_with("Merge aborted: index and working tree saved first in B; "));
    assert!(notice(1).contains("in B (1 file); "));
    assert!(notice(2).contains("in B (2 files); "));
    let it = get(Lang::It).tr_args(
        "merge-aborted-notice",
        [
            ("title", Arg::from("Merge")),
            ("backup", Arg::from("B")),
            ("n", Arg::from(3)),
        ],
    );
    // The Italian text is a different translation of the same message, with the same arguments.
    assert_ne!(
        it,
        get(Lang::En).tr_args(
            "merge-aborted-notice",
            [
                ("title", Arg::from("Merge")),
                ("backup", Arg::from("B")),
                ("n", Arg::from(3)),
            ],
        )
    );
    assert!(it.contains('B') && it.contains('3'));
}
