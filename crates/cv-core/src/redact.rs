//! Redaction rules shared by every surface that writes diagnostics out of
//! the app: structured logs and the Doctor support bundle. Keys that look
//! like a secret are dropped, never masked, so a careless caller cannot
//! leak a credential by choosing a new field name.

use serde_json::Value;

/// Whether a field named `key` may hold a credential.
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("secret")
        || key.contains("password")
        || key.contains("passphrase")
        || key.contains("token")
        || key.contains("nsec")
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("credential")
        || key.starts_with("private_key")
        || key == "authorization"
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
            "authorization",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in ["page", "root", "status", "key", "count"] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn nested_secret_fields_are_dropped() {
        let mut v = json!({
            "version": "boris/0.9.0",
            "apiToken": "leak",
            "nested": [{ "password": "leak", "ok": 1 }],
        });
        drop_secret_keys(&mut v);
        assert_eq!(
            v,
            json!({ "version": "boris/0.9.0", "nested": [{ "ok": 1 }] })
        );
    }
}
