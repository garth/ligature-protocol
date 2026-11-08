//! The documentation site's mdBook preprocessor (LP-181).
//!
//! The pages under `docs/` link to files elsewhere in the repository (the JSON Schemas,
//! the licence files, the implementation's READMEs) and to parts of `docs/` the site
//! leaves out (the decisions, the milestone plans, the security audits), since the site
//! is for users and for presenting the protocol. A relative link to any of those is
//! rewritten to the file on GitHub before the page is rendered. Everything else is left
//! as it is, for the link check to judge.
//!
//! mdBook runs the preprocessor as `lp-book supports <renderer>`, which exits 0 for
//! every renderer, and then with `[context, book]` as JSON on stdin, expecting the book
//! back on stdout.

use std::io::{self, Read, Write};

use serde_json::Value;

/// The repository the links go to.
const REPO: &str = "https://github.com/garth/ligature-protocol";
/// The branch the links show.
const BRANCH: &str = "main";
/// Directories under `docs/` the site does not publish.
const UNPUBLISHED: [&str; 3] = ["adr/", "milestones/", "security-audit/"];

fn main() -> io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("supports") {
        // The rewrite is the same for every renderer.
        return Ok(());
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let (_context, mut book): (Value, Value) =
        serde_json::from_str(&input).map_err(io::Error::other)?;
    rewrite_book(&mut book);
    let output = serde_json::to_string(&book).map_err(io::Error::other)?;
    io::stdout().write_all(output.as_bytes())
}

/// Rewrites the links of every chapter of the book, in place. mdBook 0.5 lists the
/// chapters under `items`; 0.4 listed them under `sections`.
fn rewrite_book(book: &mut Value) {
    for key in ["items", "sections"] {
        if let Some(items) = book.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                rewrite_item(item);
            }
        }
    }
}

/// Rewrites one item of the book: a chapter and its sub-items. Separators and part
/// titles carry no links.
fn rewrite_item(item: &mut Value) {
    let Some(chapter) = item.get_mut("Chapter").and_then(Value::as_object_mut) else {
        return;
    };
    let path = chapter
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(path) = path {
        let page_dir = path.rfind('/').map_or("", |i| &path[..i]);
        if let Some(content) = chapter.get("content").and_then(Value::as_str) {
            let rewritten = rewrite_links(content, page_dir);
            chapter.insert("content".to_owned(), Value::String(rewritten));
        }
    }
    if let Some(sub_items) = chapter.get_mut("sub_items").and_then(Value::as_array_mut) {
        for sub in sub_items {
            rewrite_item(sub);
        }
    }
}

/// Rewrites every inline link (`](target)`, with an optional `#fragment`) in
/// `markdown`, a page in `page_dir` under `docs/` (`""` for `docs/` itself), whose
/// target leaves `docs/` or lands in a directory the site does not publish. A link
/// with a scheme, an absolute path, a title or a space is left alone.
#[must_use]
pub fn rewrite_links(markdown: &str, page_dir: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(open) = rest.find("](") {
        out.push_str(&rest[..open + 2]);
        rest = &rest[open + 2..];
        let Some(close) = rest.find(')') else {
            break;
        };
        let inside = &rest[..close];
        if inside.is_empty() || inside.contains(char::is_whitespace) {
            continue;
        }
        let (target, fragment) = match inside.find('#') {
            Some(hash) => (&inside[..hash], &inside[hash..]),
            None => (inside, ""),
        };
        if let Some(url) = github_url(target, page_dir) {
            out.push_str(&url);
            out.push_str(fragment);
            rest = &rest[close..];
        }
    }
    out.push_str(rest);
    out
}

/// The GitHub URL for a link `target` from a page in `page_dir`, or `None` when the
/// target stays within the site.
fn github_url(target: &str, page_dir: &str) -> Option<String> {
    if target.is_empty() || target.starts_with('/') || has_scheme(target) {
        return None;
    }
    let resolved = normalise(&format!("{page_dir}/{target}"));
    let outside = resolved == ".." || resolved.starts_with("../");
    // The directory itself counts, so a link to `adr/` is rewritten as `adr/x.md` is.
    let resolved_dir = format!("{resolved}/");
    let unpublished = UNPUBLISHED.iter().any(|d| resolved_dir.starts_with(d));
    if !outside && !unpublished {
        return None;
    }
    let repo_path = normalise(&format!("docs/{page_dir}/{target}"));
    let kind = if target.ends_with('/') {
        "tree"
    } else {
        "blob"
    };
    Some(format!("{REPO}/{kind}/{BRANCH}/{repo_path}"))
}

/// Whether a link target starts with a URL scheme (`https:`, `mailto:`).
fn has_scheme(target: &str) -> bool {
    let Some(colon) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon];
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}

/// A path with its `.` and `..` segments folded, as `posixpath.normpath` folds them:
/// a `..` that climbs above the start stays.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_to_an_adr_from_a_chapter_goes_to_github() {
        let page = "see [ADR 0043](../adr/0043-keys-carry-their-meaning.md) for keys";
        assert_eq!(
            rewrite_links(page, "spec"),
            "see [ADR 0043](https://github.com/garth/ligature-protocol/blob/main/docs/adr/0043-keys-carry-their-meaning.md) for keys"
        );
    }

    #[test]
    fn a_link_out_of_docs_goes_to_github_with_its_fragment() {
        let page =
            "the [schema](../../schemas/message.schema.json#gossip) and [licence](../../LICENSE)";
        assert_eq!(
            rewrite_links(page, "spec"),
            "the [schema](https://github.com/garth/ligature-protocol/blob/main/schemas/message.schema.json#gossip) \
             and [licence](https://github.com/garth/ligature-protocol/blob/main/LICENSE)"
        );
    }

    #[test]
    fn a_directory_link_goes_to_the_tree() {
        assert_eq!(
            rewrite_links("[audits](security-audit/)", ""),
            "[audits](https://github.com/garth/ligature-protocol/tree/main/docs/security-audit)"
        );
        assert_eq!(
            rewrite_links("[adrs](../adr/)", "guide"),
            "[adrs](https://github.com/garth/ligature-protocol/tree/main/docs/adr)"
        );
    }

    #[test]
    fn links_within_the_site_are_left_alone() {
        let page = "[install](guide/install.md), [3g](spec/05-capabilities.md#3g-input), \
                    ![logo](assets/sponsors/nordstack.svg), [home](https://example.org/x.md), \
                    [root](/ligature-protocol/), [titled](../adr/x.md \"a title\"), [empty]()";
        assert_eq!(rewrite_links(page, ""), page);
        let chapter = "[overview](00-overview.md) and [glossary](../glossary.md)";
        assert_eq!(rewrite_links(chapter, "spec"), chapter);
    }

    #[test]
    fn a_whole_chapter_is_rewritten_in_place_and_sub_items_too() {
        let mut book = serde_json::json!({
            "sections": [
                {"Chapter": {
                    "name": "Capabilities",
                    "path": "spec/05-capabilities.md",
                    "content": "[ADR 0008](../adr/0008-capabilities.md)",
                    "sub_items": [
                        {"Chapter": {"path": "guide/input.md", "content": "[plan](../milestones/07-shared-input.md)", "sub_items": []}}
                    ]
                }},
                "Separator",
                {"PartTitle": "Specification"},
                {"Chapter": {"name": "Draft", "path": null, "content": "[x](../adr/x.md)", "sub_items": []}}
            ]
        });
        rewrite_book(&mut book);
        let top = &book["sections"][0]["Chapter"];
        assert_eq!(
            top["content"],
            "[ADR 0008](https://github.com/garth/ligature-protocol/blob/main/docs/adr/0008-capabilities.md)"
        );
        assert_eq!(
            top["sub_items"][0]["Chapter"]["content"],
            "[plan](https://github.com/garth/ligature-protocol/blob/main/docs/milestones/07-shared-input.md)"
        );
        // A draft chapter has no path and is left as it is.
        assert_eq!(
            book["sections"][3]["Chapter"]["content"],
            "[x](../adr/x.md)"
        );
    }

    #[test]
    fn normalise_folds_dots_and_keeps_a_climb_above_the_start() {
        assert_eq!(normalise("spec/../adr/./x.md"), "adr/x.md");
        assert_eq!(normalise("spec/../../schemas/x.json"), "../schemas/x.json");
        assert_eq!(normalise("/guide//install.md"), "guide/install.md");
        assert_eq!(normalise("docs//LICENSE"), "docs/LICENSE");
    }
}
