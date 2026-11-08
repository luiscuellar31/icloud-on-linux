//! Text shown to the user. English is the source language; write every such text with `tr!` so it can be
//! translated, and keep the values in `{name}` placeholders.
use std::collections::HashMap;
use std::env;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::translation_es::SPANISH;

/// The languages the app can show: their codes and names.
pub const LANGUAGES: [(&str, &str); 2] = [("en", "English"), ("es", "Español")];

static SHOW_SPANISH: AtomicBool = AtomicBool::new(false);
static CATALOG: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| SPANISH.iter().copied().collect());
/// The desktop's languages, read at the first `apply`, so that the `LANGUAGE` the app then sets for GTK's own text
/// does not change what Automatic means.
static DESKTOP: LazyLock<Vec<String>> = LazyLock::new(desktop_languages);

/// Return the text for the current language.
pub fn tr(text: &str) -> String {
    translate(text, SHOW_SPANISH.load(Ordering::Relaxed)).to_owned()
}

fn translate(text: &str, spanish: bool) -> &str {
    if spanish {
        CATALOG.get(text).copied().unwrap_or(text)
    } else {
        text
    }
}

/// The desktop's preferred languages, most preferred first, as locale names such as `es_MX.UTF-8`.
pub fn desktop_languages() -> Vec<String> {
    let mut names: Vec<String> = env::var("LANGUAGE")
        .unwrap_or_default()
        .split(':')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    // The first of these that is set names the locale, as in gettext.
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|variable| env::var(variable).ok().filter(|value| !value.is_empty()));
    names.extend(locale);
    names
}

/// The language for a saved choice: `auto`, `en`, or `es`. `auto` uses the first desktop language the app has,
/// and English when it has none of them.
pub fn resolve(choice: &str, desktop: &[String]) -> &'static str {
    let known = |code: &str| LANGUAGES.iter().map(|(known, _)| *known).find(|known| *known == code);
    known(choice)
        .or_else(|| {
            desktop.iter().find_map(|name| {
                let code = name.split(['_', '-', '.', '@']).next().unwrap_or_default();
                known(&code.to_lowercase())
            })
        })
        .unwrap_or("en")
}

/// The language that text is shown in now.
pub fn current() -> &'static str {
    if SHOW_SPANISH.load(Ordering::Relaxed) {
        "es"
    } else {
        "en"
    }
}

/// Show later text in the language for `choice`, and return that language.
pub fn apply(choice: &str) -> &'static str {
    let language = resolve(choice, &DESKTOP);
    SHOW_SPANISH.store(language == "es", Ordering::Relaxed);
    language
}

/// Replace each `{name}` placeholder with its value in one pass, so a value is never read as a placeholder.
pub fn format(template: String, values: &[(&str, String)]) -> String {
    let mut text = String::with_capacity(template.len());
    let mut rest = template.as_str();
    while let Some(start) = rest.find('{') {
        text.push_str(&rest[..start]);
        let tail = &rest[start..];
        let filled = tail.find('}').and_then(|end| {
            let (_, value) = values.iter().find(|(name, _)| *name == &tail[1..end])?;
            Some((value, end))
        });
        if let Some((value, end)) = filled {
            text.push_str(value);
            rest = &tail[end + 1..];
        } else {
            text.push('{');
            rest = &tail[1..];
        }
    }
    text.push_str(rest);
    text
}

/// `tr!("Text")` translates a literal; `tr!("{count} items", count = total)` also fills its placeholders.
#[macro_export]
macro_rules! tr {
    ($text:literal) => {
        $crate::i18n::tr($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::format($crate::i18n::tr($text), &[$((stringify!($name), ($value).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_text_comes_from_the_catalog_and_falls_back_to_english() {
        assert_eq!(translate("Connect", true), "Conectar");
        assert_eq!(translate("Connect", false), "Connect");
        assert_eq!(translate("Not in the catalog", true), "Not in the catalog");
    }

    #[test]
    fn automatic_choice_follows_the_desktop_languages() {
        let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        for (desktop, expected) in [
            (names(&["es-MX", "en-US"]), "es"),
            (names(&["fr-FR", "en-GB"]), "en"),
            (names(&["de_DE"]), "en"),
            (names(&["pt-BR", "es_419"]), "es"),
            (names(&["C.UTF-8", "es_MX.UTF-8@euro"]), "es"),
            (Vec::new(), "en"),
        ] {
            assert_eq!(resolve("auto", &desktop), expected, "{desktop:?}");
        }
        assert_eq!(resolve("en", &names(&["es_MX.UTF-8"])), "en");
        assert_eq!(resolve("es", &[]), "es");
    }

    #[test]
    fn placeholders_are_filled_by_name() {
        assert_eq!(tr!("{count} of {total} items", count = 2, total = 5), "2 of 5 items");
        assert_eq!(tr!("Plain text"), "Plain text");
        assert_eq!(tr!("{name} and {other}", name = "{other}", other = 1), "{other} and 1");
        assert_eq!(tr!("{unknown} {", other = 1), "{unknown} {");
    }
}
