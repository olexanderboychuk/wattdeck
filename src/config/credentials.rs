use serde::{Deserialize, Serialize};

/// API keys. `Debug` is hand-written so keys can never appear in logs, panic
/// messages, or crash dumps.
#[derive(Clone)]
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
}

impl Credentials {
    pub fn new(access_key: impl Into<String>, secret_key: impl Into<String>) -> Self {
        Self {
            access_key: access_key.into(),
            secret_key: secret_key.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.access_key.is_empty() || self.secret_key.is_empty()
    }
}

impl Default for Credentials {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}

impl Serialize for Credentials {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = s.serialize_struct("Credentials", 2)?;
        out.serialize_field("access_key", &self.access_key)?;
        out.serialize_field("secret_key", &self.secret_key)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for Credentials {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            access_key: String,
            #[serde(default)]
            secret_key: String,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Self::new(raw.access_key, raw.secret_key))
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Credentials(access_key: {:?}, secret_key: <redacted>)",
            self.redacted()
        )
    }
}

impl Credentials {
    /// First 4 characters of the access key, for logs and UI labels.
    pub fn redacted(&self) -> String {
        if self.access_key.len() <= 4 {
            return "****".to_string();
        }
        format!("{}****", &self.access_key[..4])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_leaks_the_secret() {
        // Fake keys. `redacted()` keeps only the first 4 characters of the access
        // key (`FAKE****`), and the secret is replaced by `<redacted>`, so the
        // fake secret string must not appear anywhere in the Debug output.
        let creds = Credentials::new("FAKE-ACCESS-KEY-1a2b3c4d", "FAKE-SECRET-KEY-9x8y7z6w");
        let text = format!("{creds:?}");
        assert!(!text.contains("FAKE-SECRET-KEY-9x8y7z6w"));
        assert!(text.contains("FAKE****"));
    }

    #[test]
    fn empty_keys_are_detected() {
        assert!(Credentials::default().is_empty());
        assert!(Credentials::new("key", "").is_empty());
        assert!(!Credentials::new("key", "secret").is_empty());
    }

    #[test]
    fn redact_keeps_only_four_chars() {
        assert_eq!(
            Credentials::new("ABCDEFGHIJKLMNOP", "x").redacted(),
            "ABCD****"
        );
        assert_eq!(Credentials::new("AB", "x").redacted(), "****");
    }

    #[test]
    fn missing_fields_deserialize_to_empty() {
        let creds: Credentials = toml::from_str("[credentials]\nregion = \"EU\"\n").unwrap();
        assert!(creds.is_empty());
    }
}
