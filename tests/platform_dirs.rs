#[cfg(target_os = "windows")]
use std::path::Path;
use std::process::Command;

/// `wattdeck --init` prints the config path, so the CLI surface is where the
/// platform map is checked. `LiveProbe` only reads the keyring
/// (`src/secrets/keyring_store.rs:127-133`), so `--init` with an isolated HOME
/// writes nothing real.
#[cfg(target_os = "linux")]
#[test]
fn xdg_config_home_is_respected() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--init")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let written = dir.path().join("wattdeck").join("config.toml");
    assert!(
        text.contains(&written.display().to_string()),
        "init wrote: {text}"
    );
    assert!(
        written.exists(),
        "config file at {} not written",
        written.display()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn home_gives_the_xdg_default() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--init")
        .env("HOME", dir.path())
        .env_remove("XDG_CONFIG_HOME")
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let written = dir
        .path()
        .join(".config")
        .join("wattdeck")
        .join("config.toml");
    assert!(
        text.contains(&written.display().to_string()),
        "init wrote: {text}"
    );
    assert!(
        written.exists(),
        "config file at {} not written",
        written.display()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_uses_the_application_support_location() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--init")
        .env("HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let written = dir
        .path()
        .join("Library/Application Support")
        .join("wattdeck")
        .join("config.toml");
    assert!(
        text.contains(&written.display().to_string()),
        "init wrote: {text}"
    );
    assert!(
        written.exists(),
        "config file at {} not written",
        written.display()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_uses_the_known_folder_locations() {
    // SHGetKnownFolderPath ignores the APPDATA/LOCALAPPDATA env vars (verified
    // in dirs-sys-0.4.1/src/lib.rs:151-182), so the assertion is against the
    // runner's real profile: the path must start with %APPDATA% and end with
    // the app directory. The runner is ephemeral.
    let appdata = std::env::var("APPDATA").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--init")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let written = Path::new(&appdata).join("wattdeck").join("config.toml");
    assert!(
        text.contains(&written.display().to_string()),
        "init wrote: {text}"
    );
    assert!(
        written.exists(),
        "config file at {} not written",
        written.display()
    );
    std::fs::remove_dir_all(Path::new(&appdata).join("wattdeck")).unwrap();
}

/// The log file lands in the data directory, so `--debug` shows where data lives.
#[cfg(target_os = "linux")]
#[test]
fn linux_data_dir_is_xdg_or_the_home_default() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .args(["--debug", "--mock", "status"])
        .env("HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "status failed: {text}");
    let log = dir.path().join("wattdeck").join("wattdeck.log");
    assert!(
        log.exists(),
        "no log file at {} (stderr: {text})",
        log.display()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_data_dir_is_local_app_data() {
    let local = std::env::var("LOCALAPPDATA").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .args(["--debug", "--mock", "status"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "status failed: {text}");
    let log = Path::new(&local).join("wattdeck").join("wattdeck.log");
    assert!(
        log.exists(),
        "no log file at {} (stderr: {text})",
        log.display()
    );
    std::fs::remove_dir_all(Path::new(&local).join("wattdeck")).unwrap();
}

/// The message for a machine with no per-user profile names the real cause.
/// `BaseDirs::new()` is `None` only when the OS lookup fails, which cannot be
/// simulated from a test, so the words are pinned and the unreachable arm is
/// labelled.
#[test]
fn a_missing_profile_is_named_by_the_message() {
    assert!(
        wattdeck::config::NO_PROFILE.contains("user profile"),
        "{}",
        wattdeck::config::NO_PROFILE
    );
}
