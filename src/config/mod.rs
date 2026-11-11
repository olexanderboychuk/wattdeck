pub mod credentials;
pub mod settings;

pub use settings::Config;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The app's own name, which is the directory name under the XDG paths. The vendor
/// name does not belong in it: wattdeck is the app, EcoFlow is the vendor of the
/// devices. The installed binary and the log file use the same name.
pub const APP_DIR: &str = "wattdeck";

/// Where the app keeps its configuration. On Linux `XDG_CONFIG_HOME` is
/// respected, else `~/.config`; on macOS `~/Library/Application Support`; on
/// Windows `%APPDATA%`. The app's own name is the directory under the base.
/// `directories` resolves the base from the OS's own rules, so a Windows user
/// gets the Known Folder location, not a `$HOME` that does not exist.
pub fn config_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context(NO_PROFILE)?;
    Ok(base.config_dir().join(APP_DIR))
}

/// Where the app keeps its data (SQLite database, logs). Linux
/// `XDG_DATA_HOME`/`~/.local/share`, macOS `~/Library/Application Support`,
/// Windows `%LOCALAPPDATA%`.
pub fn data_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context(NO_PROFILE)?;
    Ok(base.data_local_dir().join(APP_DIR))
}

/// The message for a machine with no per-user profile. Windows has no `$HOME`,
/// so the old `neither XDG_CONFIG_HOME nor HOME is set` bail was a lie there:
/// the real cause is the OS not giving a per-user location, and the message
/// names that.
pub const NO_PROFILE: &str =
    "no user profile directory — wattdeck needs the OS per-user config and data locations";

/// Load the TOML config from `path`, or from the default location. A missing file
/// yields defaults so `wattdeck --mock` works on a fresh machine.
pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let file = match path {
        Some(p) => p.to_path_buf(),
        None => config_dir()?.join("config.toml"),
    };
    if !file.exists() {
        let cfg = Config {
            source_path: file,
            ..Config::default()
        };
        cfg.validate()?;
        return Ok(cfg);
    }
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("reading config {}", file.display()))?;
    let mut cfg: Config =
        toml::from_str(&text).with_context(|| format!("parsing config {}", file.display()))?;
    cfg.source_path = file;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn config_roundtrip_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let cfg: Config = toml::from_str("[api]\nregion = \"AP\"\n").unwrap();
        assert_eq!(cfg.api.region, settings::Region::AP);
        assert_eq!(cfg.api.refresh_interval_seconds, 30);
    }

    #[test]
    fn dirs_are_named_after_the_app() {
        assert_eq!(
            config_dir().unwrap().file_name().unwrap().to_str(),
            Some(APP_DIR)
        );
        assert_eq!(
            data_dir().unwrap().file_name().unwrap().to_str(),
            Some(APP_DIR)
        );
        assert_eq!(APP_DIR, "wattdeck");
    }
}
