//! Verified bytes to the binary's own location. No network.

use std::path::{Path, PathBuf};

/// Extract the verified archive with the system `tar` and return the binary.
/// Windows ships `tar.exe` (bsdtar), which reads a zip; that claim is labelled
/// unverified in the spec, because no Windows machine has run wattdeck.
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
        // An archive that extracts cleanly and carries no binary: the release job
        // packages `README.md` and `LICENSE` beside the binary, so a member is
        // present and `tar` succeeds.
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("README.md"), "the readme").unwrap();
        let asset = dir.path().join("asset.tar.gz");
        std::process::Command::new("tar")
            .args([
                "-czf",
                asset.to_str().unwrap(),
                "-C",
                docs.to_str().unwrap(),
                "README.md",
            ])
            .status()
            .expect("tar");
        let err = extract(&asset, &dir.path().join("stage"), "wattdeck").unwrap_err();
        assert!(
            err.contains("the archive did not contain wattdeck"),
            "{err}"
        );
    }

    #[test]
    fn the_stale_name_is_the_target_with_the_old_suffix() {
        assert_eq!(
            stale_path(std::path::Path::new("c:/wattdeck/wattdeck.exe")),
            PathBuf::from("c:/wattdeck/wattdeck.exe.old")
        );
    }
}
