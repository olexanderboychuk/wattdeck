//! The launch check: latest release, SHA256 verify, install over the running
//! binary. The network lives here; the UI receives one `Update::Release`.

pub mod install;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;

use crate::config::settings::UpdateConfig;
use crate::services::Update;

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

/// The live check: the latest release, the asset's SHA256 verified against
/// `SHA256SUMS.txt`, then the install into `target`. `current` is the caller's
/// version — `env!("CARGO_PKG_VERSION")` — never read from the running binary.
pub async fn check(
    cfg: &UpdateConfig,
    current: &str,
    cache_dir: &Path,
    target: &Path,
    tx: Option<&mpsc::Sender<Update>>,
) -> Result<Status> {
    run_with_seams(cfg, current, cache_dir, target, tx, &seams()).await
}

async fn run_with_seams(
    cfg: &UpdateConfig,
    current: &str,
    cache_dir: &Path,
    target: &Path,
    tx: Option<&mpsc::Sender<Update>>,
    s: &Seams,
) -> Result<Status> {
    install::cleanup_stale(target);
    let tgt = self::target().map_err(anyhow::Error::msg)?;
    let tag = match (&s.tag, &s.release_dir) {
        (Some(tag), _) => tag.clone(),
        (None, Some(_)) => {
            return Err(anyhow!(
                "the release-dir seam needs WATTDECK_TAG; its assets carry no tag"
            ));
        }
        (None, None) => fetch_latest(cfg).await?,
    };
    // Compare before any download: the up-to-date case is one API request.
    if !is_newer(&tag, current) {
        return Ok(Status::UpToDate);
    }
    let asset = asset_name(&tag, &tgt);
    if let Some(tx) = tx {
        let _ = tx
            .send(Update::Release(Status::Available { to: tag.clone() }))
            .await;
    }
    let sums = read_sums(cfg, cache_dir, s, &tag).await?;
    let want = expected_hash(&sums, &asset)
        .ok_or_else(|| anyhow!("{asset} is not listed in SHA256SUMS.txt"))?;
    let bytes = read_asset(cfg, cache_dir, s, &tag, &asset).await?;
    let got = sha256_of(&bytes);
    if got != want {
        // The bytes on disk are never installed. The message names the asset and
        // both hashes.
        tracing::warn!("SHA256 mismatch for {asset}: expected {want}, got {got}");
        return Ok(Status::Failed(format!(
            "SHA256 mismatch for {asset}: expected {want}, got {got}"
        )));
    }
    let stage = cache_dir.join("stage");
    let staged = cache_dir.join(&asset);
    std::fs::create_dir_all(&stage)
        .map_err(|e| anyhow!("{} is not writable: {e}", cache_dir.display()))?;
    std::fs::write(&staged, &bytes)
        .map_err(|e| anyhow!("{} is not writable: {e}", cache_dir.display()))?;
    let bin = install::extract(&staged, &stage, tgt.binary).map_err(anyhow::Error::msg)?;
    install::install(&bin, target).map_err(anyhow::Error::msg)?;
    tracing::info!("updated wattdeck {current} to {tag}");
    let status = Status::Applied {
        from: current.to_string(),
        to: tag.strip_prefix('v').unwrap_or(&tag).to_string(),
    };
    if let Some(tx) = tx {
        let _ = tx.send(Update::Release(status.clone())).await;
    }
    Ok(status)
}

/// `releases/latest` answers unauthenticated, 60 requests per hour per IP. The
/// version is read out of the JSON; a body without `tag_name` is refused.
async fn fetch_latest(cfg: &UpdateConfig) -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        cfg.repo.trim_matches('/')
    );
    let body = http_get(cfg, &url).await?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    value["tag_name"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the GitHub release JSON has no tag_name"))
}

async fn read_sums(cfg: &UpdateConfig, cache_dir: &Path, s: &Seams, tag: &str) -> Result<String> {
    if let Some(dir) = &s.release_dir {
        return std::fs::read_to_string(dir.join("SHA256SUMS.txt"))
            .map_err(|e| anyhow!("{} has no SHA256SUMS.txt: {e}", dir.display()));
    }
    let url = asset_url(cfg, tag, "SHA256SUMS.txt");
    let bytes = http_get(cfg, &url).await?;
    cache_write(cache_dir, "SHA256SUMS.txt", &bytes)?;
    Ok(String::from_utf8(bytes)?)
}

async fn read_asset(
    cfg: &UpdateConfig,
    cache_dir: &Path,
    s: &Seams,
    tag: &str,
    asset: &str,
) -> Result<Vec<u8>> {
    if let Some(dir) = &s.release_dir {
        return std::fs::read(dir.join(asset))
            .map_err(|e| anyhow!("{} has no {asset}: {e}", dir.display()));
    }
    let bytes = http_get(cfg, &asset_url(cfg, tag, asset)).await?;
    cache_write(cache_dir, asset, &bytes)?;
    Ok(bytes)
}

fn asset_url(cfg: &UpdateConfig, tag: &str, name: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{tag}/{name}",
        cfg.repo.trim_matches('/')
    )
}

/// The one HTTP request shape: the configured timeout, the user agent, one
/// response body read in full.
async fn http_get(cfg: &UpdateConfig, url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_seconds))
        .build()?;
    let res = client
        .get(url)
        .header("User-Agent", "wattdeck")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    let status = res.status();
    if !status.is_success() {
        return Err(anyhow!("the GitHub request for {url} answered {status}"));
    }
    Ok(res.bytes().await?.to_vec())
}

fn cache_write(cache_dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| anyhow!("{} is not writable: {e}", cache_dir.display()))?;
    std::fs::write(cache_dir.join(name), bytes)
        .map_err(|e| anyhow!("{} is not writable: {e}", cache_dir.display()))?;
    Ok(())
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

        // The target the test replaces is a file in the temp dir, never the
        // running test binary.
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

    /// The live request, on the real endpoint. `current` is the package version,
    /// so the released version cannot be newer than it; the target is a temp file,
    /// so this test can never install anything. It is the check the launch does,
    /// against GitHub, once per test run.
    #[tokio::test]
    async fn the_live_check_of_the_released_version_is_one_request() {
        let dir = tempfile::tempdir().unwrap();
        let status = check(
            &UpdateConfig::default(),
            env!("CARGO_PKG_VERSION"),
            &dir.path().join("cache"),
            &dir.path().join("wattdeck"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(status, Status::UpToDate, "the released build is current");
    }
}
