//! JSONPath subset: `$`, `.key`, `['key']`, `[n]`, `[*]`, `.*`. Keys may
//! contain `@`, `-`, `:`. A match counts only when it is non-null.

use crate::Error;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
    Wild,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath(Vec<Seg>);

/// Selectors under `$http` name facts only a live HTTP fetch can supply
/// (status, headers, redirects, timings); cv-rules reports them `Limited`.
pub fn is_live_only(path: &str) -> bool {
    path == "$http" || path.starts_with("$http.") || path.starts_with("$http[")
}

impl JsonPath {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let err = |m: &str| Error::Selector(format!("{m} in JSONPath `{s}`"));
        let c: Vec<char> = s.chars().collect();
        if c.first() != Some(&'$') {
            return Err(err("must start with `$`"));
        }
        let mut i = 1;
        let mut segs = Vec::new();
        while i < c.len() {
            match c[i] {
                '.' => {
                    let st = i + 1;
                    i = st;
                    while i < c.len() && c[i] != '.' && c[i] != '[' {
                        i += 1;
                    }
                    let key: String = c[st..i].iter().collect();
                    match key.as_str() {
                        "" => return Err(err("empty key")),
                        "*" => segs.push(Seg::Wild),
                        _ => segs.push(Seg::Key(key)),
                    }
                }
                '[' => {
                    let end = (i..c.len())
                        .find(|&k| c[k] == ']')
                        .ok_or_else(|| err("unclosed ["))?;
                    let body: String = c[i + 1..end].iter().collect();
                    let body = body.trim();
                    if body == "*" {
                        segs.push(Seg::Wild);
                    } else if let Some(k) = body
                        .strip_prefix('\'')
                        .and_then(|b| b.strip_suffix('\''))
                        .or_else(|| body.strip_prefix('"').and_then(|b| b.strip_suffix('"')))
                    {
                        segs.push(Seg::Key(k.to_string()));
                    } else {
                        segs.push(Seg::Index(body.parse().map_err(|_| err("bad index"))?));
                    }
                    i = end + 1;
                }
                ch => return Err(err(&format!("unexpected `{ch}`"))),
            }
        }
        Ok(Self(segs))
    }

    pub fn select<'a>(&self, root: &'a Value) -> Vec<&'a Value> {
        let mut cur = vec![root];
        for seg in &self.0 {
            let mut next = Vec::new();
            for v in cur {
                match (seg, v) {
                    (Seg::Key(k), Value::Object(m)) => next.extend(m.get(k)),
                    (Seg::Index(n), Value::Array(a)) => next.extend(a.get(*n)),
                    (Seg::Wild, Value::Array(a)) => next.extend(a.iter()),
                    (Seg::Wild, Value::Object(m)) => next.extend(m.values()),
                    _ => {}
                }
            }
            cur = next;
        }
        cur.retain(|v| !v.is_null());
        cur
    }

    pub fn count(&self, root: &Value) -> u64 {
        self.select(root).len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths() {
        let v = json!({"@type": "FAQPage", "a": [{"b": 1}, {"b": null}, {"c": 2}], "t": null});
        let n = |p: &str| JsonPath::parse(p).unwrap().count(&v);
        assert_eq!(n("$"), 1);
        assert_eq!(n("$.@type"), 1);
        assert_eq!(n("$['@type']"), 1);
        assert_eq!(n("$.a[*]"), 3);
        assert_eq!(n("$.a[*].b"), 1);
        assert_eq!(n("$.a[2].c"), 1);
        assert_eq!(n("$.t"), 0);
        assert_eq!(n("$.*"), 2);
        assert!(JsonPath::parse("a.b").is_err());
        assert!(is_live_only("$http.headers.x-robots-tag"));
        assert!(!is_live_only("$.http"));
    }
}
