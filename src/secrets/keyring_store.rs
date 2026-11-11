//! Keys in the freedesktop secret service, through the `keyring` crate.
//!
//! The crate's API was verified on 2026-11-11 by a throwaway probe:
//! `Entry::new(service, user)`, `set_password`, `get_password`,
//! `delete_credential`, and an absent entry is `Err(Error::NoEntry)`. The
//! roundtrip worked against the KDE secret service with no unlock prompt (the
//! wallet was unlocked). `keyring::Error` is `#[non_exhaustive]`, so `NoEntry`
//! is matched, never constructed — which is why `KeyringOps::get` returns
//! `Option<String>` and the crate's error type stays inside `LiveOps`.
//!
//! **Not verified:** the error shape on a machine with no user bus, and what a
//! locked wallet does. Both are labelled in the spec and reported by Task 7.

use anyhow::{Context, Result};
use keyring::{Entry, Error};

use crate::config::credentials::Credentials;
use crate::secrets::{KeyringProbe, ProbeOutcome, SecretStore};

/// The app's own name is the service name; the vendor name does not belong in it.
const SERVICE: &str = "wattdeck";
const ACCESS: &str = "access_key";
const SECRET: &str = "secret_key";

/// The seam between the store and the secret service. `get` yields `None` when
/// the entry is absent, which is a normal state, not a failure.
pub trait KeyringOps {
    fn set(&self, user: &str, value: &str) -> Result<()>;
    fn get(&self, user: &str) -> Result<Option<String>>;
    fn delete(&self, user: &str) -> Result<()>;
}

/// The real secret service.
pub struct LiveOps;

impl KeyringOps for LiveOps {
    fn set(&self, user: &str, value: &str) -> Result<()> {
        Entry::new(SERVICE, user)
            .with_context(|| format!("opening the keyring entry for {user}"))?
            .set_password(value)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context(format!(
                "storing the keyring entry for {user} — unlock the wallet and re-run"
            ))
    }

    fn get(&self, user: &str) -> Result<Option<String>> {
        let entry = Entry::new(SERVICE, user)
            .with_context(|| format!("opening the keyring entry for {user}"))?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            // The entry is not there. A service that answers and has no entry is
            // a working keyring, not a failure.
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("{e}")).context(format!(
                "reading the keyring entry for {user} — unlock the wallet and re-run"
            )),
        }
    }

    fn delete(&self, user: &str) -> Result<()> {
        Entry::new(SERVICE, user)
            .with_context(|| format!("opening the keyring entry for {user}"))?
            .delete_credential()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context(format!("deleting the keyring entry for {user}"))
    }
}

pub struct KeyringStore {
    ops: Box<dyn KeyringOps>,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self {
            ops: Box::new(LiveOps),
        }
    }
    pub fn with_ops(ops: Box<dyn KeyringOps>) -> Self {
        Self { ops }
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringStore {
    fn backend(&self) -> &'static str {
        "keyring"
    }

    fn describe(&self) -> String {
        format!("keyring (service {SERVICE})")
    }

    fn load(&self) -> Result<Option<Credentials>> {
        // Both entries or neither. Half a pair is not a credential.
        match (self.ops.get(ACCESS)?, self.ops.get(SECRET)?) {
            (Some(a), Some(s)) => Ok(Some(Credentials::new(a, s))),
            (None, _) | (_, None) => Ok(None),
        }
    }

    /// `save` reads both entries back. A roundtrip that does not match what was
    /// written is a failure, not a success.
    fn save(&self, creds: &Credentials) -> Result<()> {
        self.ops.set(ACCESS, &creds.access_key)?;
        self.ops.set(SECRET, &creds.secret_key)?;
        match self.load()? {
            Some(c) if c.access_key == creds.access_key && c.secret_key == creds.secret_key => {
                Ok(())
            }
            _ => anyhow::bail!("the keyring roundtrip did not match what was written"),
        }
    }
}

/// What the keyring says about this machine: an entry present, an entry cleanly
/// absent, or no service at all.
pub struct LiveProbe;

impl KeyringProbe for LiveProbe {
    fn probe(&self) -> ProbeOutcome {
        match LiveOps.get(ACCESS) {
            Ok(Some(_)) => ProbeOutcome::Found,
            Ok(None) => ProbeOutcome::Absent,
            Err(e) => ProbeOutcome::Unavailable(e.to_string()),
        }
    }
}

/// The live roundtrip, run explicitly: `cargo test --lib -- --ignored`. It
/// writes a dummy value to the real keyring and deletes it again.
#[test]
#[ignore = "live probe: writes a dummy value to the real keyring and deletes it"]
fn keyring_roundtrip_on_this_machine() {
    let ops = LiveOps;
    ops.set("probe", "dummy-value").unwrap();
    assert_eq!(ops.get("probe").unwrap().unwrap(), "dummy-value");
    ops.delete("probe").unwrap();
    assert!(ops.get("probe").unwrap().is_none());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::credentials::Credentials;
    use crate::secrets::ProbeOutcome;
    use crate::secrets::file_store::FileStore;
    use crate::secrets::open_with;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A scripted keyring: a map stands in for the secret service, so the
    /// roundtrip and the selection rule are tested without D-Bus.
    #[derive(Default)]
    struct ScriptedOps(RefCell<HashMap<String, String>>);

    impl KeyringOps for ScriptedOps {
        fn set(&self, user: &str, value: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(user.to_string(), value.to_string());
            Ok(())
        }
        fn get(&self, user: &str) -> Result<Option<String>> {
            Ok(self.0.borrow().get(user).cloned())
        }
        fn delete(&self, user: &str) -> Result<()> {
            self.0.borrow_mut().remove(user);
            Ok(())
        }
    }

    struct Scripted(ProbeOutcome);
    impl KeyringProbe for Scripted {
        fn probe(&self) -> ProbeOutcome {
            // The payload cannot be moved out of a borrowed reference, so the
            // reason is cloned.
            match &self.0 {
                ProbeOutcome::Found => ProbeOutcome::Found,
                ProbeOutcome::Absent => ProbeOutcome::Absent,
                ProbeOutcome::Unavailable(r) => ProbeOutcome::Unavailable(r.clone()),
            }
        }
    }

    #[test]
    fn keyring_roundtrip_after_save() {
        let store = KeyringStore::with_ops(Box::new(ScriptedOps::default()));
        store.save(&Credentials::new("KEY", "SECRET")).unwrap();
        let back = store.load().unwrap().unwrap();
        assert_eq!(
            (back.access_key.as_str(), back.secret_key.as_str()),
            ("KEY", "SECRET")
        );
        assert_eq!(store.backend(), "keyring");
    }

    #[test]
    fn absent_entries_load_as_none() {
        let store = KeyringStore::with_ops(Box::new(ScriptedOps::default()));
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn half_a_pair_is_not_a_credential() {
        let ops = ScriptedOps::default();
        ops.0
            .borrow_mut()
            .insert("access_key".to_string(), "K".to_string());
        let store = KeyringStore::with_ops(Box::new(ops));
        assert!(store.load().unwrap().is_none());
    }

    /// `save` reads both entries back. A store that accepts the write and then
    /// reports nothing is a failure, not a success.
    #[test]
    fn a_write_that_does_not_read_back_is_a_failure() {
        struct Broken;
        impl KeyringOps for Broken {
            fn set(&self, _u: &str, _v: &str) -> Result<()> {
                Ok(())
            }
            fn get(&self, _u: &str) -> Result<Option<String>> {
                Ok(None)
            }
            fn delete(&self, _u: &str) -> Result<()> {
                Ok(())
            }
        }
        let store = KeyringStore::with_ops(Box::new(Broken));
        assert!(store.save(&Credentials::new("KEY", "SECRET")).is_err());
    }

    #[test]
    fn probe_found_and_absent_pick_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path().join("credentials.toml"));
        let store = open_with(&Scripted(ProbeOutcome::Found), file);
        assert_eq!(store.backend(), "keyring");
        let file = FileStore::new(dir.path().join("credentials.toml"));
        let store = open_with(&Scripted(ProbeOutcome::Absent), file);
        assert_eq!(store.backend(), "keyring");
    }

    #[test]
    fn no_service_falls_back_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path().join("credentials.toml"));
        let store = open_with(
            &Scripted(ProbeOutcome::Unavailable("no session bus".into())),
            file,
        );
        assert_eq!(store.backend(), "plaintext");
        let described = store.describe();
        assert!(described.starts_with("plaintext"), "{described}");
        assert!(
            described.contains("credentials.toml"),
            "the path is named: {described}"
        );
    }

    #[test]
    fn the_service_name_is_the_app() {
        assert_eq!(SERVICE, "wattdeck");
    }
}
