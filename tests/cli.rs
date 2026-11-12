use std::path::Path;
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
/// wizard. The assertion uses the path the app prints, because the config
/// location is platform-specific: `XDG_CONFIG_HOME` steers it on Linux, on macOS
/// the app dir is `$HOME/Library/Application Support/wattdeck`, on Windows
/// `%APPDATA%\wattdeck`. Asserting a fixed `wattdeck/config.toml` under the temp
/// HOME is a Linux expectation, and it failed on the macOS runner
/// (run 43064495565: the file went to
/// `…/Library/Application Support/wattdeck/config.toml`). The app's own words
/// name the file on every platform, so the test checks the named file: it
/// exists, it is in a directory named after the app, and it is the starter
/// config. The credential template is not asserted: an isolated HOME does not
/// disable the keyring (the user bus is per-user).
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
    let line = text
        .lines()
        .find(|l| l.starts_with("wrote ") && l.contains("config.toml"))
        .unwrap_or_else(|| panic!("the app must name the config file it wrote: {text}"));
    let path = Path::new(line.trim_start_matches("wrote "));
    // `Path` is not `Display`, and a path may hold non-Unicode bytes, so the
    // failure messages print `path.display()` (1.89.0 printed E0277 when the
    // test interpolated `{path}`).
    let shown = path.display();
    assert!(
        path.exists(),
        "the named config file is not on disk: {text}"
    );
    let app_dir = path
        .parent()
        .unwrap_or_else(|| panic!("the config has no parent directory: {shown}"));
    assert_eq!(
        app_dir.file_name().and_then(|s| s.to_str()),
        Some("wattdeck"),
        "the config must live in the app's own directory: {shown}"
    );
    let body = std::fs::read_to_string(path).unwrap();
    assert!(
        body.contains("region = \"EU\""),
        "the written file is not the starter config: {body}"
    );
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

/// The launch check is opt-out, and the headless path is a subcommand. `--help` is
/// where the surface is checked; `--help` touches no network.
#[test]
fn the_update_surface_is_in_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_wattdeck"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--no-update"), "--help printed: {text}");
    assert!(
        text.contains("update"),
        "the headless update command is not in help: {text}"
    );
}
