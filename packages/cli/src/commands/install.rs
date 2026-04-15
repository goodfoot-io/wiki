//! `wiki install` command.
//!
//! Phase 3 of the Codex install plan: resolves the Codex home directory,
//! downloads the repository archive through a testable [`SourceFetcher`]
//! trait, and extracts the managed plugin files into a temporary directory.
//! Actual installation (skill replacement, hook JSON upsert, `config.toml`
//! edits) is still deferred to later phases.

use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use miette::{IntoDiagnostic, Result, WrapErr, miette};

/// GitHub owner for the managed repository.
const SOURCE_OWNER: &str = "goodfoot-io";
/// GitHub repo for the managed repository.
const SOURCE_REPO: &str = "wiki";
/// User agent used when fetching archives from GitHub.
const USER_AGENT: &str = concat!("wiki-cli/", env!("CARGO_PKG_VERSION"));
/// Prefix under the archive's top-level directory where the plugin lives.
const PLUGIN_PREFIX: &str = "plugins/wiki/";

/// Abstraction over the source of a repository archive.
///
/// Production code uses [`GitHubFetcher`]; tests substitute a fixture bytes
/// fetcher so they never touch the network.
pub trait SourceFetcher {
    /// Fetch a `.zip` archive for the given git ref and return its bytes.
    fn fetch_archive(&self, git_ref: &str) -> Result<Vec<u8>>;
}

/// Production fetcher that downloads a repository archive from GitHub.
pub struct GitHubFetcher;

impl GitHubFetcher {
    /// Build the archive URL for a given ref.
    ///
    /// Factored out so the URL-building logic can be tested without touching
    /// the network.
    pub fn archive_url(git_ref: &str) -> String {
        format!("https://github.com/{SOURCE_OWNER}/{SOURCE_REPO}/archive/{git_ref}.zip")
    }
}

impl SourceFetcher for GitHubFetcher {
    fn fetch_archive(&self, git_ref: &str) -> Result<Vec<u8>> {
        let url = Self::archive_url(git_ref);
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .into_diagnostic()
            .wrap_err("wiki install: failed to build HTTP client")?;
        let response = client
            .get(&url)
            .send()
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(miette!("wiki install: GET {url} returned HTTP {status}"));
        }
        let bytes = response
            .bytes()
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read body from {url}"))?;
        Ok(bytes.to_vec())
    }
}

/// Summary of files produced by [`extract_plugin_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPlugin {
    /// The destination root under which the files were written.
    pub dest: PathBuf,
    /// Paths (relative to `dest`) that were successfully extracted.
    pub files: Vec<PathBuf>,
}

/// Whether a path under the plugin prefix should be extracted.
fn is_wanted_plugin_path(rel_in_plugin: &str) -> bool {
    if rel_in_plugin == "hooks/hooks.json" {
        return true;
    }
    if rel_in_plugin == ".codex-plugin/plugin.json" {
        return true;
    }
    if let Some(rest) = rel_in_plugin.strip_prefix("skills/wiki/") {
        return !rest.is_empty();
    }
    false
}

/// Validate a zip entry name and return its components.
///
/// Rejects absolute paths, drive prefixes, and `..` traversal (zip-slip).
fn safe_relative_components(name: &str) -> Result<Vec<String>> {
    if name.is_empty() {
        return Err(miette!("wiki install: empty path in archive"));
    }
    let path = Path::new(name);
    if path.is_absolute() || name.starts_with('/') || name.starts_with('\\') {
        return Err(miette!(
            "wiki install: absolute path in archive rejected: {name}"
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(os) => {
                let s = os
                    .to_str()
                    .ok_or_else(|| miette!("wiki install: non-UTF8 path in archive: {name}"))?;
                parts.push(s.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(miette!(
                    "wiki install: parent-directory traversal in archive rejected: {name}"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(miette!(
                    "wiki install: absolute path in archive rejected: {name}"
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(miette!("wiki install: empty path in archive"));
    }
    Ok(parts)
}

/// Extract the managed plugin files from a GitHub-style repository archive
/// into `dest`.
///
/// Expects the archive to contain exactly one top-level directory (GitHub
/// archives use `wiki-<ref>/...`). Only files under that directory matching
/// the plugin allowlist are extracted; the top-level and `plugins/wiki/`
/// prefix are stripped so the layout under `dest` matches the on-disk
/// layout consumed by later install phases.
pub fn extract_plugin_files(archive: &[u8], dest: &Path) -> Result<ExtractedPlugin> {
    let reader = Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(reader)
        .into_diagnostic()
        .wrap_err("wiki install: failed to open archive")?;

    // First pass: determine the unique top-level directory.
    let mut top_level: Option<String> = None;
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .into_diagnostic()
            .wrap_err("wiki install: failed to read archive entry")?;
        let name = entry.name().to_string();
        let parts = safe_relative_components(&name)?;
        let first = &parts[0];
        match &top_level {
            None => top_level = Some(first.clone()),
            Some(existing) if existing == first => {}
            Some(existing) => {
                return Err(miette!(
                    "wiki install: archive must contain exactly one top-level directory; found both {existing:?} and {first:?}"
                ));
            }
        }
    }
    let top_level =
        top_level.ok_or_else(|| miette!("wiki install: archive contains no entries"))?;

    // Second pass: extract wanted files.
    let mut files: Vec<PathBuf> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .into_diagnostic()
            .wrap_err("wiki install: failed to read archive entry")?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let parts = safe_relative_components(&name)?;
        if parts[0] != top_level {
            // First pass already guaranteed a single top-level, but be defensive.
            continue;
        }
        let rel_in_repo = parts[1..].join("/");
        let Some(rel_in_plugin) = rel_in_repo.strip_prefix(PLUGIN_PREFIX) else {
            continue;
        };
        if !is_wanted_plugin_path(rel_in_plugin) {
            continue;
        }

        let out_path = dest.join(rel_in_plugin);
        // Defence in depth: make sure the resolved output path stays under `dest`.
        if !out_path.starts_with(dest) {
            return Err(miette!(
                "wiki install: extracted path escapes destination: {}",
                out_path.display()
            ));
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .into_diagnostic()
                .wrap_err_with(|| format!("wiki install: failed to create {}", parent.display()))?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read archive entry {name}"))?;
        let mut file = std::fs::File::create(&out_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to create {}", out_path.display()))?;
        file.write_all(&buf)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to write {}", out_path.display()))?;
        files.push(PathBuf::from(rel_in_plugin));
    }

    files.sort();
    Ok(ExtractedPlugin {
        dest: dest.to_path_buf(),
        files,
    })
}

/// Resolve the Codex home directory.
///
/// Priority order:
/// 1. `cli_override` (from `--codex-home`).
/// 2. `$CODEX_HOME` environment variable.
/// 3. `$HOME/.codex`.
///
/// Fails closed with a clear error if none can be resolved.
pub fn resolve_codex_home(cli_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path.to_path_buf());
    }

    if let Ok(codex_home) = std::env::var("CODEX_HOME")
        && !codex_home.is_empty()
    {
        return Ok(PathBuf::from(codex_home));
    }

    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join(".codex"));
    }

    Err(miette!(
        "wiki install: cannot resolve Codex home: pass --codex-home, or set CODEX_HOME or HOME"
    ))
}

/// Run the `wiki install` command.
///
/// Fails closed unless `--codex` is provided — only the Codex integration is
/// supported initially. Phase 2: resolves Codex home and prints the planned
/// layout. With `--dry-run`, prints a dry-run report and exits without writing.
pub fn run(
    codex: bool,
    force: bool,
    dry_run: bool,
    codex_home: Option<&Path>,
    git_ref: &str,
) -> Result<i32> {
    if !codex {
        return Err(miette!(
            "wiki install: --codex is required (only the Codex integration is supported)"
        ));
    }

    let _ = force;

    let home = resolve_codex_home(codex_home)?;

    let planned: [&str; 5] = [
        "skills/wiki/SKILL.md",
        "skills/wiki/references/maintenance.md",
        "hooks.json",
        "config.toml",
        ".wiki-install/manifest.json",
    ];

    if dry_run {
        println!("wiki install --codex [DRY RUN]");
    } else {
        println!("wiki install --codex");
    }
    println!("  codex home: {}", home.display());
    println!("  ref:        {git_ref}");
    println!("  planned files:");
    for rel in planned {
        println!("    - {}/{}", home.display(), rel);
    }
    if dry_run {
        println!("dry run: no files written.");
        return Ok(0);
    }

    // Phase 3: fetch and extract to a temp directory; print the extracted
    // file list and exit. Later phases will actually install into `home`.
    let fetcher = GitHubFetcher;
    run_fetch_and_extract(&fetcher, git_ref)
}

/// Fetch the archive and extract the plugin files into a fresh temp
/// directory, printing the extracted file list. Kept separate from [`run`]
/// so the phase-3 fetch/extract path is unit-testable with a fake fetcher.
fn run_fetch_and_extract<F: SourceFetcher>(fetcher: &F, git_ref: &str) -> Result<i32> {
    let archive = fetcher.fetch_archive(git_ref)?;
    let temp = tempfile::Builder::new()
        .prefix("wiki-install-")
        .tempdir()
        .into_diagnostic()
        .wrap_err("wiki install: failed to create temp directory")?;
    let extracted = extract_plugin_files(&archive, temp.path())?;

    println!("wiki install --codex: fetched archive for ref {git_ref}");
    println!("  temp dir: {}", extracted.dest.display());
    println!("  extracted files:");
    for rel in &extracted.files {
        println!("    - {}", rel.display());
    }
    println!("install: later phases will write these files into Codex home.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

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
            let prev_codex = std::env::var("CODEX_HOME").ok();
            let prev_home = std::env::var("HOME").ok();
            Self {
                _lock: lock,
                prev_codex,
                prev_home,
            }
        }

        fn set_codex_home(&self, value: Option<&str>) {
            unsafe {
                match value {
                    Some(v) => std::env::set_var("CODEX_HOME", v),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
        }

        fn set_home(&self, value: Option<&str>) {
            unsafe {
                match value {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
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

    #[test]
    fn requires_codex_flag() {
        let err = run(false, false, false, None, "main").unwrap_err();
        assert!(err.to_string().contains("--codex is required"));
    }

    #[test]
    fn dry_run_prints_and_exits_zero() {
        let code = run(
            true,
            false,
            true,
            Some(Path::new("/tmp/codex-test-home")),
            "main",
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn resolves_from_cli_override() {
        let guard = EnvGuard::new();
        guard.set_codex_home(Some("/env/codex"));
        guard.set_home(Some("/env/home"));

        let override_path = PathBuf::from("/override/codex-home");
        let resolved = resolve_codex_home(Some(&override_path)).expect("resolve");
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn resolves_from_codex_home_env() {
        let guard = EnvGuard::new();
        guard.set_codex_home(Some("/env/codex"));
        guard.set_home(Some("/env/home"));

        let resolved = resolve_codex_home(None).expect("resolve");
        assert_eq!(resolved, PathBuf::from("/env/codex"));
    }

    #[test]
    fn resolves_from_home_when_codex_home_unset() {
        let guard = EnvGuard::new();
        guard.set_codex_home(None);
        guard.set_home(Some("/env/home"));

        let resolved = resolve_codex_home(None).expect("resolve");
        assert_eq!(resolved, PathBuf::from("/env/home/.codex"));
    }

    #[test]
    fn fails_when_nothing_set() {
        let guard = EnvGuard::new();
        guard.set_codex_home(None);
        guard.set_home(None);

        let err = resolve_codex_home(None).unwrap_err();
        assert!(err.to_string().contains("cannot resolve Codex home"));
    }

    // ── Phase 3 fetch/extract tests ───────────────────────────────────────

    /// Build an in-memory zip archive from `(name, contents)` entries.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
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

    #[test]
    fn github_fetcher_formats_url_for_ref() {
        assert_eq!(
            GitHubFetcher::archive_url("main"),
            "https://github.com/goodfoot-io/wiki/archive/main.zip"
        );
        assert_eq!(
            GitHubFetcher::archive_url("v1.2.3"),
            "https://github.com/goodfoot-io/wiki/archive/v1.2.3.zip"
        );
        assert_eq!(
            GitHubFetcher::archive_url("abcdef0"),
            "https://github.com/goodfoot-io/wiki/archive/abcdef0.zip"
        );
    }

    #[test]
    fn extract_plugin_files_extracts_expected_layout_and_filters_others() {
        let archive = build_zip(&[
            ("wiki-main/README.md", b"readme"),
            (
                "wiki-main/plugins/wiki/.codex-plugin/plugin.json",
                b"{\"name\":\"wiki\"}",
            ),
            ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
            (
                "wiki-main/plugins/wiki/skills/wiki/SKILL.md",
                b"---\nname: wiki\n---\n",
            ),
            (
                "wiki-main/plugins/wiki/skills/wiki/references/maintenance.md",
                b"notes",
            ),
            // Not in the allowlist — should be filtered out.
            ("wiki-main/plugins/wiki/skills/other/SKILL.md", b"nope"),
            ("wiki-main/plugins/wiki/AGENTS.md", b"nope"),
            ("wiki-main/packages/cli/Cargo.toml", b"nope"),
        ]);

        let temp = tempfile::tempdir().expect("tempdir");
        let extracted = extract_plugin_files(&archive, temp.path()).expect("extract");

        let got: Vec<String> = extracted
            .files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let want: Vec<String> = vec![
            ".codex-plugin/plugin.json".to_string(),
            "hooks/hooks.json".to_string(),
            "skills/wiki/SKILL.md".to_string(),
            "skills/wiki/references/maintenance.md".to_string(),
        ];
        let mut got_sorted = got.clone();
        got_sorted.sort();
        let mut want_sorted = want.clone();
        want_sorted.sort();
        assert_eq!(got_sorted, want_sorted, "extracted file list mismatch");

        // Verify contents on disk.
        assert_eq!(
            std::fs::read(temp.path().join("skills/wiki/SKILL.md")).expect("read skill"),
            b"---\nname: wiki\n---\n"
        );
        assert_eq!(
            std::fs::read(temp.path().join("hooks/hooks.json")).expect("read hooks"),
            b"{}"
        );
        assert_eq!(
            std::fs::read(temp.path().join(".codex-plugin/plugin.json")).expect("read plugin.json"),
            b"{\"name\":\"wiki\"}"
        );

        // Filtered files must not exist.
        assert!(!temp.path().join("skills/other/SKILL.md").exists());
        assert!(!temp.path().join("AGENTS.md").exists());
        assert!(!temp.path().join("README.md").exists());
    }

    #[test]
    fn extract_plugin_files_rejects_multiple_top_level_directories() {
        let archive = build_zip(&[
            ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
            ("other-root/plugins/wiki/hooks/hooks.json", b"{}"),
        ]);
        let temp = tempfile::tempdir().expect("tempdir");
        let err = extract_plugin_files(&archive, temp.path()).unwrap_err();
        assert!(
            err.to_string().contains("exactly one top-level directory"),
            "got: {err}"
        );
    }

    #[test]
    fn extract_plugin_files_rejects_parent_dir_zip_slip() {
        let archive = build_zip(&[
            ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
            ("wiki-main/../evil.txt", b"pwn"),
        ]);
        let temp = tempfile::tempdir().expect("tempdir");
        let err = extract_plugin_files(&archive, temp.path()).unwrap_err();
        assert!(
            err.to_string().contains("parent-directory traversal"),
            "got: {err}"
        );
    }

    #[test]
    fn extract_plugin_files_rejects_absolute_path_zip_slip() {
        let archive = build_zip(&[("/etc/passwd", b"pwn")]);
        let temp = tempfile::tempdir().expect("tempdir");
        let err = extract_plugin_files(&archive, temp.path()).unwrap_err();
        assert!(
            err.to_string().contains("absolute path in archive"),
            "got: {err}"
        );
    }

    struct FixtureFetcher {
        bytes: Vec<u8>,
    }

    impl SourceFetcher for FixtureFetcher {
        fn fetch_archive(&self, _git_ref: &str) -> Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }
    }

    #[test]
    fn run_fetch_and_extract_uses_injected_fetcher() {
        let archive = build_zip(&[
            (
                "wiki-main/plugins/wiki/skills/wiki/SKILL.md",
                b"---\nname: wiki\n---\n",
            ),
            ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
            (
                "wiki-main/plugins/wiki/.codex-plugin/plugin.json",
                b"{\"name\":\"wiki\"}",
            ),
        ]);
        let fetcher = FixtureFetcher { bytes: archive };
        let code = run_fetch_and_extract(&fetcher, "main").expect("run_fetch_and_extract");
        assert_eq!(code, 0);
    }
}
