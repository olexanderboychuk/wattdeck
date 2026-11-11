use crate::api::client::{ApiError, Client};
use crate::services::DiscoveredDevice;
use crate::utils::formatting::{mask_serial, register_secret, register_serial};
use serde_json::Value;

/// Signed device list. Verified live: the payload is an array of
/// `{"sn", "deviceName", "online", "productName"}`. `online` is a number (1/0).
pub async fn device_list(client: &Client) -> Result<Vec<DiscoveredDevice>, ApiError> {
    let data = client.get("/iot-open/sign/device/list", &[]).await?;
    Ok(parse_device_list(&data))
}

/// Signed property snapshot for one device. Verified live: `data` is the flat map of
/// dotted property keys (`bms_bmsStatus.soc`, `pd.wattsOutSum`, …) for the serial.
pub async fn properties(client: &Client, sn: &str) -> Result<Value, ApiError> {
    client
        .get("/iot-open/sign/device/quota/all", &[("sn", sn.to_string())])
        .await
}

pub async fn mqtt_credentials(client: &Client) -> Result<MqttCredentials, ApiError> {
    let data = client.get("/iot-open/sign/certification", &[]).await?;
    MqttCredentials::from_value(&data)
}

/// MQTT broker access, as the certification endpoint reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct MqttCredentials {
    pub username: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    /// `protocol` is reported as `mqtts`, i.e. TLS.
    pub tls: bool,
}

impl MqttCredentials {
    fn from_value(v: &Value) -> Result<Self, ApiError> {
        let get = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        if v.get("certificateAccount").is_none() || v.get("certificatePassword").is_none() {
            return Err(ApiError::Malformed(
                "certification has no credentials".into(),
            ));
        }
        let port: u16 = get("port")
            .parse()
            .map_err(|_| ApiError::Malformed(format!("port {:?}", v.get("port"))))?;
        let proto = get("protocol").to_lowercase();
        let out = Self {
            username: get("certificateAccount"),
            password: get("certificatePassword"),
            host: get("url"),
            port,
            tls: proto == "mqtts" || proto == "ssl" || proto == "tls",
        };
        // The broker password is a credential, and the username is the first path
        // element of every developer topic, so both are registered for the log writer.
        register_secret(&out.password, "<redacted>");
        register_secret(&out.username, "acct");
        Ok(out)
    }

    /// Serials and credentials never go to the log: only the masked serial and the
    /// broker host are safe to print.
    pub fn describe(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub fn parse_device_list(data: &Value) -> Vec<DiscoveredDevice> {
    let arr = data.as_array().cloned().unwrap_or_default();
    arr.iter()
        .filter_map(|d| {
            let sn = d.get("sn")?.as_str()?.to_string();
            // The serial is registered so the log writer masks it: the device list
            // is where the app first learns the serials, and debug lines name them.
            register_serial(&sn);
            // Verified live: `deviceName` is the user's own nickname, and for a device
            // that was never renamed it is the serial itself. The serial must not reach
            // the UI unmasked, so a name equal to the serial is dropped in favour of
            // `productName`, and if that is the serial too, the name is masked.
            let named = |v: Option<&Value>| {
                v.and_then(|x| x.as_str())
                    .filter(|n| *n != sn)
                    .map(|n| n.to_string())
            };
            let name = named(d.get("deviceName"))
                .or_else(|| named(d.get("productName")))
                .unwrap_or_else(|| mask_serial(&sn));
            Some(DiscoveredDevice {
                sn,
                product_name: name,
                online: as_bool(d.get("online")),
                product_type: d
                    .get("productType")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u16),
            })
        })
        .collect()
}

/// `online` is reported as a number in the live payload, as a bool in some payloads,
/// and as a string in the mobile app API.
fn as_bool(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_u64().map(|v| v != 0).unwrap_or(false),
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true" | "on" | "online"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_device_list_shape_is_parsed() {
        // Captured from the live account: the first entry has no deviceName, and the
        // second has a deviceName that is the serial itself.
        let data = json!([
            { "sn": "R351ZEB5PGC90303", "online": 1, "productName": "DELTA 2 Max" },
            { "sn": "RB2MN5ZE7721004", "deviceName": "RB2MN5ZE7721004", "online": 0, "productName": "RIVER mini" }
        ]);
        let list = parse_device_list(&data);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].sn, "R351ZEB5PGC90303");
        assert_eq!(list[0].product_name, "DELTA 2 Max");
        assert!(list[0].online);
        assert!(!list[1].online);
        // The serial never appears as the model name.
        assert_eq!(list[1].product_name, "RIVER mini");
        assert!(!list[1].product_name.contains("RB2MN5ZE7721004"));
    }

    #[test]
    fn a_user_nickname_wins_over_the_product_name() {
        let data = json!([{ "sn": "AAA1", "deviceName": "Studio battery", "productName": "DELTA 2 Max", "online": 1 }]);
        let list = parse_device_list(&data);
        assert_eq!(list[0].product_name, "Studio battery");
    }

    #[test]
    fn a_name_that_is_the_serial_is_masked() {
        let data =
            json!([{ "sn": "R351ZEB5PGC90303", "deviceName": "R351ZEB5PGC90303", "online": 1 }]);
        let list = parse_device_list(&data);
        assert_eq!(list[0].product_name, "R3************03");
        assert!(!list[0].product_name.contains("ZEB5PGC90303"));
    }

    #[test]
    fn list_entries_without_serial_are_skipped() {
        let data = json!([{ "online": 1 }, { "sn": "X", "online": "1" }]);
        let list = parse_device_list(&data);
        assert_eq!(list.len(), 1);
        assert!(list[0].online, "string online is parsed");
    }

    #[test]
    fn certification_is_parsed_and_described_without_secrets() {
        let v = json!({
            "certificateAccount": "account-123",
            "certificatePassword": "secret-456",
            "url": "mqtt-e.ecoflow.com",
            "port": "8883",
            "protocol": "mqtts"
        });
        let c = MqttCredentials::from_value(&v).expect("credentials");
        assert_eq!(c.host, "mqtt-e.ecoflow.com");
        assert_eq!(c.port, 8883);
        assert!(c.tls);
        // The describe string is what goes to logs; it must not contain either secret.
        let d = c.describe();
        assert_eq!(d, "mqtt-e.ecoflow.com:8883");
        assert!(!d.contains("account-123"));
        assert!(!d.contains("secret-456"));
    }

    #[test]
    fn certification_without_credentials_is_an_error() {
        let e = MqttCredentials::from_value(&json!({ "url": "mqtt-e.ecoflow.com" }))
            .expect_err("error");
        assert!(matches!(e, ApiError::Malformed(_)));
    }

    #[test]
    fn bool_and_zero_online_values_are_parsed() {
        assert!(as_bool(Some(&json!(true))));
        assert!(!as_bool(Some(&json!(0))));
        assert!(!as_bool(None));
    }
}
