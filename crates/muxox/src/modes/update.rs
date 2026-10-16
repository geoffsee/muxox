//! Explicit updates; ordinary launches only discover and display notices.

use anyhow::{Context, Result, bail, ensure};
use muxox_core::updates::{
    CURRENT_VERSION, CargoInstall, HANDOFF_EXIT, InstallContext, UPDATE_TIMEOUT, UpdateCandidate,
    capture_command, detect_install, http_client, latest_update,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

pub async fn run_update(standalone: bool) -> Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let context = detect_install(&executable)?;
    match &context {
        InstallContext::Unsupported(reason) => bail!("{reason}"),
        InstallContext::Untracked if !standalone => bail!(
            "Installation ownership is unknown. If you downloaded this executable directly, run `muxox update --standalone`. Otherwise use its original package manager."
        ),
        InstallContext::Untracked => {}
        _ if standalone => {
            bail!("--standalone cannot replace a package-manager-owned installation")
        }
        InstallContext::JavaScript { .. } if std::env::var_os("MUXOX_UPDATE_REQUEST").is_none() => {
            bail!(
                "Run `muxox update` through the JavaScript package's launcher so its package manager can safely replace the native executable."
            )
        }
        _ => {}
    }
    eprintln!("Checking for a newer stable Muxox release...");
    let Some(candidate) = latest_update(&context, UPDATE_TIMEOUT).await? else {
        eprintln!("Muxox {CURRENT_VERSION} is up to date for this installation.");
        return Ok(());
    };
    match context {
        InstallContext::Cargo { root, settings } => {
            eprintln!("Updating Muxox to {} with Cargo...", candidate.version);
            let mut command = cargo_command(&root, &settings, &candidate.version.to_string());
            #[cfg(windows)]
            let recovery = unlock_running_executable(&executable)?;
            let result = command
                .status()
                .await
                .context("Cannot run Cargo; install Cargo or update this installation manually");
            #[cfg(windows)]
            finish_windows_cargo(
                &executable,
                &recovery,
                result.as_ref().is_ok_and(|status| status.success()),
            )?;
            let status = result?;
            ensure!(
                status.success(),
                "Cargo update failed ({status}); see its output above"
            );
        }
        InstallContext::JavaScript { .. } => {
            // The parent must wait for this process to exit, especially on Windows.
            // Only a version is handed off; never executable names or shell commands.
            let request =
                std::env::var_os("MUXOX_UPDATE_REQUEST").context("Missing update handoff")?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(request)
                .context("Cannot create package-manager update request")?;
            serde_json::to_writer(
                &mut file,
                &serde_json::json!({
                    "schema": 1, "version": candidate.version.to_string(), "current_version": CURRENT_VERSION,
                }),
            )?;
            file.flush()?;
            std::process::exit(HANDOFF_EXIT);
        }
        InstallContext::Untracked => install_standalone(&executable, &candidate).await?,
        InstallContext::Unsupported(_) => unreachable!(),
    }
    eprintln!("Updated Muxox to {}.", candidate.version);
    Ok(())
}

fn cargo_command(root: &Path, settings: &CargoInstall, version: &str) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "install",
        "muxox",
        "--registry",
        "crates-io",
        "--version",
        version,
        "--force",
        "--root",
    ])
    .arg(root)
    .arg("--profile")
    .arg(&settings.profile);
    if settings.all_features {
        cmd.arg("--all-features");
    }
    if settings.no_default_features {
        cmd.arg("--no-default-features");
    }
    if !settings.features.is_empty() {
        cmd.arg("--features").arg(settings.features.join(","));
    }
    if let Some(target) = &settings.target {
        cmd.arg("--target").arg(target);
    }
    cmd
}

fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("muxox-x86_64-unknown-linux-gnu.tar.gz"),
        ("linux", "aarch64") => Ok("muxox-aarch64-unknown-linux-gnu.tar.gz"),
        ("macos", "x86_64") => Ok("muxox-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Ok("muxox-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Ok("muxox-x86_64-pc-windows-msvc.exe.zip"),
        (os, arch) => bail!("No standalone release for {os}/{arch}; install with Cargo"),
    }
}

fn verify_digest(bytes: &[u8], digest: Option<&str>) -> Result<()> {
    let expected = digest
        .and_then(|s| s.strip_prefix("sha256:"))
        .context("Release asset has no SHA-256 digest; refusing to install it")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
        "Invalid release checksum"
    );
    ensure!(
        format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(expected),
        "Release checksum mismatch; the installed executable was not changed"
    );
    Ok(())
}

fn extract_binary(bytes: &[u8], zipped: bool, output: &Path) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    const MAX_BINARY: u64 = 200 * 1024 * 1024;
    if zipped {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        let binary = archive
            .by_name("muxox.exe")
            .context("Release archive is missing muxox.exe")?;
        ensure!(
            binary.is_file() && binary.size() <= MAX_BINARY,
            "Invalid executable in release archive"
        );
        std::io::copy(&mut binary.take(MAX_BINARY + 1), &mut file)?;
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        let mut found = false;
        for entry in archive.entries()? {
            let entry = entry?;
            if entry.path()?.as_ref() != Path::new("muxox") {
                continue;
            }
            ensure!(
                !found && entry.header().entry_type().is_file() && entry.size() <= MAX_BINARY,
                "Invalid executable in release archive"
            );
            std::io::copy(&mut entry.take(MAX_BINARY + 1), &mut file)?;
            found = true;
        }
        ensure!(found, "Release archive is missing muxox");
    }
    ensure!(
        file.metadata()?.len() > 0 && file.metadata()?.len() <= MAX_BINARY,
        "Invalid release executable size"
    );
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

async fn install_standalone(executable: &Path, candidate: &UpdateCandidate) -> Result<()> {
    let release = candidate
        .release
        .as_ref()
        .context("Missing release metadata")?;
    let name = asset_name()?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .context("Release has no asset for this platform")?;
    let client = http_client(Duration::from_secs(120))?;
    eprintln!("Downloading Muxox {}...", candidate.version);
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 100 * 1024 * 1024,
            "Release download exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    verify_digest(&bytes, asset.digest.as_deref())?;
    // On the same filesystem, and permission failures happen before replacement.
    let dir = tempfile::Builder::new()
        .prefix(".muxox-update-")
        .tempdir_in(executable.parent().unwrap())?;
    let binary = dir
        .path()
        .join(if cfg!(windows) { "muxox.exe" } else { "muxox" });
    extract_binary(&bytes, name.ends_with(".zip"), &binary)?;
    let mut check = Command::new(&binary);
    check.arg("--version");
    let output = capture_command(check, Duration::from_secs(5))
        .await
        .context("Downloaded executable could not run")?;
    ensure!(
        String::from_utf8_lossy(&output).trim() == format!("muxox {}", candidate.version),
        "Downloaded executable version does not match release metadata"
    );
    let backup = dir.path().join("previous-muxox");
    std::fs::copy(executable, &backup)?;
    if let Err(error) = self_replace::self_replace(&binary) {
        // Windows self-replace may move the old executable before failing.
        if !executable.exists()
            && let Err(restore) = std::fs::copy(&backup, executable)
        {
            let recovery = dir.keep();
            bail!(
                "Update failed: {error}. Restore failed: {restore}. Original executable retained at {}",
                recovery.join("previous-muxox").display()
            );
        }
        return Err(error)
            .context("Could not replace the executable; the previous installation was retained");
    }
    Ok(())
}

#[cfg(windows)]
fn unlock_running_executable(executable: &Path) -> Result<std::path::PathBuf> {
    let placeholder = tempfile::Builder::new()
        .prefix(".muxox-running-")
        .suffix(".exe")
        .tempfile_in(executable.parent().unwrap())?;
    let path = placeholder.path().to_path_buf();
    placeholder.close()?;
    std::fs::rename(executable, &path)?;
    if let Err(error) = std::fs::copy(&path, executable) {
        // copy may have created an incomplete destination. Windows rename will
        // not overwrite it, so remove that copy before putting the live exe back.
        let _ = std::fs::remove_file(executable);
        if let Err(restore) = std::fs::rename(&path, executable) {
            bail!(
                "Cannot prepare Cargo update: {error}. Restore failed: {restore}. Original executable retained at {}",
                path.display()
            );
        }
        bail!("Cannot prepare Cargo update: {error}. The original executable was restored.");
    }
    Ok(path)
}

#[cfg(windows)]
fn finish_windows_cargo(executable: &Path, recovery: &Path, success: bool) -> Result<()> {
    if !executable.exists() {
        std::fs::copy(recovery, executable).with_context(|| {
            format!(
                "Restore the original executable from {}",
                recovery.display()
            )
        })?;
    }
    if let Err(error) = self_replace::self_delete_at(recovery) {
        eprintln!(
            "Could not schedule cleanup of {}: {error}. Remove it after Muxox exits.",
            recovery.display()
        );
    }
    if !success {
        eprintln!(
            "Cargo did not complete the update; the executable remains at {}.",
            executable.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_preserves_root_and_build_settings() {
        let settings = CargoInstall {
            bins: vec!["muxox".into()],
            features: vec!["foo".into(), "bar".into()],
            all_features: false,
            no_default_features: true,
            profile: "release".into(),
            target: Some("aarch64-apple-darwin".into()),
        };
        let command = cargo_command(Path::new("/custom root"), &settings, "2.0.0");
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "install",
                "muxox",
                "--registry",
                "crates-io",
                "--version",
                "2.0.0",
                "--force",
                "--root",
                "/custom root",
                "--profile",
                "release",
                "--no-default-features",
                "--features",
                "foo,bar",
                "--target",
                "aarch64-apple-darwin"
            ]
        );
    }

    #[test]
    fn rejects_missing_and_bad_checksums() {
        assert!(verify_digest(b"binary", None).is_err());
        assert!(verify_digest(b"binary", Some(&format!("sha256:{}", "0".repeat(64)))).is_err());
        assert!(
            verify_digest(
                b"binary",
                Some(&format!("sha256:{:x}", Sha256::digest(b"binary")))
            )
            .is_ok()
        );
    }

    #[test]
    fn extracts_only_expected_regular_executable() {
        let temp = tempfile::tempdir().unwrap();
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (name, bytes) in [
            ("ignored", b"ignored".as_slice()),
            ("muxox", b"binary".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let dest = temp.path().join("candidate");
        extract_binary(&bytes, false, &dest).unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"binary");
        assert!(!temp.path().join("ignored").exists());
    }

    #[test]
    fn refuses_archive_without_binary_and_symlink_binary() {
        let temp = tempfile::tempdir().unwrap();
        for symlink in [false, true] {
            let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
                Vec::new(),
                flate2::Compression::default(),
            ));
            if symlink {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                archive
                    .append_link(&mut header, "muxox", "/somewhere/else")
                    .unwrap();
            }
            let bytes = archive.into_inner().unwrap().finish().unwrap();
            assert!(
                extract_binary(
                    &bytes,
                    false,
                    &temp.path().join(format!("candidate-{symlink}"))
                )
                .is_err()
            );
        }
    }

    #[test]
    fn extracts_windows_zip_without_other_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("muxox.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"binary").unwrap();
        archive
            .start_file("../unwanted", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"ignored").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let dest = temp.path().join("candidate.exe");
        extract_binary(&bytes, true, &dest).unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"binary");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
