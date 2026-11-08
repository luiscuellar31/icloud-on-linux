//! Text shown to the user. English is the source language; write every such text with `tr!` so it can be
//! translated, and keep the values in `{name}` placeholders.

/// Return the text for the current language.
pub fn tr(text: &str) -> String {
    text.to_owned()
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
    #[test]
    fn placeholders_are_filled_by_name() {
        assert_eq!(tr!("{count} of {total} items", count = 2, total = 5), "2 of 5 items");
        assert_eq!(tr!("Plain text"), "Plain text");
        assert_eq!(tr!("{name} and {other}", name = "{other}", other = 1), "{other} and 1");
        assert_eq!(tr!("{unknown} {", other = 1), "{unknown} {");
    }
}
