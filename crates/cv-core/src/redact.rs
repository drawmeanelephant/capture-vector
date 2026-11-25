//! Redaction rules shared by every surface that writes diagnostics out of
//! the app: structured logs and the Doctor support bundle. Keys that look
//! like a secret are dropped, never masked, so a careless caller cannot
//! leak a credential by choosing a new field name.

use serde_json::Value;

/// Split a key into words across camelCase, digit and `-`/`_` boundaries,
/// lowercased. `clientPrivateKey` and `client-private-key` both parse to
/// `[client, private, key]`; an all-caps run with no boundary information
/// (`PRIVATEKEY`) stays one fused word.
fn key_words(key: &str) -> Vec<String> {
    let chars: Vec<char> = key.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut last: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, '-' | '_') {
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
    // Key spelling is not a security boundary: JSON producers use snake,
    // kebab and camel case interchangeably. Compare only the letters, so
    // `privateKey`, `private-key` and `private_key` share one rule.
    let squashed: String = key
        .chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if squashed.contains("secret")
        || squashed.contains("password")
        || squashed.contains("passphrase")
        || squashed.contains("token")
        || squashed.contains("nsec")
        || squashed.contains("apikey")
        || squashed.contains("credential")
        || squashed == "authorization"
    {
        return true;
    }
    // `private key` is a word pair, not a prefix: `clientPrivateKey` is as
    // secret as `privateKey`, while `publicKey` stays innocent. A fused
    // word carries no boundary information, so one containing `privatekey`
    // contiguously is treated as the pair rather than trusted.
    let words = key_words(key);
    words
        .windows(2)
        .any(|w| w[0] == "private" && (w[1] == "key" || w[1] == "keys"))
        || words.iter().any(|w| w.contains("privatekey"))
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
