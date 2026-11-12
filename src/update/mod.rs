//! The launch check: latest release, SHA256 verify, install over the running
//! binary. The network lives here; the UI receives one `Update::Release`.

pub mod install;

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
        release_dir: std::env::var("WATTDECK_RELEASE_DIR")
            .map(PathBuf::from)
            .ok(),
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
        let off = Seams {
            release_dir: None,
            tag: None,
        };
        let seams = Seams {
            release_dir: Some(std::path::PathBuf::from("/tmp/wattdeck-rel")),
            tag: Some("v9.9.9".to_string()),
        };
        assert!(launch_check(&cfg, false, false, &off));
        assert!(
            !launch_check(&cfg, true, false, &off),
            "--no-update forces it off"
        );
        assert!(
            !launch_check(&cfg, false, true, &off),
            "a mock run does no network"
        );
        assert!(
            launch_check(&cfg, false, true, &seams),
            "a mock run with the release-dir seam is offline by construction"
        );
        let off_cfg = UpdateConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!launch_check(&off_cfg, false, false, &seams));
    }

    #[test]
    fn the_three_built_targets_are_the_installers_table() {
        assert_eq!(
            target_for("linux", "x86_64").unwrap().triple,
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_for("macos", "aarch64").unwrap().triple,
            "aarch64-apple-darwin"
        );
        assert_eq!(
            target_for("windows", "x86_64").unwrap().ext,
            "zip",
            "the Windows asset is a zip, as the release job packages it"
        );
        let refused = target_for("linux", "aarch64").unwrap_err();
        assert!(
            refused.contains("no wattdeck build for linux on aarch64"),
            "{refused}"
        );
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
        assert_eq!(
            target_for("windows", "x86_64").unwrap().binary,
            "wattdeck.exe"
        );
    }

    #[test]
    fn the_sums_line_for_the_asset_is_the_first_field() {
        let sums = "aaa  wattdeck-v0.3.0-x86_64-unknown-linux-gnu.tar.gz\nbbb  SHA256SUMS.txt\n";
        assert_eq!(
            expected_hash(sums, "wattdeck-v0.3.0-x86_64-unknown-linux-gnu.tar.gz"),
            Some("aaa".to_string())
        );
        assert_eq!(
            expected_hash(sums, "SHA256SUMS.txt"),
            Some("bbb".to_string())
        );
        assert_eq!(
            expected_hash(sums, "wattdeck-v9.9.9-x86_64-unknown-linux-gnu.tar.gz"),
            None
        );
        // A line with a third field is not a sums line.
        assert_eq!(expected_hash("aaa  asset  extra\n", "asset"), None);
        assert_eq!(
            expected_hash("AAA  asset\n", "asset"),
            Some("aaa".to_string())
        );
    }

    #[test]
    fn the_hash_of_the_empty_input_is_the_published_one() {
        // The published SHA256 of zero bytes; the string is the value, not a claim.
        assert_eq!(
            sha256_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
