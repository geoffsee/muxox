//! Installation ownership and read-only update discovery, shared by every UI.

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::app::AppMsg;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_RELEASE: &str = "https://api.github.com/repos/geoffsee/muxox/releases/latest";
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(3);
pub const UPDATE_TIMEOUT: Duration = Duration::from_secs(15);
pub const HANDOFF_EXIT: i32 = 75;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateNotice {
    pub current_version: String,
    pub latest_version: String,
}

impl std::fmt::Display for UpdateNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Muxox {} is available (current: {}). Run `muxox update`.",
            self.latest_version, self.current_version
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CargoInstall {
    pub bins: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub all_features: bool,
    #[serde(default)]
    pub no_default_features: bool,
    pub profile: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub enum InstallContext {
    Cargo {
        root: PathBuf,
        settings: CargoInstall,
    },
    JavaScript {
        package_root: PathBuf,
        runtime: PathBuf,
    },
    Untracked,
    Unsupported(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
pub struct UpdateCandidate {
    pub version: Version,
    pub release: Option<Release>,
}

pub fn detect_install(executable: &Path) -> Result<InstallContext> {
    let executable = executable
        .canonicalize()
        .context("Cannot locate the running executable")?;
    let bin = executable
        .parent()
        .context("Executable has no parent directory")?;
    let root = bin
        .parent()
        .context("Executable has no installation root")?;

    // The package contains the native executable in bin/, including Yarn's
    // unplugged layout and pnpm's virtual store. Do not infer npm from PATH.
    if let Ok(contents) = std::fs::read(root.join("package.json"))
        && let Ok(package) = serde_json::from_slice::<serde_json::Value>(&contents)
        && package["name"] == "muxox"
    {
        let runtime = std::env::var_os("MUXOX_NODE_EXEC_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("node"));
        return Ok(InstallContext::JavaScript {
            package_root: root.into(),
            runtime,
        });
    }

    if bin.file_name().is_some_and(|name| name == "bin") {
        let metadata = root.join(".crates2.json");
        if metadata.exists() {
            #[derive(Deserialize)]
            struct Tracking {
                installs: std::collections::BTreeMap<String, CargoInstall>,
            }
            let tracking: Tracking = serde_json::from_slice(&std::fs::read(&metadata)?)
                .with_context(|| {
                    format!(
                        "Cannot read Cargo ownership metadata at {}",
                        metadata.display()
                    )
                })?;
            let name = executable.file_name().unwrap().to_string_lossy();
            let mut owners = tracking
                .installs
                .into_iter()
                .filter(|(_, settings)| settings.bins.iter().any(|bin| bin == name.as_ref()));
            if let Some((id, settings)) = owners.next() {
                if owners.next().is_some() {
                    bail!(
                        "More than one Cargo package claims this executable; update it with Cargo manually"
                    );
                }
                if id.starts_with("muxox ")
                    && (id.ends_with("(registry+https://github.com/rust-lang/crates.io-index)")
                        || id.ends_with("(sparse+https://index.crates.io/)"))
                {
                    return Ok(InstallContext::Cargo {
                        root: root.into(),
                        settings,
                    });
                }
                return Ok(InstallContext::Unsupported(format!(
                    "Cargo owns this executable ({id}). Update it with the original Cargo source and --root {}.",
                    root.display()
                )));
            }
            return Ok(InstallContext::Unsupported(format!(
                "Cargo metadata exists at {}, but ownership of this binary is unclear. Reinstall it with Cargo.",
                metadata.display()
            )));
        }
        if root.join(".crates.toml").exists() {
            return Ok(InstallContext::Unsupported(
                "This Cargo installation has legacy tracking metadata. Update it with Cargo to retain its build settings.".into()));
        }
    }

    let managed = executable.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("node_modules" | ".yarn" | ".pnpm" | "Cellar" | ".cargo" | ".rustup")
        )
    }) || executable.starts_with("/nix/store");
    if managed {
        return Ok(InstallContext::Unsupported(
            "This executable is in a managed installation, but its owner could not be identified. Use the original package manager.".into()));
    }
    Ok(InstallContext::Untracked)
}

pub fn newer_stable(current: &str, latest: &str) -> Result<Option<Version>> {
    let current = Version::parse(current.trim_start_matches('v'))?;
    let latest = Version::parse(latest.trim_start_matches('v'))?;
    Ok((latest.pre.is_empty() && latest.cmp_precedence(&current).is_gt()).then_some(latest))
}

pub fn http_client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!(
            "muxox/{CURRENT_VERSION} (https://github.com/geoffsee/muxox)"
        ))
        .https_only(true)
        .connect_timeout(Duration::from_secs(3))
        .timeout(timeout)
        .build()?)
}

async fn github_candidate(
    client: &reqwest::Client,
    url: &str,
    current: &str,
) -> Result<Option<UpdateCandidate>> {
    let release: Release = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    Ok(
        newer_stable(current, &release.tag_name)?.map(|version| UpdateCandidate {
            version,
            release: Some(release),
        }),
    )
}

pub async fn latest_update(
    context: &InstallContext,
    timeout: Duration,
) -> Result<Option<UpdateCandidate>> {
    match context {
        InstallContext::Untracked => {
            github_candidate(&http_client(timeout)?, GITHUB_RELEASE, CURRENT_VERSION).await
        }
        InstallContext::Cargo { .. } => {
            #[derive(Deserialize)]
            struct CrateVersion {
                max_stable_version: Option<String>,
            }
            #[derive(Deserialize)]
            struct Response {
                #[serde(rename = "crate")]
                package: CrateVersion,
            }
            let response: Response = http_client(timeout)?
                .get("https://crates.io/api/v1/crates/muxox")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let version = response
                .package
                .max_stable_version
                .context("crates.io has no stable muxox version")?;
            Ok(
                newer_stable(CURRENT_VERSION, &version)?.map(|version| UpdateCandidate {
                    version,
                    release: None,
                }),
            )
        }
        InstallContext::JavaScript {
            package_root,
            runtime,
        } => {
            let mut cmd = Command::new(runtime);
            // canonicalize() produces verbatim paths on Windows (\\?\...),
            // which Node cannot resolve as a script entry point. Resolve the
            // helper relative to the package directory instead.
            cmd.arg("lib/manager.js")
                .arg("check")
                .current_dir(package_root);
            let output = capture_command(cmd, timeout).await?;
            #[derive(Deserialize)]
            struct Response {
                version: String,
            }
            let response: Response = serde_json::from_slice(&output)
                .context("Invalid package manager version response")?;
            Ok(
                newer_stable(CURRENT_VERSION, &response.version)?.map(|version| UpdateCandidate {
                    version,
                    release: None,
                }),
            )
        }
        InstallContext::Unsupported(reason) => bail!("{reason}"),
    }
}

/// A check is read-only and its failure never affects service startup.
pub fn spawn_update_check(tx: mpsc::UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let check = async {
            let context = detect_install(&std::env::current_exe()?)?;
            latest_update(&context, CHECK_TIMEOUT).await
        };
        if let Ok(Ok(Some(candidate))) = tokio::time::timeout(CHECK_TIMEOUT, check).await {
            let _ = tx.send(AppMsg::UpdateAvailable(UpdateNotice {
                current_version: CURRENT_VERSION.into(),
                latest_version: candidate.version.to_string(),
            }));
        }
    });
}

// The guard also runs when the enclosing startup deadline cancels this future.
struct QueryProcess {
    pid: u32,
    armed: bool,
}
impl Drop for QueryProcess {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(unix)]
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}

pub async fn capture_command(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let child = command
        .spawn()
        .context("Cannot start the installation's package manager")?;
    let mut guard = QueryProcess {
        pid: child.id().context("Package manager has no process ID")?,
        armed: true,
    };
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .context("Update check timed out")??;
    guard.armed = false;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_without_downgrades_or_build_metadata() {
        assert!(newer_stable("1.9.0", "v1.10.0").unwrap().is_some());
        for version in ["1.9.0", "1.8.9", "1.9.0+build", "2.0.0-rc.1"] {
            assert!(newer_stable("1.9.0", version).unwrap().is_none());
        }
        assert!(newer_stable("2.0.0-rc.1", "2.0.0").unwrap().is_some());
        assert!(newer_stable("1.0.0", "invalid").is_err());
    }

    #[test]
    fn ownership_uses_executable_location_and_cargo_tracking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let exe = dir.path().join("bin/muxox");
        std::fs::write(&exe, "fixture").unwrap();
        assert!(matches!(
            detect_install(&exe).unwrap(),
            InstallContext::Untracked
        ));
        let metadata = serde_json::json!({"installs": {
            "muxox 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)": {
                "bins": ["muxox"], "features": ["unix"], "all_features": false,
                "no_default_features": true, "profile": "release", "target": "aarch64-apple-darwin"
            }
        }});
        std::fs::write(dir.path().join(".crates2.json"), metadata.to_string()).unwrap();
        let InstallContext::Cargo { root, settings } = detect_install(&exe).unwrap() else {
            panic!("expected Cargo")
        };
        assert_eq!(root, dir.path().canonicalize().unwrap());
        assert_eq!(settings.features, ["unix"]);
        assert!(settings.no_default_features);
        std::fs::write(dir.path().join("package.json"), r#"{"name":"muxox"}"#).unwrap();
        assert!(matches!(
            detect_install(&exe).unwrap(),
            InstallContext::JavaScript { .. }
        ));
    }

    #[test]
    fn non_registry_cargo_and_managed_paths_cannot_self_replace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("node_modules/unknown");
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("muxox");
        std::fs::write(&exe, "fixture").unwrap();
        assert!(matches!(
            detect_install(&exe).unwrap(),
            InstallContext::Unsupported(_)
        ));
        let root = dir.path().join("custom-root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/muxox");
        std::fs::write(&exe, "fixture").unwrap();
        std::fs::write(root.join(".crates2.json"), r#"{"installs":{"muxox 1.0.0 (git+https://example.org/muxox)":{"bins":["muxox"],"profile":"release","target":null}}}"#).unwrap();
        assert!(matches!(
            detect_install(&exe).unwrap(),
            InstallContext::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn github_responses_are_validated_without_live_network() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for body in [
            r#"{"tag_name":"v99.0.0","draft":false,"prerelease":false,"assets":[]}"#,
            r#"{"tag_name":"v99.0.0","draft":true,"prerelease":false,"assets":[]}"#,
            "malformed",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let _ = socket.read(&mut buffer).await;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let result = github_candidate(
                &reqwest::Client::new(),
                &format!("http://{address}"),
                "1.0.0",
            )
            .await;
            if body == "malformed" {
                assert!(result.is_err());
            } else if body.contains("\"draft\":true") {
                assert!(result.unwrap().is_none());
            } else {
                assert_eq!(result.unwrap().unwrap().version.to_string(), "99.0.0");
            }
        }
    }

    #[tokio::test]
    async fn slow_and_offline_checks_are_bounded() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let start = std::time::Instant::now();
        assert!(
            github_candidate(&client, &format!("http://{address}"), "1.0.0")
                .await
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        server.abort();
        let _ = server.await;
        assert!(
            github_candidate(&client, &format!("http://{address}"), "1.0.0")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_query_kills_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "(sleep 0.3; touch \"$MUXOX_TEST_MARKER\") & wait"])
            .env("MUXOX_TEST_MARKER", &marker);
        assert!(
            capture_command(cmd, Duration::from_millis(50))
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!marker.exists(), "query descendants survived cancellation");
    }
}
