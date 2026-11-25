//! Redaction rules shared by every surface that writes diagnostics out of
//! the app: structured logs and the Doctor support bundle. Keys that look
//! like a secret are dropped, never masked, so a careless caller cannot
//! leak a credential by choosing a new field name.

use serde_json::Value;

/// Key spelling is not a security boundary: JSON producers use snake,
/// kebab, camel, dot and path separators interchangeably, and flatten
/// nested objects into dotted or slashed key paths.
fn is_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.' | '/' | '\\')
}

/// Split a key into words across camelCase, digit and separator
/// boundaries, lowercased. `clientPrivateKey` and `client-private-key`
/// both parse to `[client, private, key]`; an all-caps run with no
/// boundary information (`PRIVATEKEY`) stays one fused word.
fn key_words(key: &str) -> Vec<String> {
    let chars: Vec<char> = key.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut last: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        if is_separator(c) {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            last = None;
            continue;
        }
        if c.is_ascii_uppercase()
            && let Some(p) = last
        {
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if (p.is_ascii_lowercase()
                || p.is_ascii_digit()
                || (p.is_ascii_uppercase() && next_lower))
                && !cur.is_empty()
            {
                words.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c.to_ascii_lowercase());
        last = Some(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Whether a field named `key` may hold a credential.
pub fn is_secret_key(key: &str) -> bool {
    // Compare only the letters, so `privateKey`, `private-key`,
    // `private_key` and flattened paths like `nested.privateKey` share
    // one rule.
    let squashed: String = key
        .chars()
        .filter(|&c| !is_separator(c))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if squashed.contains("secret")
        || squashed.contains("password")
        || squashed.contains("passphrase")
        || squashed.contains("token")
        || squashed.contains("nsec")
        || squashed.contains("apikey")
        || squashed.contains("credential")
        || squashed.contains("privatekey")
        || squashed == "authorization"
    {
        return true;
    }
    // `private key` is a word pair, not a prefix or an adjacency:
    // `privateSigningKey` is as secret as `clientPrivateKey`, while
    // `publicKey` stays innocent. The fused contiguous case is already
    // caught above; this catches `private` followed by `key`/`keys`
    // with intervening words such as `signing`.
    let words = key_words(key);
    words
        .iter()
        .enumerate()
        .any(|(i, w)| w == "private" && words[i + 1..].iter().any(|w| w == "key" || w == "keys"))
}

/// Drop every object field, at any depth, whose key [`is_secret_key`].
pub fn drop_secret_keys(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| !is_secret_key(k));
            m.values_mut().for_each(drop_secret_keys);
        }
        Value::Array(a) => a.iter_mut().for_each(drop_secret_keys),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_keys_are_recognized() {
        for key in [
            "secret",
            "api_key",
            "CV_NOSTR_NSEC",
            "password",
            "access_token",
            "private_key",
            "privateKey",
            "private-key",
            "PRIVATEKEY",
            "api-key",
            "apiKey",
            "secretKey",
            "access-token",
            "authorization",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in [
            "page",
            "root",
            "status",
            "key",
            "count",
            "monkey",
            "publicKey",
            "keyboard",
        ] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn compound_private_key_variants_are_recognized() {
        for key in [
            "clientPrivateKey",
            "client_private_key",
            "client-private-key",
            "CLIENT_PRIVATE_KEY",
            "signingPrivateKey",
            "privateKeys",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in ["publicKey", "clientPublicKey", "key", "keyboard", "monkey"] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn postmerge_probe_spellings_are_secret() {
        // Exact spellings reproduced by the post-merge review probe.
        for key in [
            "privateSigningKey",
            "private_signing_key",
            "clientPrivateSigningKey",
            "clientprivateKey",
            "clientprivate-key",
            "private-keymaterial",
            "private-keypem",
            "nested.privateKey",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in [
            "publicKey",
            "clientPublicKey",
            "nested.publicKey",
            "keyboard",
        ] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn flattened_path_separators_do_not_bypass_protection() {
        for key in [
            "a/b/privateKey",
            "a\\b\\privateKey",
            "nested.private.signing.key",
            "deeply.nested.clientPrivateKey",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn nested_compound_private_keys_are_dropped() {
        let mut v = json!({
            "clientPrivateKey": "leak",
            "inner": { "client_private_key": "leak", "client-public-key2x": "ok" },
            "list": [{ "signingPrivateKey": "leak", "clientPublicKey": "not-secret" }],
        });
        drop_secret_keys(&mut v);
        assert_eq!(
            v,
            json!({
                "inner": { "client-public-key2x": "ok" },
                "list": [{ "clientPublicKey": "not-secret" }],
            })
        );
    }

    #[test]
    fn nested_secret_fields_are_dropped() {
        let mut v = json!({
            "version": "boris/0.9.0",
            "apiToken": "leak",
            "privateKey": "leak",
            "nested": [{ "password": "leak", "ok": 1, "publicKey": "not-secret" }],
        });
        drop_secret_keys(&mut v);
        assert_eq!(
            v,
            json!({ "version": "boris/0.9.0", "nested": [{ "ok": 1, "publicKey": "not-secret" }] })
        );
    }
}
