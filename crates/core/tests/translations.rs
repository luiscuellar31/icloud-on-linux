//! Every text passed to `tr!` in the app has a Spanish translation with the same placeholders, and the catalog
//! has no unused text.
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use icloud_core::translation_es::SPANISH;
use proc_macro2::{Delimiter, TokenStream, TokenTree};

fn sources(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Collect the text of each `tr!` call. Reading tokens skips comments and documentation, and modules marked
/// `#[cfg(test)]` are skipped because their text is never shown.
fn collect(tokens: TokenStream, texts: &mut Vec<String>) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut skip_module = false;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Punct(hash) if hash.as_char() == '#' => {
                if let Some(TokenTree::Group(attribute)) = tokens.get(index + 1) {
                    skip_module |= attribute.stream().to_string() == "cfg (test)";
                }
            }
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace && skip_module => skip_module = false,
            TokenTree::Group(group) => collect(group.stream(), texts),
            TokenTree::Ident(name) if name == "tr" => {
                if let (Some(TokenTree::Punct(bang)), Some(TokenTree::Group(arguments))) =
                    (tokens.get(index + 1), tokens.get(index + 2))
                {
                    if bang.as_char() == '!' {
                        let first = arguments.stream().into_iter().next().expect("tr! needs a text");
                        let literal: syn::LitStr = syn::parse2(first.into()).expect("pass tr! a string literal");
                        texts.push(literal.value());
                    }
                }
            }
            _ => {}
        }
    }
}

fn source_texts() -> BTreeSet<String> {
    let mut files = Vec::new();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for entry in fs::read_dir(&crates).unwrap() {
        let source = entry.unwrap().path().join("src");
        if source.is_dir() {
            sources(&source, &mut files);
        }
    }
    let mut texts = Vec::new();
    for file in files {
        let tokens: TokenStream = fs::read_to_string(&file).unwrap().parse().unwrap();
        collect(tokens, &mut texts);
    }
    texts.into_iter().collect()
}

fn placeholders(text: &str) -> BTreeSet<&str> {
    text.split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect()
}

#[test]
fn every_text_is_translated_into_spanish() {
    let catalog: HashSet<&str> = SPANISH.iter().map(|(source, _)| *source).collect();
    let missing: Vec<String> = source_texts()
        .into_iter()
        .filter(|text| !catalog.contains(text.as_str()))
        .collect();
    assert!(missing.is_empty(), "add Spanish text for: {missing:#?}");
}

#[test]
fn the_catalog_has_no_unused_or_repeated_text() {
    let texts = source_texts();
    let unused: Vec<&str> = SPANISH
        .iter()
        .map(|(source, _)| *source)
        .filter(|source| !texts.contains(*source))
        .collect();
    assert!(unused.is_empty(), "remove unused text: {unused:#?}");
    let unique: HashSet<&str> = SPANISH.iter().map(|(source, _)| *source).collect();
    assert_eq!(unique.len(), SPANISH.len(), "a text appears twice in the catalog");
}

#[test]
fn translations_keep_the_same_values() {
    for (source, translation) in SPANISH {
        assert_eq!(placeholders(source), placeholders(translation), "{source}");
    }
}
