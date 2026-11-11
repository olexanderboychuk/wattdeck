//! `wattdeck setup`: ask for the keys, store them, verify, show what they see.
//!
//! The prompt and the API check are traits, so the wizard is unit-tested with a
//! scripted prompt and a scripted check — no terminal, no network, no keyring.
//! The keys are read with echo off, and the wizard never prints them.

use std::io::IsTerminal;

use anyhow::{Result, bail};

use crate::api::client::Client;
use crate::api::devices::device_list;
use crate::config::Config;
use crate::config::credentials::Credentials;
use crate::secrets::SecretStore;
use crate::secrets::file_store::protection_text;
use crate::utils::formatting::mask_serial;

pub trait Prompt {
    fn read_line(&mut self, label: &str) -> Result<String>;
    fn read_secret(&mut self, label: &str) -> Result<String>;
    fn confirm(&mut self, label: &str) -> Result<bool>;
    fn print(&mut self, line: &str) -> Result<()>;
}

pub struct TerminalPrompt;

impl Prompt for TerminalPrompt {
    fn read_line(&mut self, label: &str) -> Result<String> {
        print!("{label}");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }

    /// Echo off. `rpassword::read_password` reads from the TTY (verified in the
    /// crate source), so a piped stdin is not a source — the TTY gate in `run`
    /// is what keeps the wizard from hanging on a pipe.
    fn read_secret(&mut self, label: &str) -> Result<String> {
        print!("{label}");
        std::io::Write::flush(&mut std::io::stdout())?;
        Ok(rpassword::read_password()?.trim().to_string())
    }

    fn confirm(&mut self, label: &str) -> Result<bool> {
        let answer = self.read_line(label)?;
        Ok(matches!(answer.as_str(), "y" | "Y" | "yes" | "Yes"))
    }

    fn print(&mut self, line: &str) -> Result<()> {
        println!("{line}");
        Ok(())
    }
}

/// The one validation the wizard does: the device list the keys can see. The
/// error is what the API said, as it arrived.
pub trait DeviceCheck {
    fn check(&self, creds: &Credentials) -> std::result::Result<Vec<String>, String>;
}

pub struct LiveCheck {
    cfg: Config,
}

impl LiveCheck {
    pub fn new(cfg: &Config) -> Self {
        Self { cfg: cfg.clone() }
    }
}

impl DeviceCheck for LiveCheck {
    fn check(&self, creds: &Credentials) -> std::result::Result<Vec<String>, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(async {
            let client = Client::new(&self.cfg.api, creds);
            match device_list(&client).await {
                Ok(devices) => Ok(devices
                    .iter()
                    .map(|d| {
                        format!(
                            "{} {} {}",
                            mask_serial(&d.sn),
                            d.product_name,
                            if d.online { "online" } else { "offline" }
                        )
                    })
                    .collect()),
                Err(e) => Err(format!("{e}")),
            }
        })
    }
}

#[derive(Debug)]
pub struct Report {
    pub backend: &'static str,
    pub wrote: bool,
    pub lines: Vec<String>,
}

/// The wizard, with the TTY gate passed in so it can be tested without a terminal.
pub fn run_with(
    tty: bool,
    store: &dyn SecretStore,
    prompt: &mut dyn Prompt,
    check: &dyn DeviceCheck,
) -> Result<Report> {
    if !tty {
        bail!("interactive setup needs a terminal; set ECOFLOW_ACCESS_KEY / ECOFLOW_SECRET_KEY");
    }
    let access = prompt.read_secret("Access key: ")?;
    if access.is_empty() {
        bail!("the wizard needs an access key");
    }
    let secret = prompt.read_secret("Secret key: ")?;
    if secret.is_empty() {
        bail!("the wizard needs a secret key");
    }
    let creds = Credentials::new(access, secret);

    // Keys at rest in a plaintext file are a labelled fallback, and writing them
    // needs an explicit yes.
    if store.backend() == "plaintext" {
        let yes = prompt.confirm(&format!(
            "no secret service on this machine. Store the keys in the plaintext file, {}? [y/N] {}",
            protection_text(),
            store.describe()
        ))?;
        if !yes {
            prompt.print(
                "not stored. Set ECOFLOW_ACCESS_KEY / ECOFLOW_SECRET_KEY, \
                 or run the wizard on a machine with a secret service.",
            )?;
            return Ok(Report {
                backend: store.backend(),
                wrote: false,
                lines: Vec::new(),
            });
        }
    }

    store.save(&creds)?;
    prompt.print(&format!("stored in the {} backend", store.backend()))?;
    if store.backend() == "keyring" {
        // Say what was not written: no migration, no silent import.
        prompt.print("did not write the plaintext credentials file")?;
    }
    let wrote = true;

    match check.check(&creds) {
        Ok(lines) => {
            prompt.print(&format!("API: {} devices", lines.len()))?;
            for line in &lines {
                prompt.print(&format!("  {line}"))?;
            }
            Ok(Report {
                backend: store.backend(),
                wrote,
                lines: lines.clone(),
            })
        }
        Err(e) => {
            // The refusal is printed as it arrived, never reworded.
            prompt.print(&format!("API refused: {e}"))?;
            Ok(Report {
                backend: store.backend(),
                wrote,
                lines: Vec::new(),
            })
        }
    }
}

/// The entry point: the TTY gate reads the real stdin.
pub fn run(
    store: &dyn SecretStore,
    prompt: &mut dyn Prompt,
    check: &dyn DeviceCheck,
) -> Result<Report> {
    run_with(std::io::stdin().is_terminal(), store, prompt, check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::credentials::Credentials;
    use crate::secrets::MemoryStore;
    use crate::secrets::SecretStore;
    use crate::secrets::file_store::FileStore;

    /// A scripted prompt: answers come from a queue, and every printed line is
    /// recorded, so the test can assert the keys never appear on screen.
    struct ScriptedPrompt {
        answers: std::collections::VecDeque<String>,
        transcript: Vec<String>,
    }

    impl ScriptedPrompt {
        fn new(answers: &[&str]) -> Self {
            Self {
                answers: answers.iter().map(|s| s.to_string()).collect(),
                transcript: Vec::new(),
            }
        }
    }

    impl Prompt for ScriptedPrompt {
        fn read_line(&mut self, label: &str) -> Result<String> {
            self.transcript.push(label.to_string());
            Ok(self.answers.pop_front().unwrap_or_default())
        }
        fn read_secret(&mut self, label: &str) -> Result<String> {
            self.transcript.push(label.to_string());
            Ok(self.answers.pop_front().unwrap_or_default())
        }
        fn confirm(&mut self, label: &str) -> Result<bool> {
            let a = self.read_line(label)?;
            Ok(matches!(a.as_str(), "y" | "Y"))
        }
        fn print(&mut self, line: &str) -> Result<()> {
            self.transcript.push(line.to_string());
            Ok(())
        }
    }

    struct ScriptedCheck(std::result::Result<Vec<String>, String>);
    impl DeviceCheck for ScriptedCheck {
        fn check(&self, _creds: &Credentials) -> std::result::Result<Vec<String>, String> {
            self.0.clone()
        }
    }

    /// A store that reports the keyring backend, so the wizard's naming of the
    /// file it did not write is testable without D-Bus.
    struct KeyringLike(MemoryStore);
    impl SecretStore for KeyringLike {
        fn load(&self) -> Result<Option<Credentials>> {
            self.0.load()
        }
        fn save(&self, c: &Credentials) -> Result<()> {
            self.0.save(c)
        }
        fn backend(&self) -> &'static str {
            "keyring"
        }
        fn describe(&self) -> String {
            "keyring (service wattdeck)".into()
        }
    }

    #[test]
    fn keys_are_stored_and_never_echoed() {
        let store = MemoryStore::default();
        let mut prompt =
            ScriptedPrompt::new(&["FAKE-ACCESS-KEY-1a2b3c4d", "FAKE-SECRET-KEY-9x8y7z6w"]);
        let check = ScriptedCheck(Ok(vec!["R3************44 DELTA 2 Max online".into()]));
        let report = run_with(true, &store, &mut prompt, &check).unwrap();
        assert_eq!(report.backend, "memory");
        assert!(report.wrote);
        assert_eq!(report.lines.len(), 1);
        let text = prompt.transcript.join("\n");
        assert!(
            !text.contains("FAKE-ACCESS-KEY-1a2b3c4d"),
            "the access key was echoed: {text}"
        );
        assert!(
            !text.contains("FAKE-SECRET-KEY-9x8y7z6w"),
            "the secret key was echoed: {text}"
        );
        assert!(text.contains("stored in the memory backend"), "{text}");
        assert!(
            text.contains("R3************44 DELTA 2 Max online"),
            "{text}"
        );
        assert_eq!(
            store.load().unwrap().unwrap().secret_key,
            "FAKE-SECRET-KEY-9x8y7z6w"
        );
    }

    #[test]
    fn plaintext_write_needs_consent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("credentials.toml"));
        let mut prompt = ScriptedPrompt::new(&["KEY", "SECRET", "n"]);
        let check = ScriptedCheck(Ok(vec![]));
        let report = run_with(true, &store, &mut prompt, &check).unwrap();
        assert!(!report.wrote, "refused consent must not write a file");
        assert!(!dir.path().join("credentials.toml").exists());
    }

    #[test]
    fn consent_writes_mode_600() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("credentials.toml"));
        let mut prompt = ScriptedPrompt::new(&["KEY", "SECRET", "y"]);
        let check = ScriptedCheck(Ok(vec![]));
        let report = run_with(true, &store, &mut prompt, &check).unwrap();
        assert!(report.wrote);
        let file = dir.path().join("credentials.toml");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        #[cfg(windows)]
        {
            // Windows has no unix mode: the file is inside the user profile, and
            // that is the protection the app can name.
            use crate::secrets::file_store::protection;
            assert_eq!(protection(&file), "protected by your Windows user profile");
        }
        assert_eq!(store.load().unwrap().unwrap().access_key, "KEY");
    }

    #[test]
    fn an_empty_key_stops_the_wizard() {
        let store = MemoryStore::default();
        let mut prompt = ScriptedPrompt::new(&[""]);
        let check = ScriptedCheck(Ok(vec![]));
        assert!(run_with(true, &store, &mut prompt, &check).is_err());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn an_api_refusal_is_printed_as_it_arrived() {
        let store = MemoryStore::default();
        let mut prompt = ScriptedPrompt::new(&["KEY", "SECRET"]);
        let check = ScriptedCheck(Err("code 1006: current device is not allowed".into()));
        let report = run_with(true, &store, &mut prompt, &check).unwrap();
        let text = prompt.transcript.join("\n");
        assert!(
            text.contains("API refused: code 1006: current device is not allowed"),
            "{text}"
        );
        // The keys were stored before the check ran; only the device list failed.
        assert!(report.wrote);
        assert!(report.lines.is_empty());
    }

    #[test]
    fn no_terminal_bails() {
        let store = MemoryStore::default();
        let mut prompt = ScriptedPrompt::new(&[]);
        let check = ScriptedCheck(Ok(vec![]));
        let err = run_with(false, &store, &mut prompt, &check)
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a terminal"), "{err}");
        assert!(err.contains("ECOFLOW_ACCESS_KEY"), "{err}");
    }

    #[test]
    fn the_keyring_backend_names_the_file_it_did_not_write() {
        let store = KeyringLike(MemoryStore::default());
        let mut prompt = ScriptedPrompt::new(&["KEY", "SECRET"]);
        let check = ScriptedCheck(Ok(vec![]));
        run_with(true, &store, &mut prompt, &check).unwrap();
        let text = prompt.transcript.join("\n");
        assert!(text.contains("stored in the keyring backend"), "{text}");
        assert!(
            text.contains("did not write the plaintext credentials file"),
            "{text}"
        );
    }
}
