use std::process::Command;

/// The CLI name is the app's public identity, so `--help` is where it is checked.
/// `--help` touches no network, no credentials and no device.
#[test]
fn help_names_the_app() {
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("wattdeck"), "--help printed: {text}");
    assert!(!text.contains("ecoflow-tui"), "--help printed: {text}");
}

/// `--version` prints the clap command name, which is the name set in `main.rs`,
/// not the binary name, so this is the assertion that pins the CLI name itself.
#[test]
fn version_names_the_app() {
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--version")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("wattdeck {}", env!("CARGO_PKG_VERSION")),
        "--version printed: {text}"
    );
}

/// The onboarding surface is `wattdeck setup` and `wattdeck --init`. `--setup`
/// means two things and is gone.
#[test]
fn setup_and_init_are_in_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("init"), "--help printed: {text}");
    assert!(text.contains("setup"), "--help printed: {text}");
    for line in text.lines() {
        assert!(
            !line.contains("--setup"),
            "`--setup` is still in help: {line}"
        );
    }
}

/// `--init` is the scripted path: it writes the starter config and points at the
/// wizard. `XDG_CONFIG_HOME` is set to the temp dir so the config location is
/// the one the test can see — an isolated HOME does not disable the keyring (the
/// user bus is per-user), so the credential template is not asserted here.
#[test]
fn init_writes_the_starter_config_and_points_at_setup() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--init")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let cfg = dir.path().join("wattdeck/config.toml");
    assert!(cfg.exists(), "config written: {text}");
    assert!(
        text.contains("wattdeck setup"),
        "hint names the wizard: {text}"
    );
    assert_eq!(out.status.code(), Some(0), "exit code: {text}");
}

/// Mock mode means no network, whatever the credential store holds. The headless
/// runs (`devices`, `status`, `export`) must obey `--mock` the same way the TUI
/// does: no provider that contacts the API, and no history file. An isolated
/// HOME keeps the run's files visible to the test.
#[test]
fn mock_run_never_contacts_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .args(["--debug", "--mock", "status"])
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .unwrap();
    // `print_status` writes the mock note to stderr, so the assertion reads both.
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = dir.path().join("wattdeck").join("wattdeck.log");
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(out.status.code(), Some(0), "status failed: {log_text}");
    assert!(all.contains("note: mock provider"), "output: {all}");
    assert!(
        !dir.path().join("wattdeck").join("history.sqlite").exists(),
        "a mock run wrote history"
    );
    assert!(
        !log_text.contains("api error"),
        "the mock run talked to the API: {log_text}"
    );
}

/// The wizard reads from a terminal. Piped stdin is not one, and the message
/// names the env-var path so a scripted install has an exit.
#[test]
fn wizard_needs_a_terminal() {
    use std::process::Stdio;
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("setup")
        .env("HOME", "/tmp/opencode/wattdeck-home")
        .env("XDG_CONFIG_HOME", "/tmp/opencode/wattdeck-home")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "must fail: {text}");
    assert!(text.contains("needs a terminal"), "message: {text}");
    assert!(
        text.contains("ECOFLOW_ACCESS_KEY"),
        "env-var path named: {text}"
    );
}
