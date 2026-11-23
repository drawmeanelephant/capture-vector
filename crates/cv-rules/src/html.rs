//! Minimal, allocation-light HTML tokenizer + CSS-subset selector matcher
//! over *final* HTML. It is not a browser: it builds an element tree from
//! start/end tags (void and raw-text elements handled), which is enough to
//! answer head-level and structural questions deterministically.
//!
//! Selector subset: `tag` / `*`, `[a]`, `[a=v]`, `[a~=v]` (token match),
//! `[a^=v]` — value matches are ASCII case-insensitive, as HTML treats the
//! enumerated values these rules inspect (`rel`, `name`, `type`) —
//! `:not(<simple>)`, `:invalid-json` (raw text that fails to parse as
//! JSON), `:robots-noindex` (a robots meta saying `noindex` or `none`), combined with
//! descendant (` `), child (`>`) and adjacent-sibling (`+`) combinators.

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub parent: Option<usize>,
    pub prev_sibling: Option<usize>,
    /// Raw text for raw-text elements (`script`, `style`, `title`, `textarea`).
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HtmlDoc {
    pub elements: Vec<Element>,
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];
const RAW_TEXT: &[&str] = &["script", "style", "title", "textarea"];

fn find_ci(hay: &str, from: usize, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    (from..h.len().saturating_sub(n.len() - 1)).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

impl HtmlDoc {
    pub fn parse(src: &str) -> Self {
        let b = src.as_bytes();
        let mut elements: Vec<Element> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        // last child per element; index `elements.len()` slot is unused, root tracked separately.
        let mut last_child: Vec<Option<usize>> = Vec::new();
        let mut root_last: Option<usize> = None;
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'<' {
                i += 1;
                continue;
            }
            let rest = &src[i..];
            if rest.starts_with("<!--") {
                i = src[i + 4..].find("-->").map_or(b.len(), |p| i + 4 + p + 3);
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                i = src[i..].find('>').map_or(b.len(), |p| i + p + 1);
                continue;
            }
            if rest.starts_with("</") {
                let end = src[i..].find('>').map_or(b.len(), |p| i + p);
                let name = src[i + 2..end].trim().to_ascii_lowercase();
                if let Some(pos) = stack.iter().rposition(|&e| elements[e].name == name) {
                    stack.truncate(pos);
                }
                i = (end + 1).min(b.len());
                continue;
            }
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b':') {
                j += 1;
            }
            if j == i + 1 {
                i += 1;
                continue;
            }
            let name = src[i + 1..j].to_ascii_lowercase();
            let mut attrs = Vec::new();
            let mut self_closing = false;
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    break;
                }
                if b[j] == b'>' {
                    j += 1;
                    break;
                }
                if b[j] == b'/' {
                    self_closing = true;
                    j += 1;
                    continue;
                }
                let ks = j;
                while j < b.len()
                    && !b[j].is_ascii_whitespace()
                    && !matches!(b[j], b'=' | b'>' | b'/')
                {
                    j += 1;
                }
                let key = src[ks..j].to_ascii_lowercase();
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let mut val = String::new();
                if j < b.len() && b[j] == b'=' {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                        let q = b[j];
                        let vs = j + 1;
                        let ve = (vs..b.len()).find(|&k| b[k] == q).unwrap_or(b.len());
                        val = decode_entities(&src[vs..ve]);
                        j = (ve + 1).min(b.len());
                    } else {
                        let vs = j;
                        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        val = decode_entities(&src[vs..j]);
                    }
                }
                if !key.is_empty() {
                    attrs.push((key, val));
                } else {
                    j += 1;
                }
            }
            let parent = stack.last().copied();
            let prev_sibling = match parent {
                Some(p) => last_child[p],
                None => root_last,
            };
            let idx = elements.len();
            elements.push(Element {
                name: name.clone(),
                attrs,
                parent,
                prev_sibling,
                text: String::new(),
            });
            last_child.push(None);
            match parent {
                Some(p) => last_child[p] = Some(idx),
                None => root_last = Some(idx),
            }
            i = j;
            if RAW_TEXT.contains(&name.as_str()) && !self_closing {
                let close = format!("</{name}");
                let end = find_ci(src, i, &close).unwrap_or(b.len());
                elements[idx].text = src[i..end].to_string();
                i = src[end..].find('>').map_or(b.len(), |p| end + p + 1);
            } else if !self_closing && !VOID.contains(&name.as_str()) {
                stack.push(idx);
            }
        }
        Self { elements }
    }

    pub fn select(&self, sel: &CssSelector) -> Vec<usize> {
        (0..self.elements.len())
            .filter(|&i| self.matches_at(sel, sel.parts.len() - 1, i))
            .collect()
    }

    fn matches_at(&self, sel: &CssSelector, part: usize, idx: usize) -> bool {
        if !sel.parts[part].1.matches(&self.elements[idx]) {
            return false;
        }
        if part == 0 {
            return true;
        }
        let e = &self.elements[idx];
        match sel.parts[part].0 {
            Combinator::Descendant => {
                let mut cur = e.parent;
                while let Some(p) = cur {
                    if self.matches_at(sel, part - 1, p) {
                        return true;
                    }
                    cur = self.elements[p].parent;
                }
                false
            }
            Combinator::Child => e.parent.is_some_and(|p| self.matches_at(sel, part - 1, p)),
            Combinator::Adjacent => e
                .prev_sibling
                .is_some_and(|p| self.matches_at(sel, part - 1, p)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
    Adjacent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttrOp {
    Has,
    Eq(String),
    Token(String),
    Prefix(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Compound {
    tag: Option<String>,
    attrs: Vec<(String, AttrOp)>,
    nots: Vec<Compound>,
    invalid_json: bool,
    robots_noindex: bool,
}

impl Compound {
    fn matches(&self, e: &Element) -> bool {
        if self.tag.as_ref().is_some_and(|t| *t != e.name) {
            return false;
        }
        for (k, op) in &self.attrs {
            let Some(v) = e.attr(k) else { return false };
            let ok = match op {
                AttrOp::Has => true,
                AttrOp::Eq(x) => v.trim().eq_ignore_ascii_case(x),
                AttrOp::Token(x) => v
                    .split(|c: char| c.is_ascii_whitespace() || c == ',')
                    .any(|t| t.eq_ignore_ascii_case(x)),
                AttrOp::Prefix(x) => v
                    .as_bytes()
                    .get(..x.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(x.as_bytes())),
            };
            if !ok {
                return false;
            }
        }
        if self.nots.iter().any(|n| n.matches(e)) {
            return false;
        }
        if self.robots_noindex && !is_robots_noindex(e) {
            return false;
        }
        if self.invalid_json && serde_json::from_str::<serde_json::Value>(&e.text).is_ok() {
            return false;
        }
        true
    }
}

/// A parsed selector: compounds joined by combinators (first combinator unused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssSelector {
    parts: Vec<(Combinator, Compound)>,
}

impl CssSelector {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let err = |m: &str| Error::Selector(format!("{m} in `{s}`"));
        let c: Vec<char> = s.chars().collect();
        let mut i = 0;
        let mut parts = Vec::new();
        let mut comb = Combinator::Descendant;
        loop {
            let mut saw_ws = false;
            while i < c.len() && c[i].is_whitespace() {
                i += 1;
                saw_ws = true;
            }
            if i >= c.len() {
                break;
            }
            if c[i] == '>' || c[i] == '+' {
                if parts.is_empty() {
                    return Err(err("leading combinator"));
                }
                comb = if c[i] == '>' {
                    Combinator::Child
                } else {
                    Combinator::Adjacent
                };
                i += 1;
                continue;
            }
            if !parts.is_empty() && !saw_ws && comb == Combinator::Descendant {
                return Err(err("missing combinator"));
            }
            let (compound, next) = parse_compound(&c, i).map_err(|m| err(&m))?;
            parts.push((comb, compound));
            comb = Combinator::Descendant;
            i = next;
        }
        if parts.is_empty() {
            return Err(err("empty selector"));
        }
        Ok(Self { parts })
    }
}

/// `<meta name="robots">` whose content carries `noindex` or `none`.
pub fn is_robots_noindex(e: &Element) -> bool {
    e.name == "meta"
        && e.attr("name")
            .is_some_and(|n| n.trim().eq_ignore_ascii_case("robots"))
        && e.attr("content").is_some_and(|c| {
            c.split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                .any(|t| t.eq_ignore_ascii_case("noindex") || t.eq_ignore_ascii_case("none"))
        })
}

fn parse_compound(c: &[char], mut i: usize) -> Result<(Compound, usize), String> {
    let mut out = Compound::default();
    let start = i;
    while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '-' || c[i] == '*') {
        i += 1;
    }
    let tag: String = c[start..i].iter().collect();
    if !tag.is_empty() && tag != "*" {
        out.tag = Some(tag.to_ascii_lowercase());
    }
    while i < c.len() && !c[i].is_whitespace() && c[i] != '>' && c[i] != '+' && c[i] != ')' {
        match c[i] {
            '[' => {
                let end = (i..c.len()).find(|&k| c[k] == ']').ok_or("unclosed [")?;
                let body: String = c[i + 1..end].iter().collect();
                let (key, op) = if let Some((k, v)) = body.split_once("~=") {
                    (k, AttrOp::Token(unquote(v)))
                } else if let Some((k, v)) = body.split_once("^=") {
                    (k, AttrOp::Prefix(unquote(v)))
                } else if let Some((k, v)) = body.split_once('=') {
                    (k, AttrOp::Eq(unquote(v)))
                } else {
                    (body.as_str(), AttrOp::Has)
                };
                let key = key.trim().to_ascii_lowercase();
                if key.is_empty() {
                    return Err("empty attribute name".into());
                }
                out.attrs.push((key, op));
                i = end + 1;
            }
            ':' => {
                let rest: String = c[i..].iter().collect();
                if rest.starts_with(":not(") {
                    let (inner, next) = parse_compound(c, i + 5)?;
                    if next >= c.len() || c[next] != ')' {
                        return Err("unclosed :not(".into());
                    }
                    out.nots.push(inner);
                    i = next + 1;
                } else if rest.starts_with(":robots-noindex") {
                    out.robots_noindex = true;
                    i += ":robots-noindex".len();
                } else if rest.starts_with(":invalid-json") {
                    out.invalid_json = true;
                    i += ":invalid-json".len();
                } else {
                    return Err(format!("unsupported pseudo-class `{rest}`"));
                }
            }
            ch => return Err(format!("unexpected `{ch}`")),
        }
    }
    if i == start {
        return Err("empty compound".into());
    }
    Ok((out, i))
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(v)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(html: &str, sel: &str) -> usize {
        HtmlDoc::parse(html)
            .select(&CssSelector::parse(sel).unwrap())
            .len()
    }

    #[test]
    fn selectors() {
        let h = r#"<!doctype html><html lang="en"><head><title>A</title>
            <link rel="canonical alternate" href="x"><meta name=description content="d">
            <script type="application/ld+json">{"@type":"Article"}</script>
            <script type="application/ld+json">{broken</script></head>
            <body><!-- <img> --><h1>T</h1><p>a</p><img src=a><img alt="" src=b><br/></body></html>"#;
        assert_eq!(count(h, "head title"), 1);
        assert_eq!(count(h, "head > title"), 1);
        assert_eq!(count(h, "body title"), 0);
        assert_eq!(count(h, "html[lang]"), 1);
        assert_eq!(count(h, "link[rel~=CANONICAL]"), 1);
        assert_eq!(count(h, "meta[name=description]"), 1);
        assert_eq!(count(h, "img:not([alt])"), 1);
        assert_eq!(count(h, "h1 + p"), 1);
        assert_eq!(count(h, "p + h1"), 0);
        assert_eq!(
            count(h, r#"script[type="application/ld+json"]:invalid-json"#),
            1
        );
    }

    #[test]
    fn rejects_unsupported() {
        assert!(CssSelector::parse("a:hover").is_err());
        assert!(CssSelector::parse("").is_err());
        assert!(CssSelector::parse("[unclosed").is_err());
    }
}
