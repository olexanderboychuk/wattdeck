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
        .env("WATTDECK_RELEASE_DIR", &rel)
        .env("WATTDECK_TAG", "v9.9.9")
        .env("HOME", dir.path())
        // The app's directories go in their own subdirectory, so the data dir the
        // app creates (`data/wattdeck`) is never the binary file this test runs.
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
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
    let shown = String::from_utf8_lossy(&again.stdout).trim().to_string();
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
    std::fs::write(
        rel.join("SHA256SUMS.txt"),
        "deadbeef  something-else.tar.gz\n",
    )
    .unwrap();

    let out = Command::new(&installed)
        .arg("update")
        .env("WATTDECK_RELEASE_DIR", &rel)
        .env("WATTDECK_TAG", "v9.9.9")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(0), "a refusal must fail: {all}");
    assert!(all.contains("update check failed: "), "refusal: {all}");
    assert!(
        all.contains("SHA256SUMS.txt"),
        "the sums file is named: {all}"
    );
    // The binary the run uses is untouched.
    let v = Command::new(&installed).arg("--version").output().unwrap();
    let shown = String::from_utf8_lossy(&v.stdout).trim().to_string();
    assert_eq!(
        shown,
        format!("wattdeck {}", env!("CARGO_PKG_VERSION")),
        "a refusal must not change the binary: {shown}"
    );
}
