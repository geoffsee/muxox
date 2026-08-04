// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;

use super::skill_assets::{AGENTS_OPENAI_YAML, MCP_TOOLS_MD, SETUP_MD, SKILL_MD};

const SKILL_FILES: &[(&str, &str)] = &[
    ("SKILL.md", SKILL_MD),
    ("references/mcp-tools.md", MCP_TOOLS_MD),
    ("references/setup.md", SETUP_MD),
    ("agents/openai.yaml", AGENTS_OPENAI_YAML),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SkillClient {
    All,
    Codex,
    Cursor,
    Claude,
    Vibe,
    Grok,
    Agy,
    Hermes,
    #[value(name = "opencode")]
    OpenCode,
    Cline,
}

impl SkillClient {
    fn all() -> &'static [Self] {
        &[
            Self::Codex,
            Self::Cursor,
            Self::Claude,
            Self::Vibe,
            Self::Grok,
            Self::Agy,
            Self::Hermes,
            Self::OpenCode,
            Self::Cline,
        ]
    }

    fn directory(self) -> &'static str {
        match self {
            Self::All => unreachable!("all is expanded before resolving a directory"),
            Self::Codex => ".codex/skills/muxox",
            Self::Cursor => ".cursor/skills/muxox",
            Self::Claude => ".claude/skills/muxox",
            Self::Vibe => ".vibe/skills/muxox",
            Self::Grok => ".grok/skills/muxox",
            Self::Agy => ".agents/skills/muxox",
            Self::Hermes => "skills/muxox",
            Self::OpenCode => ".opencode/skills/muxox",
            Self::Cline => ".cline/skills/muxox",
        }
    }
}

fn home_directory() -> Result<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable)
        .map(PathBuf::from)
        .with_context(|| format!("{variable} is not set; cannot locate global skill directories"))
}

pub fn run_install_skill(clients: &[SkillClient], dry_run: bool, force: bool) -> Result<()> {
    run_install_skill_at(&home_directory()?, clients, dry_run, force)
}

fn run_install_skill_at(
    home: &Path,
    clients: &[SkillClient],
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let selected: Vec<SkillClient> = if clients.iter().any(|client| *client == SkillClient::All) {
        SkillClient::all().to_vec()
    } else {
        let mut selected = Vec::new();
        for client in clients {
            if !selected.contains(client) {
                selected.push(*client);
            }
        }
        selected
    };

    if selected.is_empty() {
        bail!("at least one client must be selected");
    }

    for client in selected {
        let destination = home.join(client.directory());
        for (relative_path, contents) in SKILL_FILES {
            let path = destination.join(relative_path);

            if path.exists() {
                let existing = fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                if existing == *contents {
                    println!("Up to date {}", path.display());
                    continue;
                }
                if !force {
                    println!("Skipped {} (use --force to replace)", path.display());
                    continue;
                }
            }

            if dry_run {
                println!("Would install {}", path.display());
                continue;
            }

            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
            println!("Installed {}", path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TestHome {
        path: PathBuf,
    }

    impl TestHome {
        fn new(label: &str) -> Self {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "muxox_skill_{label}_{}_{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn assert_client_tree(home: &Path, client: SkillClient) {
        let destination = home.join(client.directory());
        for (relative_path, contents) in SKILL_FILES {
            let path = destination.join(relative_path);
            assert_eq!(
                fs::read_to_string(&path)
                    .unwrap_or_else(|err| panic!("read {}: {err}", path.display())),
                *contents,
                "unexpected contents at {}",
                path.display()
            );
        }
    }

    #[test]
    fn installs_all_clients_and_is_idempotent() {
        let home = TestHome::new("all");

        run_install_skill_at(home.path(), &[SkillClient::All], false, false).unwrap();
        for client in SkillClient::all() {
            assert_client_tree(home.path(), *client);
        }
        assert_eq!(
            home.path().join(SkillClient::Hermes.directory()),
            home.path().join("skills/muxox")
        );

        // Second run leaves every file unchanged (up-to-date short-circuit).
        run_install_skill_at(home.path(), &[SkillClient::All], false, false).unwrap();
        for client in SkillClient::all() {
            assert_client_tree(home.path(), *client);
        }
    }

    #[test]
    fn does_not_overwrite_without_force() {
        let home = TestHome::new("skip");
        let path = home.path().join(".codex/skills/muxox/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "user version").unwrap();

        run_install_skill_at(home.path(), &[SkillClient::Codex], false, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "user version");

        run_install_skill_at(home.path(), &[SkillClient::Codex], false, true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), SKILL_MD);
        assert_client_tree(home.path(), SkillClient::Codex);
    }

    #[test]
    fn partial_tree_preserves_diverged_file_and_installs_missing() {
        let home = TestHome::new("partial");
        let skill_md = home.path().join(".codex/skills/muxox/SKILL.md");
        fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        fs::write(&skill_md, "user version").unwrap();

        run_install_skill_at(home.path(), &[SkillClient::Codex], false, false).unwrap();

        assert_eq!(fs::read_to_string(&skill_md).unwrap(), "user version");
        for (relative_path, contents) in SKILL_FILES.iter().skip(1) {
            let path = home
                .path()
                .join(SkillClient::Codex.directory())
                .join(relative_path);
            assert_eq!(fs::read_to_string(&path).unwrap(), *contents);
        }
    }

    #[test]
    fn dry_run_does_not_write_files() {
        let home = TestHome::new("dry");

        run_install_skill_at(home.path(), &[SkillClient::Codex], true, false).unwrap();

        let destination = home.path().join(SkillClient::Codex.directory());
        assert!(!destination.exists(), "dry-run must not create skill tree");
        for (relative_path, _) in SKILL_FILES {
            assert!(!destination.join(relative_path).exists());
        }
    }

    #[test]
    fn dry_run_skips_existing_diverged_file_without_writing() {
        let home = TestHome::new("dry_skip");
        let path = home.path().join(".codex/skills/muxox/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "user version").unwrap();

        run_install_skill_at(home.path(), &[SkillClient::Codex], true, false).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "user version");
        // Missing files are not created under dry-run either.
        assert!(
            !home
                .path()
                .join(".codex/skills/muxox/references/mcp-tools.md")
                .exists()
        );
    }

    #[test]
    fn empty_client_list_errors() {
        let home = TestHome::new("empty");
        let err = run_install_skill_at(home.path(), &[], false, false).unwrap_err();
        assert!(
            err.to_string().contains("at least one client"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn all_plus_specific_client_installs_full_set() {
        let home = TestHome::new("all_plus");
        run_install_skill_at(
            home.path(),
            &[SkillClient::All, SkillClient::Codex],
            false,
            false,
        )
        .unwrap();
        for client in SkillClient::all() {
            assert_client_tree(home.path(), *client);
        }
    }

    #[test]
    fn selected_clients_only_install_those_roots() {
        let home = TestHome::new("subset");
        run_install_skill_at(
            home.path(),
            &[SkillClient::Codex, SkillClient::Cursor, SkillClient::Codex],
            false,
            false,
        )
        .unwrap();

        assert_client_tree(home.path(), SkillClient::Codex);
        assert_client_tree(home.path(), SkillClient::Cursor);
        assert!(!home.path().join(SkillClient::Claude.directory()).exists());
        assert!(!home.path().join(SkillClient::Hermes.directory()).exists());
    }

    #[test]
    fn force_replaces_full_tree_when_all_files_diverge() {
        let home = TestHome::new("force_tree");
        let destination = home.path().join(SkillClient::Codex.directory());
        for (relative_path, _) in SKILL_FILES {
            let path = destination.join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "stale").unwrap();
        }

        run_install_skill_at(home.path(), &[SkillClient::Codex], false, true).unwrap();
        assert_client_tree(home.path(), SkillClient::Codex);
    }

    #[test]
    fn force_with_matching_content_is_up_to_date_without_rewrite() {
        let home = TestHome::new("force_match");
        run_install_skill_at(home.path(), &[SkillClient::Codex], false, false).unwrap();
        let path = home.path().join(".codex/skills/muxox/SKILL.md");
        let before = fs::metadata(&path).unwrap().modified().unwrap();

        // Ensure mtime can move if a rewrite happened.
        std::thread::sleep(std::time::Duration::from_millis(20));
        run_install_skill_at(home.path(), &[SkillClient::Codex], false, true).unwrap();

        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "matching content should not be rewritten");
        assert_client_tree(home.path(), SkillClient::Codex);
    }

    #[test]
    fn directory_at_file_path_errors() {
        let home = TestHome::new("dir_file");
        let path = home.path().join(".codex/skills/muxox/SKILL.md");
        fs::create_dir_all(&path).unwrap();

        let err = run_install_skill_at(home.path(), &[SkillClient::Codex], false, false)
            .expect_err("directory where a file is expected must error");
        let message = format!("{err:#}");
        assert!(
            message.contains("reading") || message.contains("Is a directory"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn create_dir_all_failure_when_parent_is_file() {
        let home = TestHome::new("parent_file");
        // `.codex` as a file blocks creating `.codex/skills/muxox/...`.
        let blocker = home.path().join(".codex");
        fs::write(&blocker, "not a directory").unwrap();

        let err = run_install_skill_at(home.path(), &[SkillClient::Codex], false, false)
            .expect_err("file blocking destination parent must error");
        let message = format!("{err:#}");
        assert!(
            message.contains("creating"),
            "unexpected error: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_existing_file_errors() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestHome::new("unreadable");
        let path = home.path().join(".codex/skills/muxox/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "secret").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&path, perms).unwrap();

        let result = run_install_skill_at(home.path(), &[SkillClient::Codex], false, false);

        // Restore permissions so Drop can clean up.
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        let err = result.expect_err("unreadable existing file must error");
        let message = format!("{err:#}");
        assert!(
            message.contains("reading"),
            "unexpected error: {message}"
        );
    }
}
