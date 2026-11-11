use crate::api::signing;
use crate::config::credentials::Credentials;
use crate::config::settings::ApiConfig;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("api error code {code}: {message}")]
    Api { code: String, message: String },
    #[error("no data in response (code {code})")]
    NoData { code: String },
    #[error("malformed response: {0}")]
    Malformed(String),
}

/// Envelope every signed endpoint returns: `{"code": "0", "data": ...}`.
#[derive(serde::Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    code: String,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    message: Option<String>,
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    access_key: String,
    secret_key: String,
    attempts: u32,
}

impl Client {
    pub fn new(api: &ApiConfig, creds: &Credentials) -> Self {
        let timeout = Duration::from_secs(api.timeout_seconds.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(timeout.as_secs().min(5)))
            .user_agent(concat!("wattdeck/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: api.effective_base_url(),
            access_key: creds.access_key.clone(),
            secret_key: creds.secret_key.clone(),
            attempts: 3,
        }
    }

    /// Signed GET. The query parameters are part of the signature, in sorted order
    /// before the auth fields, which is the order the live API accepts. The URL is
    /// built from the same sorted map, so the signed string and the request match.
    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, ApiError> {
        let nonce = signing::make_nonce();
        let ts = signing::now_millis();
        // The query has to be signed in the same order as it is sent, so the map is
        // the sorted source for both the URL and the signature.
        let mut q: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in query {
            q.insert(k.to_string(), v.clone());
        }
        let sign = signing::sign_get(&self.secret_key, &self.access_key, &nonce, &ts, &q);
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.get(&url);
        if !q.is_empty() {
            req = req.query(&q);
        }
        req = self.signed_headers(req, &nonce, &ts, &sign);
        self.send(req).await
    }

    /// POST, signed over the flattened body plus the auth params.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let nonce = signing::make_nonce();
        let ts = signing::now_millis();
        let flat = signing::flatten_body(body);
        let sign = signing::sign_post(&self.secret_key, &self.access_key, &nonce, &ts, &flat);
        let url = format!("{}{}", self.base, path);
        let req = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .json(body);
        let req = self.signed_headers(req, &nonce, &ts, &sign);
        self.send(req).await
    }

    fn signed_headers(
        &self,
        req: reqwest::RequestBuilder,
        nonce: &str,
        ts: &str,
        sign: &str,
    ) -> reqwest::RequestBuilder {
        req.header("accessKey", &self.access_key)
            .header("nonce", nonce)
            .header("timestamp", ts)
            .header("sign", sign)
    }

    /// One request with bounded retries and exponential backoff. 429 and 5xx are
    /// retried; 4xx (apart from 429) is returned immediately.
    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let mut attempt = 0;
        let mut last = ApiError::Malformed("no attempt made".into());
        while attempt < self.attempts {
            attempt += 1;
            let req = req
                .try_clone()
                .ok_or_else(|| ApiError::Malformed("request not clonable".into()))?;
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last = ApiError::Network(e);
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
            };
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                last = ApiError::Malformed(format!("http {}", status.as_u16()));
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
            if !status.is_success() {
                return Err(ApiError::Malformed(format!("http {}", status.as_u16())));
            }
            let json: Value = resp.json().await.map_err(ApiError::Network)?;
            return decode_envelope(json);
        }
        Err(last)
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(200u64 << (attempt - 1).min(3))
}

/// Turns the envelope into the data payload, mapping a non-zero `code` to an error.
pub fn decode_envelope(json: Value) -> Result<Value, ApiError> {
    let env: Envelope<Value> =
        serde_json::from_value(json).map_err(|e| ApiError::Malformed(e.to_string()))?;
    if env.code == "0" {
        if let Some(d) = env.data {
            return Ok(d);
        }
        return Err(ApiError::NoData { code: env.code });
    }
    Err(ApiError::Api {
        code: env.code,
        message: env.message.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_envelope_yields_data() {
        let v = decode_envelope(json!({ "code": "0", "data": [{"sn": "X", "online": 1}] }))
            .expect("data");
        assert_eq!(v[0]["sn"], "X");
    }

    #[test]
    fn error_envelope_maps_the_code() {
        let e = decode_envelope(json!({ "code": "8521", "message": "signature is wrong" }))
            .expect_err("error");
        assert!(matches!(e, ApiError::Api { code: ref c, .. } if c == "8521"));
        assert!(e.to_string().contains("signature is wrong"));
    }

    #[test]
    fn zero_data_is_not_a_success_with_no_payload() {
        let e = decode_envelope(json!({ "code": "0" })).expect_err("error");
        assert!(matches!(e, ApiError::NoData { .. }));
    }

    #[test]
    fn backoff_grows() {
        assert!(backoff(1) < backoff(2));
        assert!(backoff(2) < backoff(3));
        assert!(backoff(4) <= Duration::from_millis(1600));
    }
}
