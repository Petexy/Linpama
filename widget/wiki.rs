//! The Arch Wiki article shown under a package: the original's
//! `_fetch_wiki_description` and its style sheet.

use crate::http::{self, quote};
use serde_json::Value;

/// The original `_WIKI_CSS`, byte for byte.
pub const WIKI_CSS: &str = include_str!("wiki.css");

/// An article: its HTML body and its address on the wiki.
#[derive(Debug, PartialEq)]
pub struct Article {
    pub html: String,
    pub url: String,
}

/// The original `_fetch_wiki_description`: the page named like the package
/// (as is, Title-Cased-With-Dashes, Title Cased), else the wiki's first
/// search hit for ever shorter dash-separated prefixes of the name. Blocking.
pub fn fetch(package: &str) -> Option<Article> {
    let mut candidates = vec![package.to_owned()];
    let spaced = title_case(&package.replace('-', " "));
    let titled = spaced.replace(' ', "-");
    if !candidates.contains(&titled) {
        candidates.push(titled);
    }
    if !candidates.contains(&spaced) {
        candidates.push(spaced);
    }
    for title in &candidates {
        if let Some(article) = parse(title) {
            return Some(article);
        }
    }
    // nvidia-open → nvidia-open, then nvidia, so that it finds "NVIDIA".
    let parts: Vec<&str> = package.split('-').collect();
    let mut seen = Vec::new();
    for length in (1..=parts.len()).rev() {
        let term = parts[..length].join("-");
        if seen.contains(&term) {
            continue;
        }
        seen.push(term.clone());
        let search_url = format!(
            "https://wiki.archlinux.org/api.php?action=opensearch&format=json&redirects=resolve&limit=1&search={}",
            quote(&term)
        );
        let Some(results) = json(&search_url) else {
            continue;
        };
        // [query, [titles], [descriptions], [urls]]
        let Some(top) = results
            .get(1)
            .and_then(Value::as_array)
            .and_then(|titles| titles.first())
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(article) = parse(top) {
            return Some(article);
        }
    }
    None
}

/// The `action=parse` request for one title.
fn parse(title: &str) -> Option<Article> {
    let api_url = format!(
        "https://wiki.archlinux.org/api.php?action=parse&prop=text&format=json&redirects=1&page={}",
        quote(title)
    );
    let data = json(&api_url)?;
    if data.get("error").is_some() {
        return None;
    }
    let parse = data.get("parse");
    let html = parse
        .and_then(|parse| parse.get("text"))
        .and_then(|text| text.get("*"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if html.is_empty() {
        return None;
    }
    let page_title = parse
        .and_then(|parse| parse.get("title"))
        .and_then(Value::as_str)
        .unwrap_or(title);
    Some(Article {
        html: html.to_owned(),
        url: format!("https://wiki.archlinux.org/title/{}", quote(page_title)),
    })
}

fn json(url: &str) -> Option<Value> {
    let body = http::get(url, 10).ok()?;
    serde_json::from_str(std::str::from_utf8(&body).ok()?).ok()
}

/// The page the original's web view was given.
pub fn page(html: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset='utf-8'><style>{WIKI_CSS}</style></head><body>{html}</body></html>"
    )
}

/// `str.title()`: a letter after a letter is lowered, any other letter
/// raised (digits and punctuation start a new word).
pub fn title_case(text: &str) -> String {
    let mut titled = String::with_capacity(text.len());
    let mut previous_cased = false;
    for c in text.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if previous_cased {
                titled.extend(c.to_lowercase());
            } else {
                titled.extend(c.to_uppercase());
            }
        } else {
            titled.push(c);
        }
        previous_cased = cased;
    }
    titled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cased_like_python() {
        assert_eq!(title_case("nvidia open"), "Nvidia Open");
        assert_eq!(title_case("x11 foo2bar"), "X11 Foo2Bar");
        assert_eq!(title_case("NVIDIA"), "Nvidia");
        assert_eq!(title_case("lib32 gcc-libs"), "Lib32 Gcc-Libs");
    }

    #[test]
    fn the_page_wraps_the_article() {
        let page = page("<p>x</p>");
        assert!(page.starts_with(
            "<!DOCTYPE html><html><head><meta charset='utf-8'><style>\n        html, body {"
        ));
        assert!(page.ends_with("</style></head><body><p>x</p></body></html>"));
    }
}
