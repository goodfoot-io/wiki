//! End-to-end integration tests for `wiki install --codex`.
//!
//! These tests drive the entire install flow — codex-home resolution,
//! archive extraction, skill/hook/config upsert, manifest, and backups —
//! through the public [`wiki::install::run_with_fetcher`] seam. A fixture
//! [`FixtureFetcher`] returns a hand-built in-memory zip so nothing touches
//! the network.
//!
//! Most tests pass `--codex-home` directly and thus do not mutate process
//! environment. The single `codex_home_env_fallback` test that exercises the
//! `$CODEX_HOME` fallback serialises env mutation with a file-local
//! [`Mutex`], because integration tests run in a separate test binary from
//! the unit tests in `install.rs` and therefore cannot share that module's
//! env lock.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::{Value as JsonValue, json};
use tempfile::TempDir;

use wiki::install::{
    ActionStatus, SourceFetcher, apply_install, extract_plugin_files, run_with_fetcher,
};

// ── Fixture archive helpers ─────────────────────────────────────────────────

/// Build an in-memory zip from `(name, contents)` entries.
fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut writer = zip::ZipWriter::new(cursor);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).expect("start_file");
            writer.write_all(data).expect("write_all");
        }
        writer.finish().expect("finish");
    }
    buf
}

/// A canonical good fixture archive that matches the layout the installer
/// expects: a single top-level directory, `plugins/wiki/**`, a SKILL.md with
/// `name: wiki` frontmatter, and one reference file.
fn good_archive() -> Vec<u8> {
    build_zip(&[
        ("wiki-main/README.md", b"readme"),
        (
            "wiki-main/plugins/wiki/.codex-plugin/plugin.json",
            br#"{"name":"wiki"}"#,
        ),
        ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
        (
            "wiki-main/plugins/wiki/skills/wiki/SKILL.md",
            b"---\nname: wiki\ndescription: test skill\n---\nbody\n",
        ),
        (
            "wiki-main/plugins/wiki/skills/wiki/references/maintenance.md",
            b"maintenance reference body",
        ),
    ])
}

/// A variant of [`good_archive`] with different SKILL.md contents so that
/// rerun tests observe a real update rather than a no-op.
fn good_archive_updated() -> Vec<u8> {
    build_zip(&[
        ("wiki-main/README.md", b"readme"),
        (
            "wiki-main/plugins/wiki/.codex-plugin/plugin.json",
            br#"{"name":"wiki"}"#,
        ),
        ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
        (
            "wiki-main/plugins/wiki/skills/wiki/SKILL.md",
            b"---\nname: wiki\ndescription: updated\n---\nnew body\n",
        ),
        (
            "wiki-main/plugins/wiki/skills/wiki/references/maintenance.md",
            b"maintenance reference body",
        ),
    ])
}

/// A fixture that is missing `SKILL.md` to exercise the validation path.
fn archive_missing_skill_md() -> Vec<u8> {
    build_zip(&[
        ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
        (
            "wiki-main/plugins/wiki/skills/wiki/references/maintenance.md",
            b"ref",
        ),
    ])
}

/// A [`SourceFetcher`] that returns fixed bytes and never performs I/O.
struct FixtureFetcher {
    bytes: Vec<u8>,
}

impl FixtureFetcher {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl SourceFetcher for FixtureFetcher {
    fn fetch_archive(&self, _git_ref: &str) -> miette::Result<Vec<u8>> {
        Ok(self.bytes.clone())
    }
}

/// A fetcher that panics if called. Used to prove `--dry-run` performs no
/// network fetch even when given a failing fetcher.
struct ExplodingFetcher;

impl SourceFetcher for ExplodingFetcher {
    fn fetch_archive(&self, _git_ref: &str) -> miette::Result<Vec<u8>> {
        panic!("fetcher must not be called in dry-run mode");
    }
}

// ── Assertion helpers ───────────────────────────────────────────────────────

fn read_json(path: &Path) -> JsonValue {
    let body = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&body).expect("parse json")
}

fn assert_install_artifacts(home: &Path) {
    assert!(home.join("skills/wiki/SKILL.md").is_file());
    assert!(home.join("skills/wiki/references/maintenance.md").is_file());
    assert!(home.join("skills/wiki/.wiki-install-managed").is_file());
    assert!(home.join("hooks.json").is_file());
    assert!(home.join("config.toml").is_file());
    assert!(home.join(".wiki-install/manifest.json").is_file());
}

// ── Process-global env lock for CODEX_HOME tests ────────────────────────────

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Scoped mutation of `CODEX_HOME` (and optionally `HOME`). Restores previous
/// values on drop and holds a process-global [`Mutex`] so parallel tests
/// cannot race on `std::env`.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    prev_codex: Option<String>,
    prev_home: Option<String>,
}

impl EnvGuard {
    fn new() -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            _lock: lock,
            prev_codex: std::env::var("CODEX_HOME").ok(),
            prev_home: std::env::var("HOME").ok(),
        }
    }

    fn set_codex_home(&self, value: &str) {
        // SAFETY: env mutation is serialized by the process-global lock above.
        unsafe { std::env::set_var("CODEX_HOME", value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: env mutation is serialized by the process-global lock above.
        unsafe {
            match &self.prev_codex {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
            match &self.prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

// ── Happy path / idempotency ────────────────────────────────────────────────

#[test]
fn fresh_install_via_codex_home_flag_writes_all_artifacts() {
    let codex = TempDir::new().unwrap();
    let fetcher = FixtureFetcher::new(good_archive());

    let code =
        run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("run");
    assert_eq!(code, 0);

    assert_install_artifacts(codex.path());

    // Skill body came from the fixture archive, not a prior on-disk tree.
    let skill = std::fs::read_to_string(codex.path().join("skills/wiki/SKILL.md")).unwrap();
    assert!(skill.contains("description: test skill"));
    let refs = std::fs::read_to_string(codex.path().join("skills/wiki/references/maintenance.md"))
        .unwrap();
    assert_eq!(refs, "maintenance reference body");
}

#[test]
fn fresh_install_writes_hooks_json_with_codex_command() {
    let codex = TempDir::new().unwrap();
    let fetcher = FixtureFetcher::new(good_archive());

    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("run");

    let hooks = read_json(&codex.path().join("hooks.json"));
    let post = hooks["hooks"]["PostToolUse"].as_array().expect("array");
    assert_eq!(post.len(), 1);
    assert_eq!(
        post[0]["hooks"][0]["command"].as_str(),
        Some("wiki hook --codex")
    );
    assert_eq!(
        post[0]["_wikiInstallId"].as_str(),
        Some("goodfoot-wiki-post-tool-use")
    );
    assert_eq!(post[0]["matcher"].as_str(), Some("Read|Bash"));
}

#[test]
fn fresh_install_enables_codex_hooks_feature_flag() {
    let codex = TempDir::new().unwrap();
    let fetcher = FixtureFetcher::new(good_archive());

    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("run");

    let body = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
    assert!(body.contains("[features]"));
    assert!(body.contains("codex_hooks = true"));
}

#[test]
fn install_preserves_unrelated_config_toml_keys() {
    let codex = TempDir::new().unwrap();
    let original =
        "# user comment\nmodel = \"gpt-5\"\n[other]\nkeep = 1\n\n[features]\nother_flag = true\n";
    std::fs::write(codex.path().join("config.toml"), original).unwrap();

    let fetcher = FixtureFetcher::new(good_archive());
    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("run");

    let body = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
    assert!(body.contains("# user comment"));
    assert!(body.contains("model = \"gpt-5\""));
    assert!(body.contains("keep = 1"));
    assert!(body.contains("other_flag = true"));
    assert!(body.contains("codex_hooks = true"));
}

#[test]
fn install_preserves_unrelated_hook_groups_and_events() {
    let codex = TempDir::new().unwrap();
    let existing = json!({
        "hooks": {
            "PreToolUse": [
                {"matcher": "Write", "hooks": [{"type": "command", "command": "echo pre"}]}
            ],
            "PostToolUse": [
                {"matcher": "Edit", "hooks": [{"type": "command", "command": "echo other"}]}
            ]
        },
        "otherTopLevel": {"keep": true}
    });
    std::fs::write(
        codex.path().join("hooks.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let fetcher = FixtureFetcher::new(good_archive());
    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("run");

    let parsed = read_json(&codex.path().join("hooks.json"));
    assert_eq!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str(),
        Some("echo pre")
    );
    assert_eq!(parsed["otherTopLevel"]["keep"].as_bool(), Some(true));

    let post = parsed["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2);
    let managed_count = post
        .iter()
        .filter(|g| {
            g.get("_wikiInstallId").and_then(|v| v.as_str()) == Some("goodfoot-wiki-post-tool-use")
        })
        .count();
    assert_eq!(managed_count, 1);
    let other_count = post
        .iter()
        .filter(|g| g.get("matcher").and_then(|v| v.as_str()) == Some("Edit"))
        .count();
    assert_eq!(other_count, 1);
}

#[test]
fn rerun_replaces_managed_hook_group_without_duplicating() {
    let codex = TempDir::new().unwrap();
    let fetcher = FixtureFetcher::new(good_archive());

    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("first");
    run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).expect("rerun");

    let parsed = read_json(&codex.path().join("hooks.json"));
    let post = parsed["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1, "rerun must not duplicate the managed group");
    assert_eq!(
        post[0]["_wikiInstallId"].as_str(),
        Some("goodfoot-wiki-post-tool-use")
    );
}

#[test]
fn rerun_updates_managed_skill_without_stale_files() {
    let codex = TempDir::new().unwrap();

    // First install: fixture archive contains an extra stale file.
    let first_archive = build_zip(&[
        (
            "wiki-main/plugins/wiki/skills/wiki/SKILL.md",
            b"---\nname: wiki\n---\noriginal\n",
        ),
        (
            "wiki-main/plugins/wiki/skills/wiki/references/maintenance.md",
            b"ref",
        ),
        (
            "wiki-main/plugins/wiki/skills/wiki/references/stale.md",
            b"will be removed on rerun",
        ),
    ]);
    let first = FixtureFetcher::new(first_archive);
    run_with_fetcher(true, false, false, Some(codex.path()), "main", &first).expect("first");
    assert!(
        codex
            .path()
            .join("skills/wiki/references/stale.md")
            .exists()
    );

    let second = FixtureFetcher::new(good_archive_updated());
    run_with_fetcher(true, false, false, Some(codex.path()), "main", &second).expect("rerun");

    let body = std::fs::read_to_string(codex.path().join("skills/wiki/SKILL.md")).unwrap();
    assert!(body.contains("new body"));
    // Stale file from the first install must be gone.
    assert!(
        !codex
            .path()
            .join("skills/wiki/references/stale.md")
            .exists(),
        "rerun left stale file"
    );
    // The second fixture's reference file is present.
    assert!(
        codex
            .path()
            .join("skills/wiki/references/maintenance.md")
            .is_file()
    );
}

#[test]
fn manifest_contains_installer_source_ref_files_and_hook_id() {
    let codex = TempDir::new().unwrap();
    let fetcher = FixtureFetcher::new(good_archive());

    run_with_fetcher(true, false, false, Some(codex.path()), "v1.2.3", &fetcher).expect("run");

    let manifest = read_json(&codex.path().join(".wiki-install/manifest.json"));
    assert_eq!(manifest["installer"].as_str(), Some("wiki install --codex"));
    assert_eq!(
        manifest["source"].as_str(),
        Some("https://github.com/goodfoot-io/wiki")
    );
    assert_eq!(manifest["ref"].as_str(), Some("v1.2.3"));
    assert_eq!(
        manifest["hookId"].as_str(),
        Some("goodfoot-wiki-post-tool-use")
    );
    let files: Vec<String> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(files.contains(&"skills/wiki/SKILL.md".to_string()));
    assert!(files.contains(&"skills/wiki/references/maintenance.md".to_string()));
    assert!(files.contains(&"hooks.json".to_string()));
    assert!(files.contains(&"config.toml".to_string()));
}

#[test]
fn codex_home_env_fallback_is_used_when_flag_absent() {
    let guard = EnvGuard::new();
    let codex = TempDir::new().unwrap();
    guard.set_codex_home(codex.path().to_str().unwrap());

    let fetcher = FixtureFetcher::new(good_archive());
    // Pass None for --codex-home so resolution falls through to $CODEX_HOME.
    run_with_fetcher(true, false, false, None, "main", &fetcher).expect("run");

    assert_install_artifacts(codex.path());
}

// ── Failure paths ───────────────────────────────────────────────────────────

#[test]
fn unmanaged_skill_without_force_fails_and_leaves_dir_unchanged() {
    let codex = TempDir::new().unwrap();
    let target = codex.path().join("skills/wiki");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("README.md"), "user-authored").unwrap();

    let fetcher = FixtureFetcher::new(good_archive());
    let err =
        run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).unwrap_err();
    assert!(err.to_string().contains("not managed"), "got: {err}");

    // User-authored file still there, no managed marker, no manifest.
    assert!(target.join("README.md").is_file());
    assert!(!target.join(".wiki-install-managed").exists());
    assert!(!target.join("SKILL.md").exists());
}

#[test]
fn force_backs_up_unmanaged_skill_and_replaces_it() {
    let codex = TempDir::new().unwrap();
    let target = codex.path().join("skills/wiki");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("README.md"), "user-authored").unwrap();

    let fetcher = FixtureFetcher::new(good_archive());
    run_with_fetcher(true, true, false, Some(codex.path()), "main", &fetcher).expect("run");

    // New skill in place, old file gone from target.
    assert!(target.join("SKILL.md").is_file());
    assert!(!target.join("README.md").exists());

    // Backup directory under .wiki-install/backups/ contains the user file.
    let backups_root = codex.path().join(".wiki-install/backups");
    let mut found_backup: Option<PathBuf> = None;
    for entry in std::fs::read_dir(&backups_root).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("skills-wiki-")
        {
            found_backup = Some(entry.path());
            break;
        }
    }
    let backup = found_backup.expect("skills-wiki backup missing");
    assert!(
        backup.join("README.md").is_file(),
        "user file not preserved in backup"
    );
}

#[test]
fn dry_run_writes_nothing_and_does_not_fetch() {
    let codex = TempDir::new().unwrap();
    // Record a sentinel to prove the directory contents are untouched.
    std::fs::write(codex.path().join("sentinel"), "keep").unwrap();

    // ExplodingFetcher panics if called, proving dry-run short-circuits
    // before network fetch.
    run_with_fetcher(
        true,
        false,
        true,
        Some(codex.path()),
        "main",
        &ExplodingFetcher,
    )
    .expect("dry run");

    assert_eq!(
        std::fs::read_to_string(codex.path().join("sentinel")).unwrap(),
        "keep"
    );
    assert!(!codex.path().join("skills/wiki/SKILL.md").exists());
    assert!(!codex.path().join("hooks.json").exists());
    assert!(!codex.path().join("config.toml").exists());
    assert!(!codex.path().join(".wiki-install/manifest.json").exists());
}

#[test]
fn invalid_existing_hooks_json_fails_closed_without_touching_skill_files() {
    let codex = TempDir::new().unwrap();
    std::fs::write(codex.path().join("hooks.json"), "not json {").unwrap();

    let fetcher = FixtureFetcher::new(good_archive());
    let err =
        run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).unwrap_err();
    assert!(err.to_string().contains("not valid JSON"), "got: {err}");

    // Pre-flight validation must reject hooks.json before any files are
    // written to disk.
    assert!(!codex.path().join("skills/wiki/SKILL.md").exists());
    assert!(
        !codex
            .path()
            .join("skills/wiki/.wiki-install-managed")
            .exists()
    );
    assert!(!codex.path().join("config.toml").exists());
    assert!(!codex.path().join(".wiki-install/manifest.json").exists());
}

#[test]
fn missing_skill_md_in_archive_fails_without_touching_destination() {
    let codex = TempDir::new().unwrap();
    // Pre-populate an unrelated file to verify we do not touch the dir.
    std::fs::write(codex.path().join("sentinel"), "keep").unwrap();

    let fetcher = FixtureFetcher::new(archive_missing_skill_md());
    let err =
        run_with_fetcher(true, false, false, Some(codex.path()), "main", &fetcher).unwrap_err();
    assert!(
        err.to_string().contains("SKILL.md"),
        "expected SKILL.md error, got: {err}"
    );

    assert_eq!(
        std::fs::read_to_string(codex.path().join("sentinel")).unwrap(),
        "keep"
    );
    assert!(!codex.path().join("skills/wiki/SKILL.md").exists());
    assert!(!codex.path().join("hooks.json").exists());
    assert!(!codex.path().join("config.toml").exists());
    assert!(!codex.path().join(".wiki-install/manifest.json").exists());
}

// ── Sanity: the public seam for `apply_install`+`extract_plugin_files` is
// also directly usable by integration tests, mirroring how a future
// subcommand could bypass fetch. Keep this thin to avoid duplicating unit
// test coverage.
#[test]
fn apply_install_direct_seam_from_extracted_tree() {
    let codex = TempDir::new().unwrap();
    let staged = TempDir::new().unwrap();
    extract_plugin_files(&good_archive(), staged.path()).expect("extract");

    let summary = apply_install(codex.path(), staged.path(), "main", false).expect("apply_install");
    assert_eq!(summary.skill, ActionStatus::New);
    assert_eq!(summary.hooks, ActionStatus::New);
    assert_eq!(summary.config, ActionStatus::New);
    assert_install_artifacts(codex.path());
}
