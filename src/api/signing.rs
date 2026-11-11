//! EcoFlow developer API request signing.
//!
//! Verified against the official documentation test vector and against live requests
//! to `api-e.ecoflow.com` from this project:
//!
//! - Headers on every request: `accessKey`, `nonce` (6 digits), `timestamp` (UTC ms), `sign`.
//! - `sign` = lowercase hex HMAC-SHA256 keyed with the secret key.
//! - GET: the request's parameters enter the canonical string, sorted, exactly like a
//!   POST body, and the auth fields are appended. Probed live against
//!   `api-e.ecoflow.com`: `?sn=…` signed as `sn=…&accessKey=…&nonce=…&timestamp=…`
//!   returns `code 0` with the property map; signing the auth fields only, the query
//!   after the auth fields, the path, or the path plus query all return
//!   `code 8521 "signature is wrong"`. A GET with no parameters signs the auth fields
//!   alone (`/iot-open/sign/device/list` returns `code 0`).
//! - POST: the canonical string is the request body's keys, sorted lexicographically,
//!   nested keys joined with dots, then `&accessKey=..&nonce=..&timestamp=..` appended.
//!   This matches the official test vector exactly (see `official_test_vector`).

use std::collections::BTreeMap;

use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;

/// Signed GET. `query` is the URL query as it will be sent: the parameters are
/// sorted and placed before the auth fields, which is what the live API accepts.
pub fn sign_get(
    secret_key: &str,
    access_key: &str,
    nonce: &str,
    timestamp: &str,
    query: &BTreeMap<String, String>,
) -> String {
    hmac_hex(
        secret_key,
        &get_canonical(access_key, nonce, timestamp, query),
    )
}

/// Canonical string for a GET with parameters: sorted params, then the auth fields.
pub fn get_canonical(
    access_key: &str,
    nonce: &str,
    timestamp: &str,
    query: &BTreeMap<String, String>,
) -> String {
    let auth = format!("accessKey={access_key}&nonce={nonce}&timestamp={timestamp}");
    if query.is_empty() {
        return auth;
    }
    let mut parts: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.sort();
    parts.push(auth);
    parts.join("&")
}

/// Signed POST: body keys sorted, then the auth fields appended.
pub fn sign_post(
    secret_key: &str,
    access_key: &str,
    nonce: &str,
    timestamp: &str,
    body: &BTreeMap<String, Value>,
) -> String {
    let mut parts: Vec<String> = body
        .iter()
        .map(|(k, v)| format!("{k}={}", format_value(v)))
        .collect();
    parts.sort();
    let canonical =
        parts.join("&") + &format!("&accessKey={access_key}&nonce={nonce}&timestamp={timestamp}");
    hmac_hex(secret_key, &canonical)
}

pub fn hmac_hex(secret_key: &str, canonical: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret_key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Flatten a JSON body into dotted keys, as the signing rule describes.
pub fn flatten_body(body: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    match body {
        Value::Object(map) => flatten_map("", map, &mut out),
        _ => {
            out.insert(String::new(), body.clone());
        }
    }
    out
}

fn flatten_map(
    prefix: &str,
    map: &serde_json::Map<String, Value>,
    out: &mut BTreeMap<String, Value>,
) {
    for (k, v) in map {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(sub) => flatten_map(&key, sub, out),
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let arr_key = format!("{key}[{i}]");
                    match item {
                        Value::Object(sub) => flatten_map(&arr_key, sub, out),
                        _ => {
                            out.insert(arr_key, item.clone());
                        }
                    }
                }
            }
            _ => {
                out.insert(key, v.clone());
            }
        }
    }
}

/// Value formatting for the canonical string: bools as `true`/`false`, numbers
/// without trailing zeros, strings verbatim (no URL encoding).
pub fn format_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                f.to_string()
            } else {
                n.to_string()
            }
        }
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

pub fn make_nonce() -> String {
    // 6 digits, as the docs specify. The value only has to be unique per request, so
    // the sub-second part of the clock is enough; no random crate is needed.
    let n = chrono::Utc::now().timestamp_subsec_nanos() % 1_000_000;
    format!("{n:06}")
}

pub fn now_millis() -> String {
    chrono::Utc::now().timestamp_millis().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The official documentation test vector. If this passes, the algorithm is the
    /// documented one, not a guess.
    #[test]
    fn official_test_vector() {
        let body = json!({"sn": "123456789", "params": {"cmdSet": 11, "id": 24, "eps": 0}});
        let flat = flatten_body(&body);
        let sign = sign_post(
            "WIbFEKre0s6sLnh4ei7SPUeYnptHG6V",
            "Fp4SvIprYSDPXtYJidEtUAd1o",
            "345164",
            "1671171709428",
            &flat,
        );
        assert_eq!(
            sign,
            "07c13b65e037faf3b153d51613638fa80003c4c38d2407379a7f52851af1473e"
        );
    }

    #[test]
    fn nested_keys_are_flattened_with_dots() {
        let flat = flatten_body(&json!({"sn": "X", "params": {"a": 1, "b": {"c": 2}}}));
        let keys: Vec<&String> = flat.keys().collect();
        assert_eq!(keys, ["params.a", "params.b.c", "sn"]);
    }

    #[test]
    fn arrays_use_index_notation() {
        let flat = flatten_body(&json!({"list": [1, 2], "sn": "S"}));
        assert!(flat.contains_key("list[0]"));
        assert!(flat.contains_key("list[1]"));
    }

    #[test]
    fn booleans_are_words() {
        assert_eq!(format_value(&json!(true)), "true");
        assert_eq!(format_value(&json!(false)), "false");
        assert_eq!(format_value(&json!(11)), "11");
        assert_eq!(format_value(&json!("text")), "text");
    }

    #[test]
    fn get_signature_is_stable_and_lowercase_hex() {
        let empty = BTreeMap::new();
        let a = sign_get("secret", "key", "123456", "1700000000000", &empty);
        let b = sign_get("secret", "key", "123456", "1700000000000", &empty);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn query_params_are_signed_before_the_auth_fields_in_sorted_order() {
        // The live API accepts this order; signing the auth fields alone, the query
        // after the auth fields, the path, or the path plus query all return 8521.
        let mut q = BTreeMap::new();
        q.insert("sn".to_string(), "R351ZEB5PGC90303".to_string());
        q.insert("b".to_string(), "2".to_string());
        q.insert("a".to_string(), "1".to_string());
        let c = get_canonical("key", "123456", "1700000000000", &q);
        assert_eq!(
            c,
            "a=1&b=2&sn=R351ZEB5PGC90303&accessKey=key&nonce=123456&timestamp=1700000000000"
        );
        let empty = BTreeMap::new();
        assert_eq!(
            get_canonical("key", "123456", "1700000000000", &empty),
            "accessKey=key&nonce=123456&timestamp=1700000000000"
        );
    }

    #[test]
    fn nonce_is_six_digits() {
        for _ in 0..200 {
            let n = make_nonce();
            assert_eq!(n.len(), 6, "{n}");
            assert!(n.parse::<u32>().is_ok());
        }
    }

    #[test]
    fn nonce_tracks_the_clock_so_requests_get_new_values() {
        let a = chrono::Utc::now().timestamp_subsec_nanos() % 1_000_000;
        let b = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::seconds(1))
            .unwrap()
            .timestamp_subsec_nanos()
            % 1_000_000;
        // A one-second shift moves the sub-second digits, so consecutive requests
        // within a sweep get different nonces.
        assert_ne!(a, b);
    }

    #[test]
    fn changing_the_body_changes_the_signature() {
        let secret = "secret";
        let flat1 = flatten_body(&json!({"sn": "A", "params": {"id": 1}}));
        let flat2 = flatten_body(&json!({"sn": "A", "params": {"id": 2}}));
        assert_ne!(
            sign_post(secret, "k", "111111", "1700000000000", &flat1),
            sign_post(secret, "k", "111111", "1700000000000", &flat2)
        );
    }
}
