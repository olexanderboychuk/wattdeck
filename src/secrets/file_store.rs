//! The plaintext-file backend: `credentials.toml` at mode 600.
//!
//! This is the fallback path, used when no secret service answers. Writing keys
//! here through the wizard needs explicit consent; a file that already exists is
//! reported and never modified by the app.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::config::credentials::Credentials;
use crate::secrets::SecretStore;

#[derive(Deserialize, Serialize)]
struct CredentialFile {
    credentials: Credentials,
}

pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default location: `config_dir()/credentials.toml`.
    pub fn default_store() -> Result<Self> {
        Ok(Self::new(config_dir()?.join("credentials.toml")))
    }

    pub fn path_of(&self) -> &Path {
        &self.path
    }
}

impl SecretStore for FileStore {
    fn backend(&self) -> &'static str {
        "plaintext"
    }

    fn describe(&self) -> String {
        format!(
            "plaintext {} ({})",
            self.path.display(),
            protection(&self.path)
        )
    }

    fn load(&self) -> Result<Option<Credentials>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&self.path)
            .with_context(|| format!("reading credentials {}", self.path.display()))?;
        let parsed: CredentialFile = toml::from_str(&text)
            .with_context(|| format!("parsing credentials {}", self.path.display()))?;
        if parsed.credentials.is_empty() {
            return Ok(None);
        }
        check_permissions(&self.path);
        Ok(Some(parsed.credentials))
    }

    /// Keys go through the TOML serializer, so they are escaped by it and never
    /// pasted into a format string.
    fn save(&self, creds: &Credentials) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = CredentialFile {
            credentials: creds.clone(),
        };
        std::fs::write(&self.path, toml::to_string_pretty(&file)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

/// Warn (never print the contents) when the credential file is group/world readable.
#[cfg(unix)]
fn check_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777);
    if let Ok(mode) = mode
        && mode & 0o077 != 0
    {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{mode:o}"),
            "credential file is group/world readable; run `chmod 600` on it"
        );
    }
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path) {}

/// How the plaintext file is protected, in words the platform can truthfully
/// use. Unix reports the real mode; Windows has no unix permissions, so the
/// honest words are the ones that name the actual protection. The functions are
/// per-platform, not one function with unused branches: a single signature
/// takes `path` for the unix report and leaves it unused on Windows, which
/// clippy `-D warnings` fails on the Windows runner (run 43064495565 printed
/// `error: unused variable: path` at this line).
#[cfg(unix)]
pub fn protection(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
        .unwrap_or_else(|_| "?".to_string())
}

#[cfg(windows)]
pub fn protection(_path: &Path) -> String {
    "protected by your Windows user profile".to_string()
}

#[cfg(not(any(unix, windows)))]
pub fn protection(_path: &Path) -> String {
    "no unix permissions to report".to_string()
}

/// The same words, for a file the app has not opened (the wizard prompt, the
/// alert, the `--init` print), where the mode the app is about to create is known.
pub fn protection_text() -> String {
    #[cfg(unix)]
    {
        "mode 600".to_string()
    }
    #[cfg(windows)]
    {
        "protected by your Windows user profile".to_string()
    }
    #[cfg(not(any(unix, windows)))]
    {
        "no unix permissions to report".to_string()
    }
}
