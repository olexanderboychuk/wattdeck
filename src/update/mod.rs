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
}
