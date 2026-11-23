//! Draft-sketch parity: how far the advisory `rune_kit::Markdown` sketch
//! drifts from the body Boris actually emits. Compares visible text only —
//! Boris's `<article class="site-content">` vs the sketch element tree —
//! and reports what each side shows that the other doesn't. Divergence is
//! expected (the sketch resolves no wikilinks, includes or components);
//! the report exists so the sketch is never mistaken for a preview.

use rune::{Cx, El, Env, Runtime};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Construct {
    pub name: &'static str,
    pub in_source: bool,
    /// The sketch shows the raw source syntax (`[[…]]`, `{{include …}}`).
    pub raw_in_sketch: bool,
    /// Words of Boris's emitted element (`<aside>`, `<details>`) the
    /// sketch never shows (admonition labels, summaries).
    pub boris_only: Vec<String>,
    /// Either of the above: the sketch is not what Boris will publish.
    pub diverges: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PageParity {
    pub page: String,
    /// Words only Boris's body shows (resolved titles, included text, labels).
    pub missing_in_sketch: Vec<String>,
    /// Words only the sketch shows (unresolved syntax).
    pub extra_in_sketch: Vec<String>,
    /// Share of Boris body words the sketch also shows, in percent.
    pub word_overlap_pct: u32,
    pub constructs: Vec<Construct>,
}

#[allow(clippy::ptr_arg)] // rune view signature: `fn(&S, &mut Cx<S>)` with `S = String`
fn sketch_view(src: &String, _cx: &mut Cx<String>) -> El {
    rune_kit::Markdown::new(src.clone()).build(&rune_kit::Kit::neutral(true))
}

/// Visible text runs of the sketch for `body` (headless).
pub fn sketch_texts(body: &str) -> Vec<String> {
    let mut rt = Runtime::new(
        body.to_string(),
        sketch_view,
        (900.0, 6000.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    rt.elements().into_iter().filter_map(|e| e.text).collect()
}

/// Visible text of Boris's emitted page body (`<article class="site-content">`).
pub fn boris_body_text(html: &str) -> String {
    strip_tags(boris_body_html(html))
}

fn boris_body_html(html: &str) -> &str {
    let open = "<article class=\"site-content\">";
    let start = html.find(open).map(|i| i + open.len()).unwrap_or(0);
    let end = html[start..]
        .find("</article>")
        .map(|i| start + i)
        .unwrap_or(html.len());
    &html[start..end]
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Visible text of Boris's first `<tag …>…</tag>` element in the body.
fn element_text(html: &str, tag: &str) -> String {
    let body = boris_body_html(html);
    let Some(start) = body.find(&format!("<{tag}")) else {
        return String::new();
    };
    let close = format!("</{tag}>");
    let end = body[start..]
        .find(&close)
        .map(|i| start + i + close.len())
        .unwrap_or(body.len());
    strip_tags(&body[start..end])
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        // rune_kit wraps inline runs in bidi isolates (U+2066–2069).
        .map(|w| {
            w.chars()
                .filter(|c| !('\u{2066}'..='\u{2069}').contains(c))
                .collect::<String>()
        })
        .map(|w| {
            w.trim_matches(|c: char| ",.;:!?()—•".contains(c))
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Compare one page's Markdown body (frontmatter stripped) with Boris's HTML.
pub fn compare(page: &str, body: &str, html: &str) -> PageParity {
    let sketch: Vec<String> = words(&sketch_texts(body).join(" "));
    let boris = words(&boris_body_text(html));
    let s: BTreeSet<&String> = sketch.iter().collect();
    let b: BTreeSet<&String> = boris.iter().collect();
    let shared = boris.iter().filter(|w| s.contains(w)).count();
    let pct = if boris.is_empty() {
        100
    } else {
        (shared * 100 / boris.len()) as u32
    };
    let joined = sketch.join(" ");
    let construct = |name, marker: &str, tag: Option<&str>| {
        let in_source = body.contains(marker);
        let raw_in_sketch = in_source && joined.contains(marker);
        let boris_only: Vec<String> = tag
            .filter(|_| in_source)
            .map(|t| {
                words(&element_text(html, t))
                    .into_iter()
                    .filter(|w| !s.contains(w))
                    .collect()
            })
            .unwrap_or_default();
        Construct {
            name,
            in_source,
            raw_in_sketch,
            diverges: raw_in_sketch || !boris_only.is_empty(),
            boris_only,
        }
    };
    PageParity {
        page: page.to_string(),
        missing_in_sketch: b.difference(&s).map(|w| w.to_string()).collect(),
        extra_in_sketch: s.difference(&b).map(|w| w.to_string()).collect(),
        word_overlap_pct: pct,
        constructs: vec![
            construct("wikilink", "[[", None),
            construct("include", "{{include", None),
            construct("aside", "<Aside", Some("aside")),
            construct("details", "<Details", Some("details")),
        ],
    }
}
