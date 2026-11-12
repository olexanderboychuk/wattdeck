# Auto-update on launch — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On launch, wattdeck checks GitHub for a newer release, verifies the asset's SHA256 before extracting, installs it over the running binary, and tells the user to restart to apply it.

**Architecture:** A tokio task owns the check (`src/update/`) and sends its status through the existing `mpsc<Update>` channel; `App` turns it into state, the footer and the Alerts screen show it, and `main` prints the restart line after the terminal is restored. Nothing on the frame path touches the network. `wattdeck update` runs the same pipeline headless.

**Tech Stack:** Rust 2024 edition, 1.89.0+ (`rust-toolchain.toml`); `reqwest` (rustls), `serde_json`, `sha2`, `hex`, `tokio`, `crossterm` 0.28, `ratatui` 0.28, `clap` 4, `tracing`. Extraction uses the system `tar`. **No crate is added to `Cargo.toml`.**

**Spec:** `docs/superpowers/specs/2026-11-12-auto-update-design.md` — read it with this plan; the plan argues from it.

## Global constraints

- **Zero new crates.** `sha2 0.11`, `hex 0.4`, `reqwest 0.13`, `serde_json 1`, `tokio 1` are already direct dependencies. `tempfile 3.27` is the dev-dependency for tests.
- **Mock mode does no network.** The launch check is skipped in a mock run — except when `WATTDECK_RELEASE_DIR` is set, which is offline by construction (the screenshot renderer runs mock runs).
- **Unverified bytes are never run.** The downloaded asset's SHA256 is compared to the `SHA256SUMS.txt` line for that asset name **before** anything is extracted. A mismatch is a refusal, and nothing is installed.
- **No sudo, no root, no system directory.** The install target is a parameter, supplied by the caller as `current_exe()`. A target directory that is not writable is refused by name.
- **A combination outside the built targets is refused by name**, with the built targets listed.
- **Exact message strings**, verbatim:
  - footer, applied: `restart to apply 9.9.9`
  - Alerts screen, applied: `updated wattdeck 0.2.0 → 9.9.9 · restart to apply`
  - stdout after `restore_terminal()`: `updated wattdeck 0.2.0 → 9.9.9 · restart to apply the new build`
  - `wattdeck update`, current: `wattdeck 0.2.0 is the latest build`
  - `wattdeck update`, installed: `updated wattdeck 0.2.0 → 9.9.9 · restart to apply the new build`
  - `wattdeck update`, failure: `update check failed: <reason>`, exit non-zero
- **The install script's test seams are reused**, documented as test-only: `WATTDECK_RELEASE_DIR` (a directory of release assets), `WATTDECK_TAG` (the latest tag). With both set, the check needs no network.
- **Label what is unverified.** No Windows machine has run wattdeck, so the rename of a running `.exe`, `bsdtar`'s zip handling, and the `.old` cleanup are claimed, not tested; the integration test skips Windows by name and says so.
- **Run every check through `make`**, which pins the toolchain (`$(HOME)/.local/rust-1.99.0`): `make test` (unit + CLI-surface tests), `make fmt-check`, `make clippy`, `make` (all of it). A single test:
  `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib <filter> -v`
- Commit after every task. Message names what the task proved, in the repo's voice (`git add <files> && git commit -m "…"`).
- `cargo test --all-targets` builds the binary, so `tests/` integration tests run the real binary via `env!("CARGO_BIN_EXE_wattdeck")`.

---

## Task 1: `[update]` config, validation, starter config, docs

**Files:**
- Modify: `src/config/settings.rs` (add `UpdateConfig`, the `update` field in `Config` at lines 9-22, the `validate()` rules at lines 240-285, tests at lines 292-336)
- Modify: `src/main.rs:186-231` (`starter_config`)
- Modify: `docs/CONFIGURATION.md` (new `## [update]` section, and the validation list)

**Interfaces:**
- Consumes: nothing.
- Produces: `wattdeck::config::settings::UpdateConfig { pub enabled: bool, pub repo: String, pub timeout_seconds: u64 }` with `Default` = `enabled: true`, `repo: "olexanderboychuk/wattdeck"`, `timeout_seconds: 10`; `Config::update: UpdateConfig`; `Config::validate()` rejects `update.timeout_seconds` outside 1..=60 and an empty `update.repo`.

- [ ] **Step 1: Write the failing tests** — append to the `#[cfg(test)] mod tests` block in `src/config/settings.rs`:

```rust
#[test]
fn update_defaults_check_every_launch() {
    let cfg = Config::default();
    assert!(cfg.update.enabled, "the launch check is on by default");
    assert_eq!(cfg.update.repo, "olexanderboychuk/wattdeck");
    assert_eq!(cfg.update.timeout_seconds, 10);
}

#[test]
fn update_timeout_outside_1_to_60_is_refused() {
    let mut cfg = Config::default();
    cfg.update.timeout_seconds = 0;
    assert!(cfg.validate().is_err(), "0 s must be refused");
    cfg.update.timeout_seconds = 61;
    assert!(cfg.validate().is_err(), "61 s must be refused");
    cfg.update.timeout_seconds = 10;
    cfg.update.repo = "  ".to_string();
    assert!(cfg.validate().is_err(), "an empty repo must be refused");
}

#[test]
fn config_toml_without_an_update_section_uses_defaults() {
    let cfg: Config = toml::from_str("[api]\nregion = \"US\"\n").unwrap();
    assert!(cfg.update.enabled, "a config file may name no [update] section");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib config -v`
Expected: FAIL — `no field 'update'` / `cannot find UpdateConfig` (compile error).

- [ ] **Step 3: Write the implementation** — in `src/config/settings.rs`, after `ControlConfig`'s `Default` impl, add:

```rust
/// The launch update check. Mock runs skip it; `--no-update` forces it off for
/// one run, whatever this says.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateConfig {
    /// Off means no GitHub request, no download, nothing installed.
    pub enabled: bool,
    /// The fork escape hatch: the repo the latest-release query is aimed at.
    pub repo: String,
    /// Request timeout, seconds.
    pub timeout_seconds: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: "olexanderboychuk/wattdeck".to_string(),
            timeout_seconds: 10,
        }
    }
}
```

Add the field to `Config`, after `controls`:

```rust
    pub controls: ControlConfig,
    pub update: UpdateConfig,
```

Add the rules inside `Config::validate()`, before `Ok(())`:

```rust
        anyhow::ensure!(
            self.update.timeout_seconds >= 1 && self.update.timeout_seconds <= 60,
            "update.timeout_seconds must be between 1 and 60, got {}",
            self.update.timeout_seconds
        );
        anyhow::ensure!(
            !self.update.repo.trim().is_empty(),
            "update.repo must name a repo"
        );
```

- [ ] **Step 4: Add `[update]` to the starter config** — in `src/main.rs`'s `starter_config`, before the closing `[controls]` block:

```
[update]
# The launch check: one GitHub API request, verify the asset's SHA256, install.
# Off means no network: enabled = false, or wattdeck --no-update
enabled = true
repo = "olexanderboychuk/wattdeck"
timeout_seconds = 10
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib config -v`
Expected: PASS, including the existing `init_writes_the_starter_config_and_points_at_setup` (it asserts `region = "EU"` is in the starter file, which is unchanged).

- [ ] **Step 6: Document the keys** — in `docs/CONFIGURATION.md`, add after the `[controls]` section:

```markdown
## `[update]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Check for a new build on every launch, verify the asset's SHA256, install it, and say `restart to apply`. `false`, or `wattdeck --no-update`, means no network. A mock run never checks, whatever this says |
| `repo` | `"olexanderboychuk/wattdeck"` | The fork escape hatch: the repo the latest-release query is aimed at |
| `timeout_seconds` | `10` | **1–60.** GitHub request timeout |

`wattdeck update` runs the same check without the TUI: check, verify, install,
print what happened. The check is one unauthenticated GitHub API request per
launch; GitHub's documented limit is 60 requests per hour per IP.
```

and add to the `## Validation` paragraph: `… and `update.timeout_seconds` outside 1–60, or an empty `update.repo`. The app refuses to start on a config that …` (keep the existing sentence, extend the list).

- [ ] **Step 7: Full verify, then commit**

Run: `make fmt && make test && make clippy`
Expected: PASS on all three.

```bash
git add src/config/settings.rs src/main.rs docs/CONFIGURATION.md
git commit -m "The [update] section: one check per launch, 1-60 s timeout, the repo as the fork escape hatch, defaults asserted"
```

---

## Task 2: the `update` module, `Status`, version compare, launch gate

**Files:**
- Create: `src/update/mod.rs`
- Modify: `src/lib.rs` (add `pub mod update;`)

**Interfaces:**
- Consumes: `UpdateConfig` from Task 1.
- Produces:
  - `wattdeck::update::Status` — `#[derive(Debug, Clone, PartialEq)] pub enum Status { UpToDate, Available { to: String }, Applied { from: String, to: String }, Failed(String) }`
  - `wattdeck::update::Seams { pub release_dir: Option<std::path::PathBuf>, pub tag: Option<String> }`
  - `pub fn seams() -> Seams` (reads `WATTDECK_RELEASE_DIR`, `WATTDECK_TAG`)
  - `pub fn parse_version(text: &str) -> Option<(u64, u64, u64)>`
  - `pub fn is_newer(tag: &str, current: &str) -> bool`
  - `pub fn launch_check(cfg: &UpdateConfig, no_update: bool, mock: bool, s: &Seams) -> bool`

- [ ] **Step 1: Write the failing tests** — `src/update/mod.rs`, tests at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_tag_is_newer_and_a_senior_one_is_not() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("v1.0.0", "0.99.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        // A tag that does not parse is not newer; never a guess.
        assert!(!is_newer("latest", "0.2.0"));
        assert!(!is_newer("v0.2", "0.2.0"));
        assert!(!is_newer("v0.2.0.1", "0.2.0"));
        assert!(parse_version("v2.10.3") == Some((2, 10, 3)));
    }

    #[test]
    fn the_launch_gate_is_enabled_not_forced_off_and_mock_stays_offline() {
        let cfg = UpdateConfig::default();
        let off = Seams { release_dir: None, tag: None };
        let seams = Seams {
            release_dir: Some(std::path::PathBuf::from("/tmp/wattdeck-rel")),
            tag: Some("v9.9.9".to_string()),
        };
        assert!(launch_check(&cfg, false, false, &off));
        assert!(!launch_check(&cfg, true, false, &off), "--no-update forces it off");
        assert!(!launch_check(&cfg, false, true, &off), "a mock run does no network");
        assert!(
            launch_check(&cfg, false, true, &seams),
            "a mock run with the release-dir seam is offline by construction"
        );
        let mut off_cfg = UpdateConfig::default();
        off_cfg.enabled = false;
        assert!(!launch_check(&off_cfg, false, false, &seams));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation** — `src/lib.rs`, add after `pub mod telemetry;` (keep alphabetical order: `update` between `telemetry` and `ui`):

```rust
pub mod update;
```

`src/update/mod.rs`:

```rust
//! The launch check: latest release, SHA256 verify, install over the running
//! binary. The network lives here; the UI receives one `Update::Release`.

use std::path::PathBuf;

use crate::config::settings::UpdateConfig;

/// What the check did. `Available` is transient: it is traced, never drawn, and a
/// silent apply goes straight to `Applied`.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    UpToDate,
    Available { to: String },
    Applied { from: String, to: String },
    Failed(String),
}

/// The test seams, documented as test-only in the README: a directory of release
/// assets, and a tag. With both set, the check touches no network.
#[derive(Debug, Clone, Default)]
pub struct Seams {
    pub release_dir: Option<PathBuf>,
    pub tag: Option<String>,
}

pub fn seams() -> Seams {
    Seams {
        release_dir: std::env::var("WATTDECK_RELEASE_DIR").map(PathBuf::from).ok(),
        tag: std::env::var("WATTDECK_TAG").ok(),
    }
}

/// `v1.2.3` and `1.2.3` are `(1, 2, 3)`. Anything else is `None`: a tag that does
/// not parse is never treated as newer. Tags are `v*.*.*` (the release workflow
/// triggers on that pattern), so a numeric tuple is enough and `semver` is not
/// added.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let body = text.trim().strip_prefix('v').unwrap_or(text.trim());
    let mut parts = body.split('.');
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((a.parse().ok()?, b.parse().ok()?, c.parse().ok()?))
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

/// The launch gate: enabled, not forced off for the run, and a mock run stays
/// offline. A mock run with `WATTDECK_RELEASE_DIR` set is offline by
/// construction, so the seam is the only way a mock run checks — and the only way
/// the footer line and the Alerts entry are verifiable without a network.
pub fn launch_check(cfg: &UpdateConfig, no_update: bool, mock: bool, s: &Seams) -> bool {
    cfg.enabled && !no_update && (!mock || s.release_dir.is_some())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/lib.rs src/update/mod.rs
git commit -m "The update module: numeric tag compare, the test seams, and a launch gate that keeps mock runs offline"
```

---

## Task 3: target triple, asset name, SHA256 parse and hash

**Files:**
- Modify: `src/update/mod.rs` (append the target table, asset name, sums parse, hash; tests in the module's `tests`)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub struct Target { pub triple: &'static str, pub ext: &'static str, pub binary: &'static str }`
  - `pub fn target_for(os: &str, arch: &str) -> Result<Target, String>`
  - `pub fn target() -> Result<Target, String>` (from `std::env::consts`)
  - `pub fn asset_name(tag: &str, t: &Target) -> String`
  - `pub fn expected_hash(sums: &str, asset: &str) -> Option<String>`
  - `pub fn sha256_of(bytes: &[u8]) -> String`

- [ ] **Step 1: Write the failing tests** — append to `src/update/mod.rs`'s `tests`:

```rust
#[test]
fn the_three_built_targets_are_the_installers_table() {
    assert_eq!(target_for("linux", "x86_64").unwrap().triple, "x86_64-unknown-linux-gnu");
    assert_eq!(target_for("macos", "aarch64").unwrap().triple, "aarch64-apple-darwin");
    assert_eq!(
        target_for("windows", "x86_64").unwrap().ext,
        "zip",
        "the Windows asset is a zip, as the release job packages it"
    );
    let refused = target_for("linux", "aarch64").unwrap_err();
    assert!(refused.contains("no wattdeck build for linux on aarch64"), "{refused}");
    assert!(refused.contains("x86_64-unknown-linux-gnu"), "{refused}");
}

#[test]
fn the_asset_name_is_the_release_jobs_pattern() {
    let t = target_for("linux", "x86_64").unwrap();
    assert_eq!(
        asset_name("v0.3.0", &t),
        "wattdeck-v0.3.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(t.binary, "wattdeck");
    assert_eq!(target_for("windows", "x86_64").unwrap().binary, "wattdeck.exe");
}

#[test]
fn the_sums_line_for_the_asset_is_the_first_field() {
    let sums = "aaa  wattdeck-v0.3.0-x86_64-unknown-linux-gnu.tar.gz\nbbb  SHA256SUMS.txt\n";
    assert_eq!(
        expected_hash(sums, "wattdeck-v0.3.0-x86_64-unknown-linux-gnu.tar.gz"),
        Some("aaa".to_string())
    );
    assert_eq!(expected_hash(sums, "SHA256SUMS.txt"), Some("bbb".to_string()));
    assert_eq!(expected_hash(sums, "wattdeck-v9.9.9-x86_64-unknown-linux-gnu.tar.gz"), None);
    // A line with a third field is not a sums line.
    assert_eq!(expected_hash("aaa  asset  extra\n", "asset"), None);
    assert_eq!(expected_hash("AAA  asset\n", "asset"), Some("aaa".to_string()));
}

#[test]
fn the_hash_of_the_empty_input_is_the_published_one() {
    // The published SHA256 of zero bytes; the string is the value, not a claim.
    assert_eq!(
        sha256_of(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: FAIL — `target_for`, `asset_name`, `expected_hash`, `sha256_of` are not defined.

- [ ] **Step 3: Write the implementation** — append to `src/update/mod.rs`:

```rust
/// The asset for one target, and the binary name inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub triple: &'static str,
    pub ext: &'static str,
    pub binary: &'static str,
}

/// The targets this project builds — `install.sh`'s table, the same words. A
/// combination outside them is refused by name, so a user on an unsupported
/// machine learns what exists, not a guess.
pub fn target_for(os: &str, arch: &str) -> Result<Target, String> {
    match (os, arch) {
        ("linux", "x86_64") => Ok(Target {
            triple: "x86_64-unknown-linux-gnu",
            ext: "tar.gz",
            binary: "wattdeck",
        }),
        ("macos", "aarch64") => Ok(Target {
            triple: "aarch64-apple-darwin",
            ext: "tar.gz",
            binary: "wattdeck",
        }),
        ("windows", "x86_64") => Ok(Target {
            triple: "x86_64-pc-windows-msvc",
            ext: "zip",
            binary: "wattdeck.exe",
        }),
        (os, arch) => Err(format!(
            "no wattdeck build for {os} on {arch}. Built targets: x86_64-unknown-linux-gnu \
             (Linux x86_64), aarch64-apple-darwin (macOS Apple silicon), \
             x86_64-pc-windows-msvc (Windows x86_64)."
        )),
    }
}

pub fn target() -> Result<Target, String> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn asset_name(tag: &str, t: &Target) -> String {
    format!("wattdeck-{}-{}.{}", tag, t.triple, t.ext)
}

/// `SHA256SUMS.txt` lines are `<hash>  <asset>`, which is what the release job
/// writes. The hash for `asset`, lowercased, or `None` when the asset is not
/// listed. A line with a third field is not a sums line, so it is not read.
pub fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    for line in sums.lines() {
        let mut fields = line.split_whitespace();
        match (fields.next(), fields.next(), fields.next()) {
            (Some(hash), Some(name), None) if name == asset => {
                return Some(hash.to_lowercase());
            }
            _ => {}
        }
    }
    None
}

pub fn sha256_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/update/mod.rs
git commit -m "Target table, asset name, and the SHA256SUMS parse: the app's refusal is the installer's refusal, by name"
```

---

## Task 4: `src/update/install.rs` — extract, replace, clean up

**Files:**
- Create: `src/update/install.rs`
- Modify: `src/update/mod.rs` (add `pub mod install;` at the top)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub fn extract(archive: &Path, into: &Path, binary: &str) -> Result<PathBuf, String>`
  - `pub fn install(src: &Path, target: &Path) -> Result<(), String>`
  - `pub fn stale_path(target: &Path) -> PathBuf`
  - `pub fn cleanup_stale(target: &Path)`

This file does no network. One purpose: take verified bytes and put them where the binary lives.

- [ ] **Step 1: Write the failing tests** — `src/update/install.rs`, at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A tar.gz holding one marker binary, built with the system `tar` — the tool
    /// the installer uses too, so the test needs no archive crate.
    fn archive(dir: &std::path::Path) -> std::path::PathBuf {
        let marker_dir = dir.join("marker");
        std::fs::create_dir_all(&marker_dir).unwrap();
        let marker = marker_dir.join("wattdeck");
        std::fs::write(&marker, "#!/bin/sh\necho \"wattdeck 9.9.9\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let asset = dir.join("asset.tar.gz");
        let status = std::process::Command::new("tar")
            .args([
                "-czf",
                asset.to_str().unwrap(),
                "-C",
                marker_dir.to_str().unwrap(),
                "wattdeck",
            ])
            .status()
            .expect("tar is present on the test machine");
        assert!(status.success(), "tar failed to build the fixture");
        asset
    }

    #[cfg(unix)]
    #[test]
    fn the_archive_is_extracted_and_the_target_is_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let asset = archive(dir.path());
        let target = dir.path().join("wattdeck");
        std::fs::write(&target, "the old build").unwrap();

        let bin = extract(&asset, &dir.path().join("stage"), "wattdeck").unwrap();
        install(&bin, &target).unwrap();

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "#!/bin/sh\necho \"wattdeck 9.9.9\"\n",
            "the installed file is not the marker"
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "the installed binary must be executable");
        // The staged file is renamed away, not left beside the target.
        assert!(!dir.path().join("wattdeck.new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_archive_without_the_binary_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let asset = dir.path().join("asset.tar.gz");
        std::process::Command::new("tar")
            .args([
                "-czf",
                asset.to_str().unwrap(),
                "-C",
                dir.path().join("empty").to_str().unwrap(),
            ])
            .status()
            .expect("tar");
        let err = extract(&asset, &dir.path().join("stage"), "wattdeck").unwrap_err();
        assert!(err.contains("the archive did not contain wattdeck"), "{err}");
    }

    #[test]
    fn the_stale_name_is_the_target_with_the_old_suffix() {
        assert_eq!(
            stale_path(std::path::Path::new("c:/wattdeck/wattdeck.exe")),
            PathBuf::from("c:/wattdeck/wattdeck.exe.old")
        );
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib install -v`
Expected: FAIL — the module is missing.

- [ ] **Step 3: Write the implementation** — `src/update/install.rs`:

```rust
//! Verified bytes to the binary's own location. No network.

use std::path::{Path, PathBuf};

/// Extract the verified archive with the system `tar` and return the binary.
/// Windows ships `tar.exe` (bsdtar), which reads a zip; that claim is labelled
/// unverified on the spec, because no Windows machine has run wattdeck.
pub fn extract(archive: &Path, into: &Path, binary: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("creating {}: {e}", into.display()))?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|_| "no tar found; install the binary from source".to_string())?;
    if !status.success() {
        return Err(format!("tar failed on {}", archive.display()));
    }
    let bin = into.join(binary);
    if !bin.exists() {
        return Err(format!("the archive did not contain {binary}"));
    }
    Ok(bin)
}

/// Put the new binary where the running one lives. unix writes
/// `<dir>/wattdeck.new` in the target's own directory, so the rename is on the
/// same filesystem and atomic, and the running process keeps its old inode —
/// which is why the message is `restart`, not `reloaded`.
pub fn install(src: &Path, target: &Path) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or("the install target has no parent directory")?;
    let stem = target
        .file_stem()
        .ok_or("the install target has no file name")?
        .to_string_lossy()
        .to_string();
    let staged = dir.join(format!("{stem}.new"));
    let bytes = std::fs::read(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    std::fs::write(&staged, bytes)
        .map_err(|e| format!("{} is not writable: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("setting the mode on {}: {e}", staged.display()))?;
    }
    #[cfg(windows)]
    {
        // A rename of a running executable is allowed; the copy in place is not.
        let stale = stale_path(target);
        if stale.exists() {
            let _ = std::fs::remove_file(&stale);
        }
        std::fs::rename(target, &stale)
            .map_err(|e| format!("moving the running binary aside: {e}"))?;
    }
    std::fs::rename(&staged, target)
        .map_err(|e| format!("installing {}: {e}", target.display()))?;
    Ok(())
}

pub fn stale_path(target: &Path) -> PathBuf {
    PathBuf::from(format!("{}.old", target.display()))
}

/// The leftover `.old` from the last Windows update, deleted at the next launch.
/// A file that cannot be deleted is traced and left: it never blocks the check.
pub fn cleanup_stale(target: &Path) {
    let stale = stale_path(target);
    if stale.exists()
        && let Err(e) = std::fs::remove_file(&stale)
    {
        tracing::warn!("cannot remove {}: {e}", stale.display());
    }
}
```

and in `src/update/mod.rs`, after the module doc comment at the top:

```rust
pub mod install;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib install -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/update/install.rs src/update/mod.rs
git commit -m "Self-replace: staged rename in the target's own directory, mode 755, a refusal naming an unwritable directory"
```

---

## Task 5: the check pipeline

**Files:**
- Modify: `src/update/mod.rs` (add `Asset`, `latest`, `fetch`, `run_with_seams`, `run`, and tests)

**Interfaces:**
- Consumes: `Status`, `Seams`, `seams()`, `is_newer`, `target()`, `asset_name`, `expected_hash`, `sha256_of` from Tasks 2-3; `install::extract`, `install::install` from Task 4; `UpdateConfig` from Task 1; `crate::services::Update`, whose `Release(crate::update::Status)` variant is added by Task 6. Do Task 6's Step 3 first if the compiler needs the variant; the two tasks share one build.
- Produces:
  - `pub struct Asset { pub name: String, pub url: String }`
  - `pub async fn latest(cfg: &UpdateConfig, s: &Seams) -> Result<(String, Vec<Asset>), String>`
  - `pub async fn run_with_seams(cfg: &UpdateConfig, current: &str, cache: &Path, target: &Path, tx: Option<&tokio::sync::mpsc::Sender<Update>>, s: &Seams) -> Result<Status, String>`
  - `pub async fn run(cfg: &UpdateConfig, current: &str, cache: &Path, target: &Path, tx: Option<&mpsc::Sender<Update>>) -> Status`

- [ ] **Step 1: Write the failing tests** — append to `src/update/mod.rs`'s `tests`:

```rust
#[cfg(unix)]
#[test]
fn a_verified_asset_installs_over_the_target_and_reports_the_versions() {
    let dir = tempfile::tempdir().unwrap();
    // The fixture: a marker binary in the asset, and the sums file. The asset
    // name is computed for the host's target, so this test runs on Linux and
    // macOS, and is skipped by name on Windows.
    let marker_dir = dir.path().join("marker");
    std::fs::create_dir_all(&marker_dir).unwrap();
    let marker = marker_dir.join("wattdeck");
    std::fs::write(&marker, "#!/bin/sh\necho \"wattdeck 9.9.9\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rel = dir.path().join("release");
    std::fs::create_dir_all(&rel).unwrap();
    let t = target().unwrap();
    let asset = asset_name("v9.9.9", &t);
    let status = std::process::Command::new("tar")
        .args([
            "-czf",
            rel.join(&asset).to_str().unwrap(),
            "-C",
            marker_dir.to_str().unwrap(),
            "wattdeck",
        ])
        .status()
        .expect("tar");
    assert!(status.success());
    let bytes = std::fs::read(rel.join(&asset)).unwrap();
    std::fs::write(
        rel.join("SHA256SUMS.txt"),
        format!("{}  {asset}\n", sha256_of(&bytes)),
    )
    .unwrap();

    // The target the test replaces is a file in the temp dir, never the running
    // test binary.
    let target = dir.path().join("wattdeck");
    std::fs::write(&target, "the old build").unwrap();

    let seams = Seams {
        release_dir: Some(rel.clone()),
        tag: Some("v9.9.9".to_string()),
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let status = rt
        .block_on(run_with_seams(
            &UpdateConfig::default(),
            "0.2.0",
            &dir.path().join("cache"),
            &target,
            None,
            &seams,
        ))
        .unwrap();
    assert_eq!(
        status,
        Status::Applied {
            from: "0.2.0".to_string(),
            to: "9.9.9".to_string()
        },
        "the check did not apply the asset"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "#!/bin/sh\necho \"wattdeck 9.9.9\"\n",
        "the target is not the marker"
    );
}

#[cfg(unix)]
#[test]
fn a_hash_mismatch_installs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rel = dir.path().join("release");
    std::fs::create_dir_all(&rel).unwrap();
    let t = target().unwrap();
    let asset = asset_name("v9.9.9", &t);
    std::fs::write(rel.join(&asset), "not a binary").unwrap();
    std::fs::write(rel.join("SHA256SUMS.txt"), format!("deadbeef  {asset}\n")).unwrap();
    let target = dir.path().join("wattdeck");
    std::fs::write(&target, "the old build").unwrap();

    let seams = Seams {
        release_dir: Some(rel.clone()),
        tag: Some("v9.9.9".to_string()),
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let status = rt
        .block_on(run_with_seams(
            &UpdateConfig::default(),
            "0.2.0",
            &dir.path().join("cache"),
            &target,
            None,
            &seams,
        ))
        .unwrap();
    match status {
        Status::Failed(r) => {
            assert!(r.starts_with("SHA256 mismatch for "), "{r}");
            assert!(r.contains(&asset), "{r}");
        }
        other => panic!("a mismatch must be a refusal, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "the old build",
        "unverified bytes must never be installed"
    );
}

#[test]
fn a_tag_that_is_not_newer_is_up_to_date() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let seams = Seams {
        release_dir: None,
        tag: Some("v0.2.0".to_string()),
    };
    let status = rt
        .block_on(run_with_seams(
            &UpdateConfig::default(),
            "0.2.0",
            &std::path::PathBuf::from("/tmp/wattdeck-uptodate-cache"),
            &std::path::PathBuf::from("/tmp/wattdeck-uptodate-target"),
            None,
            &seams,
        ))
        .unwrap();
    assert_eq!(status, Status::UpToDate, "the same version is not newer");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: FAIL — `run_with_seams` is not defined.

- [ ] **Step 3: Write the implementation** — append to `src/update/mod.rs`:

```rust
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::settings::UpdateConfig;
use crate::services::Update;
use crate::update::install::{extract, install};

/// One asset in the release: its name, and the URL it downloads from.
#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// The latest release. With `WATTDECK_TAG` the answer is the seam and no request
/// is made, the same shape `install.sh` uses, so the shell test and this one
/// share the seams.
pub async fn latest(cfg: &UpdateConfig, s: &Seams) -> Result<(String, Vec<Asset>), String> {
    if let Some(tag) = &s.tag {
        return Ok((tag.clone(), Vec::new()));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_seconds.max(1)))
        .user_agent(concat!("wattdeck/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", cfg.repo);
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("github api: {e}"))?;
    if resp.status().as_u16() == 403 || resp.status().as_u16() == 404 {
        return Err(format!(
            "no latest release in {} (60 requests per hour per IP; wattdeck --no-update, \
             or [update] enabled = false, skips it)",
            cfg.repo
        ));
    }
    let json: Value = resp.json().await.map_err(|e| format!("release json: {e}"))?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if tag.is_empty() {
        return Err("the latest release has no tag_name".to_string());
    }
    let assets = json
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_string(),
                        url: a.get("browser_download_url")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((tag, assets))
}

/// One file from the release: the seam directory, or the asset's own URL.
async fn fetch(
    cfg: &UpdateConfig,
    s: &Seams,
    name: &str,
    assets: &[Asset],
    dest: &Path,
) -> Result<(), String> {
    if let Some(dir) = &s.release_dir {
        let src = dir.join(name);
        if !src.exists() {
            return Err(format!("{name} is not in {}", dir.display()));
        }
        std::fs::copy(&src, dest).map_err(|e| format!("copying {}: {e}", src.display()))?;
        return Ok(());
    }
    let url = assets
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.url.clone())
        .ok_or_else(|| format!("{name} is not in the latest release"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_seconds.max(1)))
        .user_agent(concat!("wattdeck/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("downloading {name}: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("reading {name}: {e}"))?;
    std::fs::write(dest, bytes).map_err(|e| format!("writing {}: {e}", dest.display()))?;
    Ok(())
}

/// The pipeline, with the seams passed in so a test needs no environment.
pub async fn run_with_seams(
    cfg: &UpdateConfig,
    current: &str,
    cache: &Path,
    target: &Path,
    tx: Option<&mpsc::Sender<Update>>,
    s: &Seams,
) -> Result<Status, String> {
    let t = target()?;
    let (tag, assets) = latest(cfg, s).await?;
    if !is_newer(&tag, current) {
        tracing::debug!("wattdeck {current} is the latest build");
        return Ok(Status::UpToDate);
    }
    // The user reads a version, not a tag: the `v` is the release tag's prefix,
    // not part of the number, and the messages the tests assert say `9.9.9`.
    let to = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    if let Some(tx) = tx {
        let _ = tx
            .send(Update::Release(Status::Available { to: to.clone() }))
            .await;
    }
    let work = cache.join(&tag);
    std::fs::create_dir_all(&work)
        .map_err(|e| format!("creating {}: {e}", work.display()))?;
    let asset = asset_name(&tag, &t);
    fetch(cfg, s, "SHA256SUMS.txt", &assets, &work.join("SHA256SUMS.txt")).await?;
    fetch(cfg, s, &asset, &assets, &work.join(&asset)).await?;

    // Verify the hash before extracting: unverified bytes are never run.
    let sums = std::fs::read_to_string(work.join("SHA256SUMS.txt"))
        .map_err(|e| format!("reading SHA256SUMS.txt: {e}"))?;
    let want = expected_hash(&sums, &asset)
        .ok_or_else(|| format!("{asset} is not listed in SHA256SUMS.txt"))?;
    let bytes = std::fs::read(work.join(&asset)).map_err(|e| format!("reading {asset}: {e}"))?;
    let got = sha256_of(&bytes);
    if got != want {
        return Err(format!("SHA256 mismatch for {asset}: expected {want}, got {got}"));
    }

    let bin = extract(&work.join(&asset), &work.join("stage"), t.binary)?;
    install(&bin, target)?;
    Ok(Status::Applied {
        from: current.to_string(),
        to,
    })
}

pub async fn run(
    cfg: &UpdateConfig,
    current: &str,
    cache: &Path,
    target: &Path,
    tx: Option<&mpsc::Sender<Update>>,
) -> Status {
    match run_with_seams(cfg, current, cache, target, tx, &seams()).await {
        Ok(status) => status,
        Err(reason) => Status::Failed(reason),
    }
}
```

`run_with_seams` returns `Result<Status, String>` so a test can tell a refusal from a failure of its own fixture; `run` folds the error into `Status::Failed`, which is what the launch path reports.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib update -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/update/mod.rs
git commit -m "The check pipeline: SHA256 before extract, install over the target; a mismatch installs nothing, proved offline by the seams"
```

---

## Task 6: the channel, `App` state, the Alerts entry, the footer line

**Files:**
- Modify: `src/services/mod.rs` (the `Update` enum, lines 13-40)
- Modify: `src/app.rs` (`AlertKind` at lines 92-109, the `App` struct fields at lines 120-155, `App::new` at lines 171-209, `apply_update` at lines 265-401, tests)
- Modify: `src/ui/widgets/footer.rs` (lines 41-51)

**Interfaces:**
- Consumes: `crate::update::Status` (Task 2).
- Produces:
  - `Update::Release(crate::update::Status)` — the variant Task 5 sends
  - `AlertKind::Update`
  - `App::release: Option<crate::update::Status>`, a public field the footer reads

- [ ] **Step 1: Write the failing tests** — append to `src/app.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn an_applied_update_sets_the_footer_state_and_the_exact_alert_line() {
    let mut a = App::new(Config::default(), true, Scenario::SolarCharging);
    a.apply_update(Update::Release(crate::update::Status::Applied {
        from: "0.2.0".to_string(),
        to: "9.9.9".to_string(),
    }));
    assert_eq!(
        a.release,
        Some(crate::update::Status::Applied {
            from: "0.2.0".to_string(),
            to: "9.9.9".to_string()
        }),
        "the footer state is what the footer draws"
    );
    assert!(
        a.alerts.iter().any(|al| {
            al.kind == AlertKind::Update
                && al.text == "updated wattdeck 0.2.0 → 9.9.9 · restart to apply"
        }),
        "the Alerts screen must carry the exact line: {:?}",
        a.alerts
    );
}

#[test]
fn a_failed_launch_check_is_traced_not_alerted() {
    let mut a = App::new(Config::default(), true, Scenario::SolarCharging);
    a.apply_update(Update::Release(crate::update::Status::Failed(
        "no latest release in olexanderboychuk/wattdeck".to_string(),
    )));
    assert!(
        !a.alerts.iter().any(|al| al.kind == AlertKind::Update),
        "a rate-limit refusal on every launch must not fill the Alerts screen"
    );
    assert!(a.release.is_none(), "a failure is not a footer state");
}

#[test]
fn a_newer_tag_available_is_recorded_without_an_alert() {
    let mut a = App::new(Config::default(), true, Scenario::SolarCharging);
    a.apply_update(Update::Release(crate::update::Status::Available {
        to: "9.9.9".to_string(),
    }));
    assert!(a.release.is_some());
    assert!(!a.alerts.iter().any(|al| al.kind == AlertKind::Update));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib app -v`
Expected: FAIL — `Update::Release` and `App::release` do not exist.

- [ ] **Step 3: Write the implementation**

`src/services/mod.rs`, inside `pub enum Update`, after `ControlReply { … }`:

```rust
    /// The launch update check's result for this run. The network lives in
    /// `src/update/`; the UI receives only this, so the UI never calls the API.
    Release(crate::update::Status),
```

`src/app.rs`, in `pub enum AlertKind`, after `Credential`:

```rust
    /// The launch update check: the build that was verified and installed, and the
    /// restart that applies it.
    Update,
```

In `pub struct App`, after `pub history_cache: …`:

```rust
    /// The launch update check's result for this run, when there is one.
    pub release: Option<crate::update::Status>,
```

In `App::new`, after `history_cache: HashMap::new(),`:

```rust
            release: None,
```

In `App::apply_update`, after the `Update::History(…)` arm, before `Update::Notification(n)`:

```rust
            Update::Release(status) => match status {
                crate::update::Status::Applied { from, to } => {
                    self.system_alert(
                        AlertKind::Update,
                        Level::Info,
                        format!("updated wattdeck {from} → {to} · restart to apply"),
                    );
                    self.release = Some(crate::update::Status::Applied { from, to });
                }
                // A launch failure is traced, not alerted: a rate-limit refusal on
                // every launch must not fill the Alerts screen.
                crate::update::Status::Failed(reason) => {
                    tracing::warn!("update check failed: {reason}");
                }
                other => {
                    tracing::debug!("update check: {other:?}");
                    self.release = Some(other);
                }
            },
```

`src/ui/widgets/footer.rs`, after the tabs `for` loop and before `let right`:

```rust
    if let Some(crate::update::Status::Applied { to, .. }) = &app.release {
        left.push(Span::styled(
            format!(" restart to apply {to} "),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ));
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --lib app -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/services/mod.rs src/app.rs src/ui/widgets/footer.rs
git commit -m "The update check reaches the UI as one Update: the footer segment, the Alerts line, and a failure that stays in the log"
```

---

## Task 7: CLI wiring — `--no-update`, `wattdeck update`, the launch spawn, the restart line

**Files:**
- Modify: `src/main.rs` (`Cli` at lines 37-65, `Cmd` at lines 67-80, `main` at lines 82-147, `run_tui` at lines 435-530)
- Modify: `tests/cli.rs` (append one test)
- Create: `tests/update.rs`

**Interfaces:**
- Consumes: `wattdeck::update::run`, `launch_check`, `seams`, `Status`, `install::cleanup_stale`, `App::release`.
- Produces: the CLI surface `wattdeck --no-update`, `wattdeck update`; the stdout line after `restore_terminal()`.

- [ ] **Step 1: Write the failing tests** — append to `tests/cli.rs`:

```rust
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
```

`tests/update.rs`:

```rust

use std::process::Command;

/// The install path, end to end, with no network: the seams point at a directory
/// of release assets, and the asset carries a marker binary reporting a version
/// no release has. The observable proof is `--version` after the install — the
/// bytes were verified against `SHA256SUMS.txt`, extracted, and installed over the
/// binary the run is using.
///
/// Skipped by name on Windows: the zip asset, the rename of a running `.exe`, and
/// `bsdtar`'s zip handling are labelled unverified in this project (no Windows
/// machine has run wattdeck), so this test asserts the unix path only.
#[test]
fn update_installs_a_verified_asset_and_says_restart() {
    if cfg!(windows) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // The binary the run replaces.
    let installed = dir.path().join("wattdeck");
    std::fs::copy(env!("CARGO_BIN_EXE_wattdeck"), &installed).unwrap();

    // The release dir: a marker binary in the asset, and the sums file.
    let marker_dir = dir.path().join("marker");
    std::fs::create_dir_all(&marker_dir).unwrap();
    let marker = marker_dir.join("wattdeck");
    std::fs::write(&marker, "#!/bin/sh\necho \"wattdeck 9.9.9\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rel = dir.path().join("release");
    std::fs::create_dir_all(&rel).unwrap();
    let t = wattdeck::update::target().unwrap();
    let asset = wattdeck::update::asset_name("v9.9.9", &t);
    let status = std::process::Command::new("tar")
        .args([
            "-czf",
            rel.join(&asset).to_str().unwrap(),
            "-C",
            marker_dir.to_str().unwrap(),
            "wattdeck",
        ])
        .status()
        .expect("tar");
    assert!(status.success(), "the fixture archive failed to build");
    let bytes = std::fs::read(rel.join(&asset)).unwrap();
    std::fs::write(
        rel.join("SHA256SUMS.txt"),
        format!("{}  {asset}\n", wattdeck::update::sha256_of(&bytes)),
    )
    .unwrap();

    let out = Command::new(&installed)
        .arg("update")
        .env("WATTDECK_RELEASE_DIR", rel.path())
        .env("WATTDECK_TAG", "v9.9.9")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "wattdeck update failed: {text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains(&format!(
            "updated wattdeck {} → 9.9.9 · restart to apply the new build",
            env!("CARGO_PKG_VERSION")
        )),
        "stdout: {text}"
    );

    let again = Command::new(&installed).arg("--version").output().unwrap();
    let shown = String::from_utf8_lossy(again.stdout).trim().to_string();
    assert_eq!(
        shown, "wattdeck 9.9.9",
        "the installed binary is not the marker: {shown}"
    );
}

/// A failure is a named refusal and a non-zero exit, not a silent success: the
/// sums file names an asset that is not in the directory.
#[test]
fn an_asset_missing_from_the_sums_is_a_named_refusal() {
    if cfg!(windows) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("wattdeck");
    std::fs::copy(env!("CARGO_BIN_EXE_wattdeck"), &installed).unwrap();
    let rel = dir.path().join("release");
    std::fs::create_dir_all(&rel).unwrap();
    std::fs::write(rel.join("SHA256SUMS.txt"), "deadbeef  something-else.tar.gz\n").unwrap();

    let out = Command::new(&installed)
        .arg("update")
        .env("WATTDECK_RELEASE_DIR", rel.path())
        .env("WATTDECK_TAG", "v9.9.9")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(0), "a refusal must fail: {all}");
    assert!(all.contains("update check failed: "), "refusal: {all}");
    assert!(all.contains("SHA256SUMS.txt"), "the sums file is named: {all}");
    // The binary the run uses is untouched.
    let v = Command::new(&installed).arg("--version").output().unwrap();
    let shown = String::from_utf8_lossy(v.stdout).trim().to_string();
    assert_eq!(
        shown,
        format!("wattdeck {}", env!("CARGO_PKG_VERSION")),
        "a refusal must not change the binary: {shown}"
    );

}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --test cli --test update -v`
Expected: FAIL — `--no-update` / `update` are not in help, and `wattdeck update` exits with a clap "unrecognized subcommand" error.

- [ ] **Step 3: Write the implementation** — `src/main.rs`.

In `struct Cli`, after the `scenario` field:

```rust
    /// Skip the launch update check: no GitHub request, no download, nothing installed.
    #[arg(long)]
    no_update: bool,
```

In `enum Cmd`, after `Setup`:

```rust
    /// Check for a new build, verify its SHA256, install it, print what happened.
    Update,
```

In `main`, right after `let mut cfg = config::load_config(cli.config.as_deref())?;`:

```rust
    if cli.no_update {
        cfg.update.enabled = false;
    }
    if let Ok(exe) = std::env::current_exe() {
        // The leftover from the last Windows update, deleted before the terminal
        // is set up. A no-op on unix when there is no `.old`.
        wattdeck::update::install::cleanup_stale(&exe);
    }
```

In `main`'s `match cli.cmd`, add the arm before `None =>`:

```rust
        Some(Cmd::Update) => {
            let runtime = tokio::runtime::Runtime::new()?;
            let status = runtime.block_on(wattdeck::update::run(
                &cfg.update,
                env!("CARGO_PKG_VERSION"),
                &config::data_dir()?.join("update"),
                &std::env::current_exe()?,
                None,
            ));
            match status {
                wattdeck::update::Status::UpToDate => {
                    println!("wattdeck {} is the latest build", env!("CARGO_PKG_VERSION"));
                }
                wattdeck::update::Status::Applied { from, to } => {
                    println!("updated wattdeck {from} → {to} · restart to apply the new build");
                }
                wattdeck::update::Status::Available { to } => {
                    println!("wattdeck {to} is newer; nothing was installed");
                }
                wattdeck::update::Status::Failed(reason) => {
                    anyhow::bail!("update check failed: {reason}");
                }
            }
            Ok(())
        }
```

The `None` arm becomes:

```rust
        None => {
            let check = wattdeck::update::launch_check(
                &cfg.update,
                cli.no_update,
                use_mock,
                &wattdeck::update::seams(),
            );
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(run_tui(
                cfg,
                creds,
                use_mock,
                scenario,
                cli.device,
                &source,
                check,
            ))
        }
```

`run_tui`'s signature gains one parameter, after `source: &str`:

```rust
    check_update: bool,
```

In `run_tui`, after the provider `tokio::spawn` block and before `enable_raw_mode()?`:

```rust
    if check_update {
        // The check is a background task: the terminal opens without waiting for
        // GitHub, and the answer arrives as one Update.
        let update_cfg = cfg.update.clone();
        let cache = config::data_dir()?.join("update");
        let exe = std::env::current_exe()?;
        let tx_update = tx.clone();
        tokio::spawn(async move {
            let status = wattdeck::update::run(
                &update_cfg,
                env!("CARGO_PKG_VERSION"),
                &cache,
                &exe,
                Some(&tx_update),
            )
            .await;
            let _ = tx_update.send(Update::Release(status)).await;
        });
    }
```

At the end of `run_tui`, replace `restore_terminal()?;` + `result` with:

```rust
    restore_terminal()?;
    // The line is printed after the terminal is restored, so it lands in the
    // scrollback the user reads after quitting.
    if let Some(wattdeck::update::Status::Applied { from, to }) = &app.release {
        println!("updated wattdeck {from} → {to} · restart to apply the new build");
    }
    result
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `LD_LIBRARY_PATH=$HOME/.local/rust-1.99.0/lib $HOME/.local/rust-1.99.0/bin/cargo test --test cli --test update -v`
Expected: PASS.

- [ ] **Step 5: Full verify, then commit**

Run: `make fmt && make test && make clippy`

```bash
git add src/main.rs tests/cli.rs tests/update.rs
git commit -m "wattdeck update and --no-update: the check runs as a task, installs over the running binary, and says restart — proved by the seams"
```

---

## Task 8: docs, the PTY verification, the live probe, the full verify

**Files:**
- Modify: `README.md` (Install at 35-99, FAQ at 256-287, Troubleshooting at 288-305, CLI surface at 317-325, Status at 337-360)
- Modify: `docs/ARCHITECTURE.md` (shape diagram, lines 5-33)
- Modify: `docs/DEVELOPMENT.md` (Mock mode 43-55, the terminal section 57-89, Live probes 91-115)
- Modify: `SECURITY.md` (Out of scope, 41-47)
- `docs/KEYBOARD_SHORTCUTS.md`: **no change. There is no new key.**

**Interfaces:**
- Consumes: the shipped surface from Tasks 1-7.
- Produces: documentation that states what is verified and labels what is not.

- [ ] **Step 1: README CLI surface** — replace the fenced block at 319-325 with:

```markdown
```
wattdeck --config PATH --debug --mock --live --device SERIAL --read-only --scenario NAME --no-update --init
wattdeck setup      # the wizard
wattdeck update     # check, verify, install, print what happened
wattdeck devices    # device list
wattdeck status     # one-shot summary
wattdeck export --format json
```
```

- [ ] **Step 2: README Install** — add after the paragraph that ends `…those targets are not built.`:

```markdown
Once installed, wattdeck checks for a newer release on launch: one GitHub API
request, the asset's SHA256 verified against `SHA256SUMS.txt` **before** anything
is extracted, installed over the binary you launched, and the app says
`restart to apply the new build` in the footer, in the Alerts screen, and in the
line it prints after you quit. `[update] enabled = false` or `wattdeck --no-update`
turns it off, and `wattdeck update` runs the same check with no TUI.
```

- [ ] **Step 3: README FAQ, three paragraphs rewritten verbatim**

`**Is it safe to run?**` — replace the last sentence:

```markdown
The first run on a fresh machine contacts nothing, and a mock run contacts nothing
at any time, whatever the config says. The launch check runs only in a run that is
not mock, it is one request to GitHub, and `wattdeck --no-update` turns it off.
```

`**Does it work offline?**` — add at the end of the paragraph:

```markdown
The launch check needs GitHub; offline it refuses by name, is traced, and the app
runs on the build you have.
```

`**Rate limits?**` — replace the paragraph:

```markdown
**Rate limits?** The installer makes one unauthenticated GitHub API request per
install to find the latest tag; GitHub's documented limit is 60 requests per hour
per IP, and `WATTDECK_TAG` + `WATTDECK_RELEASE_DIR` let you skip that request
entirely. The launch check makes the same single request per launch, against the
same limit: `[update] enabled = false` or `--no-update` skips it, and the check is
skipped in mock mode. The app's request rate to the EcoFlow API is the configured
refresh interval and the MQTT subscription — the app does not poll faster than you
set.
```

- [ ] **Step 4: README Troubleshooting** — replace the `no latest release` bullet and add the `tar` bullet:

```markdown
- **`no latest release` from the installer or the launch check**: GitHub's rate
  limit, and the escape hatch is `WATTDECK_TAG` + `WATTDECK_RELEASE_DIR` for the
  installer, `[update] enabled = false` or `--no-update` for the app.
- **`no tar found; install the binary from source`**: the launch check extracts the
  release asset with the system `tar` (`tar.exe` ships in `System32` on Windows
  10 and later). Build from source, or use `install.sh`, which uses `tar` the same
  way.
```

- [ ] **Step 5: `docs/ARCHITECTURE.md`** — in the shape diagram, add the third source line, so the diagram reads:

```markdown
```
  MQTT broker ──► MqttProvider ─┐
                                ├─► mpsc<Update> (256) ─► App ─► ratatui render
  REST API    ──► LiveProvider ─┘                          ▲
                                                           │
  GitHub    ──► update task ───────────────────────────────┤
                                                           │
  SQLite    ◄── storage writer thread ◄─ mpsc<StorageCmd> ─┘ (64)
```
```

and add the bullet after the `secrets/` bullet:

```markdown
- `update/` (`src/update/mod.rs`, `src/update/install.rs`) is the launch check: one
  GitHub request, the asset verified against `SHA256SUMS.txt` before extraction,
  installed over the binary that is running. It never touches the UI; the result
  comes back as one `Update::Release`, and the footer, the Alerts screen and the
  printed restart line read the state `App` made from it.
```

- [ ] **Step 6: `docs/DEVELOPMENT.md`** — in `## Mock mode`, add:

```markdown
A mock run does no network, so the launch check is skipped in mock mode whatever
the config says. The one exception is test-only: a mock run does the check when
`WATTDECK_RELEASE_DIR` is set, because that path reads a local directory, so the
footer line and the Alerts entry are verifiable offline.
```

in `## The terminal, without a terminal`, add after the wizard capture paragraph:

```markdown
The same probe shows the launch check's surface, offline through the seams: copy
the release binary to a temp dir, put a `tar.gz` containing a marker binary and a
`SHA256SUMS.txt` in a second temp dir, then
`keyprobe.py 100 26 'zzzzzzzzzzzz4q' env HOME=… WATTDECK_RELEASE_DIR=… WATTDECK_TAG=v9.9.9 <copy>/wattdeck --mock`.
The frame after the twelve `z` presses carries `restart to apply 9.9.9` in the
footer, key `4` is the Alerts line `updated wattdeck 0.2.0 → 9.9.9 · restart to
apply`, and the frame after `q` is the stdout line printed once the terminal is
restored.
```

in `## Live probes`, add to the verified list once the live run is done (Step 8):

```markdown
- `wattdeck update` against the real latest-release endpoint, and the install over
  the installed binary
```

- [ ] **Step 7: `SECURITY.md`** — in `## Out of scope for this project`, add:

```markdown
The launch update check verifies the downloaded asset against the SHA256 published
in the release, extracts it with the system `tar`, and installs it into the
directory the binary already lives in: no sudo, no elevated write, no code signing,
and no verification beyond the published checksums.
```

- [ ] **Step 8: The full verify, in order**

```bash
make            # build, all tests, fmt --check, clippy
make test-install
make release
```

Then the PTY run:

```bash
mkdir -p /tmp/wattdeck-up/bin /tmp/wattdeck-up/marker /tmp/wattdeck-up/rel
cp target/release/wattdeck /tmp/wattdeck-up/bin/wattdeck
printf '#!/bin/sh\necho "wattdeck 9.9.9"\n' > /tmp/wattdeck-up/marker/wattdeck
chmod 755 /tmp/wattdeck-up/marker/wattdeck
tar -czf /tmp/wattdeck-up/rel/wattdeck-v9.9.9-x86_64-unknown-linux-gnu.tar.gz -C /tmp/wattdeck-up/marker wattdeck
sha256sum /tmp/wattdeck-up/rel/wattdeck-v9.9.9-x86_64-unknown-linux-gnu.tar.gz \
  | sed 's# .*#  wattdeck-v9.9.9-x86_64-unknown-linux-gnu.tar.gz#' > /tmp/wattdeck-up/rel/SHA256SUMS.txt
python3 tools/pty_probe.py 100 26 'zzzzzzzzzzzz4q' \
    env HOME=/tmp/wattdeck-up WATTDECK_RELEASE_DIR=/tmp/wattdeck-up/rel WATTDECK_TAG=v9.9.9 \
    /tmp/wattdeck-up/bin/wattdeck --mock
```

Expected in the capture: the footer line `restart to apply 9.9.9`; the Alerts screen line `updated wattdeck 0.2.0 → 9.9.9 · restart to apply`; the frame after `q` showing `updated wattdeck 0.2.0 → 9.9.9 · restart to apply the new build`. If a line is missing, stop and fix the code — do not weaken the assertion.

Then the live probe, the real GitHub endpoint:

```bash
./target/release/wattdeck update
```

Record what it printed. `wattdeck 0.2.0 is the latest build` is the expected answer while `v0.2.0` is the newest release; a newer tag means the install ran and the restart line printed. Run it a second time: the second run must print `wattdeck 0.2.0 is the latest build` against the real endpoint, and `wattdeck --version` must print the version the installed binary carries.

- [ ] **Step 9: Update the test count in `README.md`'s Status section** — `cargo test --lib` prints the count; replace `249 library tests` with the number the run printed, and add to the "Still unverified" paragraph:

```markdown
the Windows self-replace (the rename of a running `wattdeck.exe`, `bsdtar`'s zip
handling, the `.old` cleanup — CI compiles and tests the Windows target, and the
install test is skipped by name on Windows), and the GitHub rate-limit behaviour of
the launch check (one request per launch is the documented exposure; the rate-limit
refusal path is tested offline through the seams, not against a throttled IP).
```

- [ ] **Step 10: Clean up, then commit**

```bash
rm -rf /tmp/wattdeck-up
make            # the full verify, after the docs edits
git add README.md SECURITY.md docs/ARCHITECTURE.md docs/DEVELOPMENT.md docs/superpowers/plans/2026-11-12-auto-update.md
git commit -m "Auto-update docs: the launch check, its rate-limit exposure, the tar rule, and the unverified Windows self-replace labelled"
```

---

## Verification at the end of the plan

Every one of these is a command with output, not a claim:

1. `make` — build, all tests, `fmt --check`, clippy `-D warnings`
2. `make test-install` — the installer's shell test, unchanged
3. `cargo test --test update` — the install of a verified asset over a real binary, with `--version` reporting the marker's version; and the sums-mismatch refusal leaving the binary untouched
4. the PTY capture — footer line, Alerts line, restart line after quit, all offline
5. `wattdeck update` against the real GitHub endpoint, run twice
6. `wattdeck --no-update --debug --mock` touching no network: the log file contains no GitHub request

If any of them fails, the plan is not done.
