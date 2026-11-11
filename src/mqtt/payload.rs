use std::collections::HashMap;

use serde_json::Value;

/// One MQTT payload, turned into the keys a device snapshot needs.
///
/// Verified shapes:
/// - `get_reply` full snapshot:
///   `{"id":874179349,"version":"1.0","operateType":"latestQuotas","moduleSn":"R351ZEB5PGC90303",
///   "code":"0","data":{"sn":"…","productType":81,"model":1,"online":1,"quotaMap":{…}}}`
/// - partial block on the quota topic:
///   `{"moduleType":1,"needAck":0,"id":2935860,"time":2935860,"params":{…},"version":"1.0","typeCode":"pdStatus"}`
///   where `params` carries only the changed keys, unprefixed.
/// - status: `{"id":"268020839","params":{"status":0},"timestamp":1755833679618,"version":"1.0"}`
/// - offline (River series capture): `{"header":[{"code":"-2"}]}`
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    /// Property keys, always in the dotted form the normalizer expects.
    pub keys: HashMap<String, Value>,
    pub online: Option<bool>,
    pub code: Option<String>,
    /// `params.status` verbatim, for diagnostics.
    pub status_raw: Option<i64>,
    pub message: Option<String>,
    pub product_type: Option<u16>,
    /// The command id. A live DELTA 2 Max echoes the id its `set` command carried on
    /// the `set_reply`, and the status topic reports an id as a string.
    pub id: Option<u64>,
}

/// `typeCode` → key prefix, taken from the meta keys that survive in a real merged
/// diagnostics dump (`pd.typeCode=pdStatus`, `bms_bmsStatus.typeCode=bmsStatus`,
/// `bms_emsStatus.typeCode=emsStatus`, `inv.typeCode=invStatus`,
/// `mppt.typeCode=mpptStatus`, `unknown_kitInfo.typeCode=kitInfo`).
pub fn prefix_for_typecode(typecode: &str) -> Option<&'static str> {
    match typecode {
        "pdStatus" => Some("pd"),
        "bmsStatus" => Some("bms_bmsStatus"),
        "emsStatus" => Some("bms_emsStatus"),
        "mpptStatus" => Some("mppt"),
        "invStatus" => Some("inv"),
        "kitInfo" => Some("bms_kitInfo"),
        _ => None,
    }
}

pub fn parse(payload: &[u8]) -> Parsed {
    let v: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(_) => return Parsed::default(),
    };
    let mut out = Parsed::default();

    // `header` is the envelope the mobile-app API uses for refusals.
    if let Some(code) = v
        .get("header")
        .and_then(|h| h.as_array())
        .and_then(|a| a.first())
        .and_then(|h| h.get("code"))
        .and_then(|c| c.as_str())
    {
        out.code = Some(code.to_string());
        if code == "-2" {
            out.online = Some(false);
        }
        return out;
    }

    if let Some(c) = v.get("code").and_then(|c| c.as_str()) {
        out.code = Some(c.to_string());
    }
    if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
        out.message = Some(m.to_string());
    }
    // The command id, which the live device echoes back on `set_reply`.
    out.id = v.get("id").and_then(|i| {
        i.as_u64()
            .or_else(|| i.as_str().and_then(|s| s.parse().ok()))
    });

    // Full snapshot.
    if let Some(data) = v.get("data") {
        if let Some(qm) = data.get("quotaMap").and_then(|q| q.as_object()) {
            for (k, val) in qm {
                out.keys.insert(k.clone(), val.clone());
            }
        }
        if let Some(pt) = data.get("productType").and_then(|p| p.as_u64()) {
            out.product_type = Some(pt as u16);
            // Store it under the dotted name the normalizer reads.
            out.keys
                .insert("bms_bmsStatus.productType".to_string(), Value::from(pt));
        }
        if let Some(on) = data.get("online") {
            out.online = Some(matches!(on, Value::Bool(true)) || as_num(Some(on)) == 1);
        }
    }

    // Partial block, or the status topic.
    if let Some(params) = v.get("params").and_then(|p| p.as_object()) {
        let status_only = params.len() == 1 && params.contains_key("status");
        if status_only {
            out.status_raw = Some(as_num(params.get("status")));
            // tolwi reads `status == 1` as online, but the field's meaning is only
            // partly verified: a live DELTA 2 Max pushes `status: 0` while it is
            // connected and streaming. A message arriving on the device's own topics
            // is proof the broker reaches it, so a push counts as online and the raw
            // value is kept for the diagnostics view.
            out.online = Some(true);
            return out;
        }
        // The block is prefixed by its typeCode; keys that already carry a prefix stay.
        let prefix = v
            .get("typeCode")
            .and_then(|t| t.as_str())
            .and_then(prefix_for_typecode);
        for (k, val) in params {
            let key = if k.contains('.') {
                k.clone()
            } else if let Some(p) = prefix {
                format!("{p}.{k}")
            } else {
                k.clone()
            };
            out.keys.insert(key, val.clone());
        }
        // Blocks that report the product type give the family.
        if let Some(pt) = out
            .keys
            .get("bms_bmsStatus.productType")
            .and_then(|p| p.as_u64())
        {
            out.product_type = Some(pt as u16);
        }
    }

    out
}

fn as_num(v: Option<&Value>) -> i64 {
    v.map(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// The `latestQuotas` publish that makes a device push its snapshot again. Verified
/// request body from the docs and tolwi/iobroker agreement.
pub fn latest_quotas() -> Vec<u8> {
    serde_json::json!({
        "version": "1.0",
        "moduleType": 0,
        "operateType": "latestQuotas",
        "params": {}
    })
    .to_string()
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_snapshot_capture_is_parsed() {
        // Verbatim DELTA 2 Max get_reply capture (tolwi issue #605).
        let payload = br#"{"id":874179349,"version":"1.0","operateType":"latestQuotas","moduleSn":"R351ZEB5PGC90303","code":"0","data":{"sn":"R351ZEB5PGC90303","productType":81,"model":1,"online":1,"quotaMap":{"bms_bmsStatus.soc":85,"pd.wattsInSum":0,"bms_emsStatus.bmsIsConnt":[3,0,0]}}}"#;
        let p = parse(payload);
        assert_eq!(p.code.as_deref(), Some("0"));
        assert_eq!(p.keys.get("bms_bmsStatus.soc"), Some(&Value::from(85)));
        assert_eq!(p.product_type, Some(81));
        assert_eq!(p.online, Some(true));
        assert_eq!(
            p.keys.get("bms_emsStatus.bmsIsConnt").unwrap(),
            &Value::from(vec![3, 0, 0])
        );
    }

    #[test]
    fn partial_block_gets_its_prefix() {
        // The docs' bmsStatus example carries one key: `{"params":{"remainTime":0}}`.
        let payload = br#"{"moduleType":2,"needAck":0,"id":2935860,"time":2935860,"params":{"remainTime":0,"soc":85},"version":"1.0","typeCode":"bmsStatus"}"#;
        let p = parse(payload);
        assert_eq!(
            p.keys.get("bms_bmsStatus.remainTime"),
            Some(&Value::from(0))
        );
        assert_eq!(p.keys.get("bms_bmsStatus.soc"), Some(&Value::from(85)));
    }

    #[test]
    fn keys_that_already_carry_a_prefix_are_left_alone() {
        let payload = br#"{"params":{"pd.wattsOutSum":218},"typeCode":"pdStatus","version":"1.0"}"#;
        let p = parse(payload);
        assert_eq!(p.keys.get("pd.wattsOutSum"), Some(&Value::from(218)));
    }

    #[test]
    fn status_topic_marks_the_device_reachable() {
        // A live DELTA 2 Max pushes `status: 0` while it is connected and streaming,
        // so a push on the device's topics counts as reachable and the raw value is
        // kept for diagnostics.
        let p = parse(br#"{"id":"268020839","params":{"status":0},"timestamp":1755833679618,"version":"1.0"}"#);
        assert_eq!(p.online, Some(true));
        assert_eq!(p.status_raw, Some(0));
        assert!(p.keys.is_empty(), "a status message is not telemetry");
        let p = parse(br#"{"params":{"status":1},"version":"1.0"}"#);
        assert_eq!(p.online, Some(true));
        assert_eq!(p.status_raw, Some(1));
    }

    #[test]
    fn offline_header_is_parsed() {
        let p = parse(br#"{"header":[{"code":"-2"}]}"#);
        assert_eq!(p.code.as_deref(), Some("-2"));
        assert_eq!(p.online, Some(false));
    }

    #[test]
    fn not_owned_code_is_reported() {
        let p = parse(br#"{"code":"-1","message":"device is not owned"}"#);
        assert_eq!(p.code.as_deref(), Some("-1"));
        assert_eq!(p.message.as_deref(), Some("device is not owned"));
        assert!(p.keys.is_empty());
    }

    #[test]
    fn garbage_payload_is_ignored_not_a_panic() {
        assert_eq!(parse(b"not json"), Parsed::default());
        assert_eq!(parse(b""), Parsed::default());
        assert_eq!(parse(b"null"), Parsed::default());
    }

    #[test]
    fn prefixes_match_the_verified_typecodes() {
        assert_eq!(prefix_for_typecode("pdStatus"), Some("pd"));
        assert_eq!(prefix_for_typecode("bmsStatus"), Some("bms_bmsStatus"));
        assert_eq!(prefix_for_typecode("emsStatus"), Some("bms_emsStatus"));
        assert_eq!(prefix_for_typecode("mpptStatus"), Some("mppt"));
        assert_eq!(prefix_for_typecode("invStatus"), Some("inv"));
        assert_eq!(prefix_for_typecode("kitInfo"), Some("bms_kitInfo"));
        assert_eq!(prefix_for_typecode("unknown"), None);
    }

    #[test]
    fn latest_quotas_body_is_the_documented_one() {
        let s = String::from_utf8(latest_quotas()).unwrap();
        assert_eq!(
            s,
            r#"{"moduleType":0,"operateType":"latestQuotas","params":{},"version":"1.0"}"#
        );
    }
}
