//! Display formatting. Values are rendered in fixed-width columns so the dashboard
//! stays aligned; `None` renders as a dash rather than a fabricated zero.

use std::sync::Mutex;

pub const NO_VALUE: &str = "--";

/// Power in watts. Below 1 kW show whole watts, above show kW with one decimal.
pub fn watts(w: Option<f64>) -> String {
    match w {
        None => NO_VALUE.to_string(),
        Some(v) => {
            if v.abs() >= 1000.0 {
                format!("{:.1} kW", v / 1000.0)
            } else {
                format!("{:.0} W", v)
            }
        }
    }
}

/// Energy in watt-hours, grouped with thousands separators.
pub fn wh(wh: Option<f64>) -> String {
    match wh {
        None => NO_VALUE.to_string(),
        Some(v) => {
            if v.abs() >= 1000.0 {
                format!("{:.2} kWh", v / 1000.0)
            } else {
                format!("{} Wh", grouped(v.round() as i64))
            }
        }
    }
}

pub fn percent(p: Option<f64>) -> String {
    match p {
        None => NO_VALUE.to_string(),
        Some(v) => format!("{:.0}%", v),
    }
}

pub fn volts(v: Option<f64>) -> String {
    match v {
        None => NO_VALUE.to_string(),
        Some(v) => format!("{:.1} V", v),
    }
}

pub fn amps(a: Option<f64>) -> String {
    match a {
        None => NO_VALUE.to_string(),
        Some(v) => format!("{:.2} A", v),
    }
}

pub fn temp(c: Option<f64>, fahrenheit: bool) -> String {
    match c {
        None => NO_VALUE.to_string(),
        Some(v) => {
            if fahrenheit {
                format!("{:.0} °F", v * 1.8 + 32.0)
            } else {
                format!("{:.0} °C", v)
            }
        }
    }
}

/// Minutes to a human duration: `7h 23m`, `411m`, `--`.
pub fn duration_min(min: Option<f64>) -> String {
    match min {
        None => NO_VALUE.to_string(),
        Some(v) if v < 0.0 => "infinite".to_string(),
        Some(v) => {
            let total = v.round() as i64;
            if total >= 60 {
                format!("{}h {:02}m", total / 60, total % 60)
            } else {
                format!("{total}m")
            }
        }
    }
}

/// Age of the last update, e.g. `3s`, `2m 05s`.
pub fn elapsed(secs: f64) -> String {
    if secs < 1.0 {
        "just now".to_string()
    } else if secs < 60.0 {
        format!("{:.0}s", secs)
    } else {
        format!("{}m {:02}s", (secs / 60.0).floor() as i64, secs as i64 % 60)
    }
}

pub fn grouped(n: i64) -> String {
    let s = n.to_string();
    let neg = s.starts_with('-');
    let digits = if neg { &s[1..] } else { &s };
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if neg {
        out.insert(0, '-')
    }
    out
}

/// One or many, in words: `1 sample`, `7 samples`. Notes read better with it.
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Mask a serial number: keep the first two and last two characters, the middle is
/// `*`, so the device is recognisable in a log without the serial being readable.
pub fn mask_serial(sn: &str) -> String {
    if sn.len() <= 4 {
        return "****".to_string();
    }
    let head = &sn[..2];
    let tail = &sn[sn.len() - 2..];
    let stars = sn.len() - 4;
    format!("{head}{}{tail}", "*".repeat(stars))
}

/// Values that must never appear in a log line: API keys, MQTT credentials, and
/// device serials. Registered as they are learned (the credential file, the
/// certification endpoint, the device list), and the log writer masks every line
/// through them. A short value is not registered: masking a 5-character string
/// would blank ordinary words.
static SECRETS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn register_secret(value: &str, masked: &str) {
    if value.len() < 6 || value == masked {
        return;
    }
    let Ok(mut list) = SECRETS.lock() else {
        return;
    };
    if !list.iter().any(|(v, _)| v == value) {
        list.push((value.to_string(), masked.to_string()));
    }
}

pub fn register_serial(sn: &str) {
    register_secret(sn, &mask_serial(sn));
}

/// Copy `text` with every registered value replaced by its mask.
pub fn mask_secrets(text: &str) -> String {
    let mut out = text.to_string();
    if let Ok(list) = SECRETS.lock() {
        for (value, masked) in list.iter() {
            out = out.replace(value.as_str(), masked);
        }
    }
    out
}

/// Copy a property map with every serial value masked. The device reports its own
/// serial inside the property values (`bms_bmsInfo.sn`, `bms_bmsStatus.packSn`), so
/// an export of the raw map has to mask them, not just the keys.
pub fn mask_values(
    map: &std::collections::HashMap<String, serde_json::Value>,
    sn: &str,
) -> serde_json::Value {
    let masked = mask_serial(sn);
    let mut out = serde_json::Map::new();
    for (k, v) in map {
        let v = match v {
            serde_json::Value::String(s) if s.contains(sn) => {
                serde_json::Value::String(s.replace(sn, &masked))
            }
            serde_json::Value::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(|i| {
                        if let serde_json::Value::String(s) = i {
                            serde_json::Value::String(s.replace(sn, &masked))
                        } else {
                            i.clone()
                        }
                    })
                    .collect(),
            ),
            serde_json::Value::Object(sub) => {
                let mut m = serde_json::Map::new();
                for (sk, sv) in sub {
                    if let serde_json::Value::String(s) = sv {
                        m.insert(
                            sk.clone(),
                            serde_json::Value::String(s.replace(sn, &masked)),
                        );
                    } else {
                        m.insert(sk.clone(), sv.clone());
                    }
                }
                serde_json::Value::Object(m)
            }
            other => other.clone(),
        };
        out.insert(k.clone(), v);
    }
    serde_json::Value::Object(out)
}

/// Clamp text to a display width, padding with spaces. Unicode width is approximate:
/// every char counts as one column, which is correct for the ASCII labels we use.
pub fn pad(s: &str, width: usize) -> String {
    let need = width.saturating_sub(s.chars().count());
    let mut out = String::with_capacity(s.len() + need);
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', need));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watts_switches_to_kw() {
        assert_eq!(watts(Some(230.0)), "230 W");
        assert_eq!(watts(Some(1420.0)), "1.4 kW");
        assert_eq!(watts(None), "--");
    }

    #[test]
    fn energy_groups_thousands() {
        assert_eq!(wh(Some(1420.0)), "1.42 kWh");
        assert_eq!(wh(Some(850.0)), "850 Wh");
        assert_eq!(wh(Some(-1234567.0)), "-1234.57 kWh");
    }

    #[test]
    fn durations() {
        assert_eq!(duration_min(Some(411.0)), "6h 51m");
        assert_eq!(duration_min(Some(45.0)), "45m");
        assert_eq!(duration_min(Some(-1.0)), "infinite");
        assert_eq!(duration_min(None), "--");
    }

    #[test]
    fn plurals_read_as_english() {
        assert_eq!(plural(1, "sample"), "1 sample");
        assert_eq!(plural(7, "sample"), "7 samples");
        assert_eq!(plural(0, "sample"), "0 samples");
    }

    #[test]
    fn serial_masking_keeps_head_and_tail() {
        assert_eq!(mask_serial("R351ZEB5PGC90303"), "R3************03");
        assert_eq!(mask_serial("AB"), "****");
    }

    #[test]
    fn registered_values_are_masked_in_text() {
        register_serial("R351ZEB5PGC90303");
        register_secret("WIbFEKre0s6sLnh4ei7SPUeYnptHG6V", "<redacted>");
        let line = "publish {\"sn\":\"R351ZEB5PGC90303\"} secret WIbFEKre0s6sLnh4ei7SPUeYnptHG6V";
        let masked = mask_secrets(line);
        assert!(
            !masked.contains("R351ZEB5PGC90303"),
            "the plain serial is masked: {masked}"
        );
        assert!(
            masked.contains("R3************03"),
            "the device is recognisable"
        );
        assert!(
            !masked.contains("WIbFEKre0s6sLnh4ei7SPUeYnptHG6V"),
            "the secret is gone: {masked}"
        );
    }

    #[test]
    fn short_values_are_not_registered() {
        // Masking a 5-character string would blank ordinary words in the log.
        register_secret("abcde", "<redacted>");
        assert_eq!(mask_secrets("abcde"), "abcde");
    }

    #[test]
    fn grouping_negative_and_small() {
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(-999), "-999");
        assert_eq!(grouped(5), "5");
    }

    #[test]
    fn padding_is_exact() {
        assert_eq!(pad("abc", 5), "abc  ");
        assert_eq!(pad("abcdef", 3), "abcdef");
    }
}
