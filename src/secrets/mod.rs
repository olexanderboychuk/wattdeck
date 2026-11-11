//! Where the API keys come from and where they are stored.
//!
//! The store is chosen at startup: a secret service if the machine has one, the
//! plaintext file (mode 600) if it does not. Keys are never printed.

use anyhow::Result;

use crate::config::credentials::Credentials;
use crate::secrets::file_store::{FileStore, protection, protection_text};
use crate::secrets::keyring_store::{KeyringStore, LiveProbe};
use crate::services::Notification;

pub mod file_store;
pub mod keyring_store;
pub mod wizard;

/// The keys' home. `load` yields `None` when this backend has no keys.
pub trait SecretStore {
    fn load(&self) -> Result<Option<Credentials>>;
    fn save(&self, creds: &Credentials) -> Result<()>;
    /// `"keyring"`, `"plaintext"`, or `"memory"` in tests.
    fn backend(&self) -> &'static str;
    /// What the `credentials:` line in `wattdeck status` prints.
    fn describe(&self) -> String;
}

/// What a probe of the keyring found.
pub enum ProbeOutcome {
    /// An entry is present.
    Found,
    /// A secret service answers, and the entry is not there.
    Absent,
    /// No secret service: the reason is the message to report.
    Unavailable(String),
}

pub trait KeyringProbe {
    fn probe(&self) -> ProbeOutcome;
}

pub trait EnvLookup {
    fn get(&self, key: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Test store: keys live in memory, no D-Bus, no file.
#[derive(Default)]
pub struct MemoryStore {
    creds: std::cell::RefCell<Option<Credentials>>,
}

impl MemoryStore {
    pub fn new(creds: Credentials) -> Self {
        Self {
            creds: std::cell::RefCell::new(Some(creds)),
        }
    }
}

impl SecretStore for MemoryStore {
    fn load(&self) -> Result<Option<Credentials>> {
        Ok(self.creds.borrow().clone())
    }
    fn save(&self, creds: &Credentials) -> Result<()> {
        self.creds.replace(Some(creds.clone()));
        Ok(())
    }
    fn backend(&self) -> &'static str {
        "memory"
    }
    fn describe(&self) -> String {
        "memory".to_string()
    }
}

/// Keys in the environment take precedence over the store: the scripted path.
pub fn load_with(store: &dyn SecretStore, env: &dyn EnvLookup) -> Result<Option<Credentials>> {
    if let (Some(k), Some(s)) = (env.get("ECOFLOW_ACCESS_KEY"), env.get("ECOFLOW_SECRET_KEY")) {
        return Ok(Some(Credentials::new(k, s)));
    }
    store.load()
}

/// Where the keys for this run come from, for `wattdeck status` and the TUI alert.
pub fn source(store: &dyn SecretStore, env: &dyn EnvLookup) -> String {
    if env.get("ECOFLOW_ACCESS_KEY").is_some() && env.get("ECOFLOW_SECRET_KEY").is_some() {
        return "env".to_string();
    }
    store.describe()
}

/// One alert when the keys sit in a plaintext file. The header gains no
/// credential text; the Alerts screen is where state the app wants seen lands.
pub fn backend_alert(source: &str) -> Option<Notification> {
    if source.starts_with("plaintext") {
        Some(Notification::warn(format!(
            "keys are in a plaintext file ({}) — no secret service on this machine",
            protection_text()
        )))
    } else {
        None
    }
}

/// The mock-mode hint, one notice on the Alerts screen. The `MOCK` badge in the
/// header is the marker; this line is the way out of mock mode.
pub fn mock_hint(use_mock: bool) -> Option<Notification> {
    if use_mock {
        Some(Notification::info(
            "no credentials — mock mode. Run: wattdeck setup",
        ))
    } else {
        None
    }
}

/// The keyring is the backend, and a plaintext file with keys also exists: say
/// so, and touch nothing. No migration, no silent import.
pub fn report_alongside(backend: &str, file: &FileStore) -> Option<String> {
    if backend != "keyring" {
        return None;
    }
    match file.load() {
        Ok(Some(_)) => Some(format!(
            "plaintext file present at {} ({}); reading the keyring, not it",
            file.path_of().display(),
            protection(file.path_of())
        )),
        _ => None,
    }
}

/// The store for this machine: the keyring when a secret service answers, the
/// plaintext file when it does not. An entry that is cleanly absent means the
/// keyring works and has no keys yet — not a reason to read a plaintext file.
pub fn open_with(probe: &dyn KeyringProbe, file: file_store::FileStore) -> Box<dyn SecretStore> {
    match probe.probe() {
        ProbeOutcome::Found | ProbeOutcome::Absent => Box::new(KeyringStore::new()),
        ProbeOutcome::Unavailable(reason) => {
            tracing::warn!("no secret service ({reason}); using the plaintext file");
            Box::new(file)
        }
    }
}

pub fn open() -> Result<Box<dyn SecretStore>> {
    Ok(open_with(
        &LiveProbe,
        file_store::FileStore::default_store()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::credentials::Credentials;
    use crate::secrets::file_store::FileStore;
    use crate::services::Level;
    use std::collections::HashMap;

    struct MapEnv(HashMap<String, String>);
    impl EnvLookup for MapEnv {
        fn get(&self, k: &str) -> Option<String> {
            self.0.get(k).cloned()
        }
    }

    #[test]
    fn env_vars_beat_the_store() {
        let store = MemoryStore::default();
        let env = MapEnv(
            [
                ("ECOFLOW_ACCESS_KEY".into(), "AKEY".into()),
                ("ECOFLOW_SECRET_KEY".into(), "SECRET".into()),
            ]
            .into(),
        );
        let creds = load_with(&store, &env).unwrap();
        assert_eq!(creds.unwrap().access_key, "AKEY");
    }

    #[test]
    fn store_is_used_when_env_is_absent() {
        let store = MemoryStore::new(Credentials::new("KEY", "SECRET"));
        let creds = load_with(&store, &MapEnv(HashMap::new())).unwrap();
        assert_eq!(creds.unwrap().secret_key, "SECRET");
    }

    #[test]
    fn source_names_the_backend_for_the_status_line() {
        let store = MemoryStore::default();
        let env = MapEnv(HashMap::new());
        assert_eq!(source(&store, &env), "memory");
        let env = MapEnv(
            [
                ("ECOFLOW_ACCESS_KEY".into(), "A".into()),
                ("ECOFLOW_SECRET_KEY".into(), "B".into()),
            ]
            .into(),
        );
        assert_eq!(source(&store, &env), "env");
    }

    #[test]
    fn plaintext_backend_gets_one_alert_naming_the_protection() {
        let n = backend_alert("plaintext /home/x/credentials.toml (600)").unwrap();
        assert_eq!(n.level, Level::Warn);
        assert!(n.text.contains("plaintext"), "{}", n.text);
        assert!(
            n.text.contains(&protection_text()),
            "alert says {}, protection is {}",
            n.text,
            protection_text()
        );
        assert!(backend_alert("keyring").is_none());
        assert!(backend_alert("env").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_plaintext_is_described_as_a_windows_thing() {
        assert_eq!(protection_text(), "protected by your Windows user profile");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(
            &path,
            "[credentials]\naccess_key = \"\"\nsecret_key = \"\"\n",
        )
        .unwrap();
        assert_eq!(protection(&path), "protected by your Windows user profile");
    }

    #[cfg(unix)]
    #[test]
    fn unix_reports_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(
            &path,
            "[credentials]\naccess_key = \"\"\nsecret_key = \"\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(protection(&path), "600");
        assert_eq!(protection_text(), "mode 600");
    }

    #[test]
    fn file_store_roundtrip_is_mode_600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        let store = FileStore::new(&path);
        store.save(&Credentials::new("KEY", "SECRET")).unwrap();
        let back = store.load().unwrap().unwrap();
        assert_eq!(
            (back.access_key.as_str(), back.secret_key.as_str()),
            ("KEY", "SECRET")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credential file mode");
        }
    }

    #[test]
    fn empty_template_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(
            &path,
            "[credentials]\naccess_key = \"\"\nsecret_key = \"\"\n",
        )
        .unwrap();
        assert!(FileStore::new(&path).load().unwrap().is_none());
        assert!(
            FileStore::new(dir.path().join("missing.toml"))
                .load()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_keyring_backend_reports_a_plaintext_file_it_does_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path().join("credentials.toml"));
        file.save(&Credentials::new("KEY", "SECRET")).unwrap();
        let line = report_alongside("keyring", &file).unwrap();
        assert!(line.starts_with("plaintext file present at"), "{line}");
        assert!(line.contains("reading the keyring, not it"), "{line}");
        assert!(line.contains(&protection(file.path_of())), "{line}");
        assert!(report_alongside("plaintext", &file).is_none());
        assert!(report_alongside("env", &file).is_none());
    }

    #[test]
    fn an_empty_template_is_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path().join("credentials.toml"));
        assert!(report_alongside("keyring", &file).is_none());
    }

    #[test]
    fn the_mock_hint_names_the_wizard() {
        let n = mock_hint(true).unwrap();
        assert_eq!(n.level, Level::Info);
        assert!(n.text.contains("wattdeck setup"), "{}", n.text);
        assert!(mock_hint(false).is_none());
    }
}
