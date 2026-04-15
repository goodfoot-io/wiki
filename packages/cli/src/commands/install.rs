//! `wiki install` command.
//!
//! Phases 1–6 of the Codex install plan:
//!
//! 1. Clap wiring and subcommand scaffold.
//! 2. Codex-home resolution and dry-run output.
//! 3. Archive fetch + extraction behind the [`SourceFetcher`] trait.
//! 4. Atomic managed skill directory replacement with marker file.
//! 5. `hooks.json` upsert for the managed `PostToolUse` group.
//! 6. `config.toml` feature-flag edit to enable `codex_hooks`.
//!
//! Cross-cutting: backups under `.wiki-install/backups/`, manifest under
//! `.wiki-install/manifest.json`, fail-closed conflict handling, and a
//! per-file summary printed at the end.

use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::{Value as JsonValue, json};
use toml_edit::{DocumentMut, Item, Table, value as toml_value};

/// GitHub owner for the managed repository.
const SOURCE_OWNER: &str = "goodfoot-io";
/// GitHub repo for the managed repository.
const SOURCE_REPO: &str = "wiki";
/// Source URL recorded in the manifest.
const SOURCE_URL: &str = "https://github.com/goodfoot-io/wiki";
/// User agent used when fetching archives from GitHub.
const USER_AGENT: &str = concat!("wiki-cli/", env!("CARGO_PKG_VERSION"));
/// Prefix under the archive's top-level directory where the plugin lives.
const PLUGIN_PREFIX: &str = "plugins/wiki/";
/// Hook install id used to identify the managed `PostToolUse` group in
/// `hooks.json`.
const HOOK_INSTALL_ID: &str = "goodfoot-wiki-post-tool-use";
/// Marker file dropped inside a managed skill directory.
const MANAGED_MARKER: &str = ".wiki-install-managed";
/// Command this installer writes into the managed hook.
const HOOK_COMMAND: &str = "wiki hook --codex";
/// Legacy command that earlier installs may have written; recognised for
/// upsert and converted to [`HOOK_COMMAND`].
const HOOK_COMMAND_LEGACY: &str = "wiki hook --claude";

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

// ── Cross-cutting helpers ────────────────────────────────────────────────────

/// Outcome of a per-target install action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    /// A brand-new file/directory was written.
    New,
    /// An existing managed target was replaced.
    Updated,
    /// The target already matched the desired state.
    Unchanged,
}

impl ActionStatus {
    fn label(self) -> &'static str {
        match self {
            ActionStatus::New => "new",
            ActionStatus::Updated => "updated",
            ActionStatus::Unchanged => "unchanged",
        }
    }
}

/// Summary of the three install actions, returned by [`apply_install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSummary {
    pub skill: ActionStatus,
    pub hooks: ActionStatus,
    pub config: ActionStatus,
    /// Skill files written, relative to `$CODEX_HOME` (for the manifest).
    pub skill_files: Vec<String>,
    /// Backups created in this run, as absolute paths.
    pub backups: Vec<PathBuf>,
}

/// Compute a UTC RFC 3339 timestamp without pulling in a date crate.
///
/// Uses the Hinnant civil-from-days algorithm. Only intended for stamping
/// install artifacts and backup filenames.
fn rfc3339_utc(ts: SystemTime) -> String {
    let dur = ts
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0));
    let secs = dur.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;

    // Hinnant civil_from_days, adapted from
    // https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m_civil = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = y + i64::from(m_civil <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{h:02}:{min:02}:{sec:02}Z",
        year = year,
        month = m_civil,
        day = d,
        h = h,
        min = m,
        sec = s,
    )
}

/// Format a timestamp suitable for filesystem paths (no colons).
fn fs_timestamp(ts: SystemTime) -> String {
    rfc3339_utc(ts).replace(':', "-")
}

/// Ensure `.wiki-install/backups/` exists under `codex_home` and return it.
fn ensure_backup_dir(codex_home: &Path) -> Result<PathBuf> {
    let dir = codex_home.join(".wiki-install").join("backups");
    std::fs::create_dir_all(&dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Recursively copy `src` into `dst`. Creates `dst` if missing.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to create {}", dst.display()))?;
    for entry in std::fs::read_dir(src)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to read {}", src.display()))?
    {
        let entry = entry.into_diagnostic()?;
        let ft = entry.file_type().into_diagnostic()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)
                .into_diagnostic()
                .wrap_err_with(|| format!("wiki install: failed to copy {}", from.display()))?;
        }
    }
    Ok(())
}

/// Walk `root` and return file paths relative to `root`, sorted.
fn list_files_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    fn walk(root: &Path, cur: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(cur)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read {}", cur.display()))?
        {
            let entry = entry.into_diagnostic()?;
            let ft = entry.file_type().into_diagnostic()?;
            let path = entry.path();
            if ft.is_dir() {
                walk(root, &path, out)?;
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.push(rel);
            }
        }
        Ok(())
    }
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

// ── Phase 4: skill install ──────────────────────────────────────────────────

/// Validate that `staged_skill_root` contains a well-formed wiki skill with
/// `name: wiki` in its frontmatter.
fn validate_staged_skill(staged_skill_root: &Path) -> Result<()> {
    let skill_md = staged_skill_root.join("SKILL.md");
    if !skill_md.is_file() {
        return Err(miette!(
            "wiki install: extracted archive is missing skills/wiki/SKILL.md"
        ));
    }
    let body = std::fs::read_to_string(&skill_md)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to read {}", skill_md.display()))?;
    let frontmatter = parse_frontmatter(&body)
        .ok_or_else(|| miette!("wiki install: SKILL.md is missing YAML frontmatter"))?;
    let name = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("name:"))
        .map(|v| v.trim().trim_matches('"').trim_matches('\''));
    if name != Some("wiki") {
        return Err(miette!(
            "wiki install: SKILL.md frontmatter must contain `name: wiki`"
        ));
    }
    Ok(())
}

/// Extract the YAML frontmatter block from a markdown document.
fn parse_frontmatter(body: &str) -> Option<&str> {
    let rest = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    Some(&rest[..end])
}

/// Install (or update) `$CODEX_HOME/skills/wiki` from `staged_skill_root`.
///
/// See module docs for the conflict policy. Returns the [`ActionStatus`] and
/// any backup path created.
fn install_skill(
    codex_home: &Path,
    staged_skill_root: &Path,
    git_ref: &str,
    installed_at: &str,
    force: bool,
    manifest_lists_skill: bool,
) -> Result<(ActionStatus, Option<PathBuf>)> {
    validate_staged_skill(staged_skill_root)?;

    let skills_dir = codex_home.join("skills");
    std::fs::create_dir_all(&skills_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to create {}", skills_dir.display()))?;
    let target = skills_dir.join("wiki");

    // Stage a sibling temp dir so the final move is an atomic rename.
    let staging = skills_dir.join(format!(
        "wiki.tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).into_diagnostic()?;
    }
    copy_dir_recursive(staged_skill_root, &staging)?;

    // Drop the marker file at the root of the staged skill dir.
    let marker_body = format!("source_ref: {git_ref}\ninstalled_at: {installed_at}\n");
    std::fs::write(staging.join(MANAGED_MARKER), marker_body)
        .into_diagnostic()
        .wrap_err("wiki install: failed to write managed marker")?;

    let mut status = ActionStatus::New;
    let mut backup: Option<PathBuf> = None;

    if target.exists() {
        let managed = target.join(MANAGED_MARKER).exists() || manifest_lists_skill;
        if !managed && !force {
            // Clean up staging before erroring out.
            let _ = std::fs::remove_dir_all(&staging);
            return Err(miette!(
                "wiki install: {} exists but is not managed by wiki install; pass --force to replace it",
                target.display()
            ));
        }
        let backup_dir = ensure_backup_dir(codex_home)?;
        let backup_path =
            backup_dir.join(format!("skills-wiki-{}", fs_timestamp(SystemTime::now())));
        copy_dir_recursive(&target, &backup_path).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&staging);
        })?;
        std::fs::remove_dir_all(&target)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to remove {}", target.display()))?;
        status = ActionStatus::Updated;
        backup = Some(backup_path);
    }

    std::fs::rename(&staging, &target)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "wiki install: failed to move skill into {}",
                target.display()
            )
        })?;

    Ok((status, backup))
}

// ── Phase 5: hooks.json upsert ──────────────────────────────────────────────

/// Build the managed hook group JSON value.
fn managed_hook_group() -> JsonValue {
    json!({
        "matcher": "Read|Bash",
        "hooks": [{
            "type": "command",
            "command": HOOK_COMMAND,
            "timeout": 10,
            "statusMessage": "Injecting wiki context"
        }],
        "_wikiInstallId": HOOK_INSTALL_ID
    })
}

/// Upsert the managed `PostToolUse` group into `$CODEX_HOME/hooks.json`.
fn upsert_hooks_json(codex_home: &Path) -> Result<(ActionStatus, Option<PathBuf>)> {
    let path = codex_home.join("hooks.json");
    let desired = managed_hook_group();

    let (mut doc, existed): (JsonValue, bool) = if path.exists() {
        let body = std::fs::read_to_string(&path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read {}", path.display()))?;
        let parsed: JsonValue = serde_json::from_str(&body).map_err(|e| {
            miette!(
                "wiki install: {} is not valid JSON: {e}. Fix or remove the file and retry.",
                path.display()
            )
        })?;
        (parsed, true)
    } else {
        (json!({}), false)
    };

    if !doc.is_object() {
        return Err(miette!(
            "wiki install: {} top-level value must be a JSON object",
            path.display()
        ));
    }
    let root_obj = doc.as_object_mut().unwrap();
    let hooks_entry = root_obj
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    if !hooks_entry.is_object() {
        return Err(miette!(
            "wiki install: {} `.hooks` must be an object",
            path.display()
        ));
    }
    let hooks_obj = hooks_entry.as_object_mut().unwrap();
    let post = hooks_obj
        .entry("PostToolUse".to_string())
        .or_insert_with(|| JsonValue::Array(Vec::new()));
    if !post.is_array() {
        return Err(miette!(
            "wiki install: {} `.hooks.PostToolUse` must be an array",
            path.display()
        ));
    }
    let post_arr = post.as_array_mut().unwrap();

    // Find a managed group: either tagged with _wikiInstallId, or a legacy
    // match (matcher Read|Bash and a hook command of --claude/--codex).
    let mut managed_idx: Option<usize> = None;
    for (i, group) in post_arr.iter().enumerate() {
        let tagged = group
            .get("_wikiInstallId")
            .and_then(|v| v.as_str())
            .map(|s| s == HOOK_INSTALL_ID)
            .unwrap_or(false);
        if tagged {
            managed_idx = Some(i);
            break;
        }
        let legacy_match = group.get("matcher").and_then(|v| v.as_str()) == Some("Read|Bash")
            && group
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|arr| {
                    arr.iter().any(|h| {
                        let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
                        cmd == HOOK_COMMAND || cmd == HOOK_COMMAND_LEGACY
                    })
                })
                .unwrap_or(false)
            && group.get("_wikiInstallId").is_none();
        if legacy_match {
            managed_idx = Some(i);
            break;
        }
    }

    let mut changed = true;
    match managed_idx {
        Some(i) => {
            if post_arr[i] == desired {
                changed = false;
            } else {
                post_arr[i] = desired;
            }
        }
        None => {
            post_arr.push(desired);
        }
    }

    if !existed {
        // Always a "new" install even if body happens to match.
        changed = true;
    }

    let status = if !existed {
        ActionStatus::New
    } else if changed {
        ActionStatus::Updated
    } else {
        ActionStatus::Unchanged
    };

    let backup = if existed && changed {
        let backup_dir = ensure_backup_dir(codex_home)?;
        let backup_path =
            backup_dir.join(format!("hooks-{}.json", fs_timestamp(SystemTime::now())));
        std::fs::copy(&path, &backup_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to back up {}", path.display()))?;
        Some(backup_path)
    } else {
        None
    };

    if changed || !existed {
        let serialized = serde_json::to_string_pretty(&doc)
            .into_diagnostic()
            .wrap_err("wiki install: failed to serialize hooks.json")?;
        std::fs::write(&path, format!("{serialized}\n"))
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to write {}", path.display()))?;
    }

    Ok((status, backup))
}

// ── Phase 6: config.toml feature flag ───────────────────────────────────────

/// Ensure `[features].codex_hooks = true` in `$CODEX_HOME/config.toml`.
fn ensure_codex_hooks_flag(codex_home: &Path) -> Result<(ActionStatus, Option<PathBuf>)> {
    let path = codex_home.join("config.toml");

    if !path.exists() {
        std::fs::create_dir_all(codex_home)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to create {}", codex_home.display()))?;
        std::fs::write(&path, "[features]\ncodex_hooks = true\n")
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to write {}", path.display()))?;
        return Ok((ActionStatus::New, None));
    }

    let body = std::fs::read_to_string(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to read {}", path.display()))?;
    let mut doc: DocumentMut = body.parse().map_err(|e| {
        miette!(
            "wiki install: {} is not valid TOML: {e}. Fix or remove the file and retry.",
            path.display()
        )
    })?;

    let features_item = doc
        .as_table_mut()
        .entry("features")
        .or_insert_with(|| Item::Table(Table::new()));
    let features_table = features_item.as_table_mut().ok_or_else(|| {
        miette!(
            "wiki install: {} `features` must be a table",
            path.display()
        )
    })?;

    let previous = features_table
        .get("codex_hooks")
        .and_then(|it| it.as_value())
        .and_then(|v| v.as_bool());

    let changed = previous != Some(true);
    if changed {
        features_table.insert("codex_hooks", toml_value(true));
    }

    let status = if changed {
        ActionStatus::Updated
    } else {
        ActionStatus::Unchanged
    };

    let backup = if changed {
        let backup_dir = ensure_backup_dir(codex_home)?;
        let backup_path =
            backup_dir.join(format!("config-{}.toml", fs_timestamp(SystemTime::now())));
        std::fs::copy(&path, &backup_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to back up {}", path.display()))?;
        Some(backup_path)
    } else {
        None
    };

    if previous == Some(false) {
        println!(
            "wiki install: enabled [features].codex_hooks in {}",
            path.display()
        );
    }

    if changed {
        std::fs::write(&path, doc.to_string())
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to write {}", path.display()))?;
    }

    Ok((status, backup))
}

// ── Manifest ────────────────────────────────────────────────────────────────

/// Load the on-disk manifest, if any, returning an empty object when absent
/// or unreadable as JSON (fail-open here: the manifest is advisory for
/// detecting legacy managed installs).
fn read_manifest(codex_home: &Path) -> JsonValue {
    let path = codex_home.join(".wiki-install").join("manifest.json");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return json!({});
    };
    serde_json::from_str(&body).unwrap_or_else(|_| json!({}))
}

/// Return `true` if the existing manifest lists `skills/wiki/**` entries.
fn manifest_lists_skill(manifest: &JsonValue) -> bool {
    manifest
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .any(|s| s.starts_with("skills/wiki/"))
        })
        .unwrap_or(false)
}

/// Write the post-install manifest.
fn write_manifest(
    codex_home: &Path,
    git_ref: &str,
    installed_at: &str,
    skill_files: &[String],
) -> Result<()> {
    let dir = codex_home.join(".wiki-install");
    std::fs::create_dir_all(&dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to create {}", dir.display()))?;
    let mut files: Vec<String> = skill_files.to_vec();
    files.push("hooks.json".to_string());
    files.push("config.toml".to_string());
    files.sort();

    let manifest = json!({
        "installer": "wiki install --codex",
        "source": SOURCE_URL,
        "ref": git_ref,
        "installedAt": installed_at,
        "files": files,
        "hookId": HOOK_INSTALL_ID,
    });
    let serialized = serde_json::to_string_pretty(&manifest)
        .into_diagnostic()
        .wrap_err("wiki install: failed to serialize manifest.json")?;
    std::fs::write(dir.join("manifest.json"), format!("{serialized}\n"))
        .into_diagnostic()
        .wrap_err("wiki install: failed to write manifest.json")?;
    Ok(())
}

// ── Pre-flight validation ───────────────────────────────────────────────────

/// Parse existing `hooks.json` and `config.toml` under `codex_home` without
/// mutating anything, so that a syntactically broken file causes the install
/// to fail before any skill files are written.
fn preflight_validate_existing(codex_home: &Path) -> Result<()> {
    let hooks_path = codex_home.join("hooks.json");
    if hooks_path.exists() {
        let body = std::fs::read_to_string(&hooks_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read {}", hooks_path.display()))?;
        let parsed: JsonValue = serde_json::from_str(&body).map_err(|e| {
            miette!(
                "wiki install: {} is not valid JSON: {e}. Fix or remove the file and retry.",
                hooks_path.display()
            )
        })?;
        if !parsed.is_object() {
            return Err(miette!(
                "wiki install: {} top-level value must be a JSON object",
                hooks_path.display()
            ));
        }
    }

    let config_path = codex_home.join("config.toml");
    if config_path.exists() {
        let body = std::fs::read_to_string(&config_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("wiki install: failed to read {}", config_path.display()))?;
        let _doc: DocumentMut = body.parse().map_err(|e| {
            miette!(
                "wiki install: {} is not valid TOML: {e}. Fix or remove the file and retry.",
                config_path.display()
            )
        })?;
    }
    Ok(())
}

// ── Orchestration ───────────────────────────────────────────────────────────

/// Apply the three install phases against `codex_home` from a pre-extracted
/// plugin tree rooted at `extracted_root` (the directory layout produced by
/// [`extract_plugin_files`]).
///
/// This is the real unit-testable seam: tests build an extracted tree by
/// hand and call this function directly, bypassing the fetch layer.
pub fn apply_install(
    codex_home: &Path,
    extracted_root: &Path,
    git_ref: &str,
    force: bool,
) -> Result<InstallSummary> {
    std::fs::create_dir_all(codex_home)
        .into_diagnostic()
        .wrap_err_with(|| format!("wiki install: failed to create {}", codex_home.display()))?;

    let staged_skill = extracted_root.join("skills/wiki");
    if !staged_skill.is_dir() {
        return Err(miette!(
            "wiki install: extracted tree is missing skills/wiki at {}",
            staged_skill.display()
        ));
    }

    // Pre-flight: fail closed on invalid existing config files BEFORE we
    // mutate anything on disk. Without this, a broken hooks.json would cause
    // install_skill to run first and leave a partial install behind.
    preflight_validate_existing(codex_home)?;

    let installed_at = rfc3339_utc(SystemTime::now());
    let manifest = read_manifest(codex_home);
    let manifest_has_skill = manifest_lists_skill(&manifest);

    let (skill_status, skill_backup) = install_skill(
        codex_home,
        &staged_skill,
        git_ref,
        &installed_at,
        force,
        manifest_has_skill,
    )?;

    let (hooks_status, hooks_backup) = upsert_hooks_json(codex_home)?;
    let (config_status, config_backup) = ensure_codex_hooks_flag(codex_home)?;

    // Enumerate the installed skill files for the manifest.
    let skill_root = codex_home.join("skills").join("wiki");
    let skill_files: Vec<String> = list_files_recursive(&skill_root)?
        .into_iter()
        .map(|p| format!("skills/wiki/{}", p.to_string_lossy().replace('\\', "/")))
        .collect();

    write_manifest(codex_home, git_ref, &installed_at, &skill_files)?;

    let mut backups: Vec<PathBuf> = Vec::new();
    backups.extend(skill_backup);
    backups.extend(hooks_backup);
    backups.extend(config_backup);

    Ok(InstallSummary {
        skill: skill_status,
        hooks: hooks_status,
        config: config_status,
        skill_files,
        backups,
    })
}

/// Print a human-readable summary of an install to stdout.
fn print_summary(codex_home: &Path, summary: &InstallSummary) {
    println!("wiki install --codex: done");
    println!(
        "  skills/wiki: {} ({} file(s))",
        summary.skill.label(),
        summary.skill_files.len()
    );
    println!("  hooks.json:  {}", summary.hooks.label());
    println!("  config.toml: {}", summary.config.label());
    if !summary.backups.is_empty() {
        println!("  backups:");
        for b in &summary.backups {
            println!("    - {}", b.display());
        }
    }
    let manifest_path = codex_home.join(".wiki-install/manifest.json");
    println!("  manifest:    {}", manifest_path.display());
}

/// Run the `wiki install` command.
///
/// Fails closed unless `--codex` is provided — only the Codex integration is
/// supported initially.
pub fn run(
    codex: bool,
    force: bool,
    dry_run: bool,
    codex_home: Option<&Path>,
    git_ref: &str,
) -> Result<i32> {
    run_with_fetcher(codex, force, dry_run, codex_home, git_ref, &GitHubFetcher)
}

/// Run the `wiki install` command against a caller-supplied [`SourceFetcher`].
///
/// This is the seam used by integration tests to exercise the entire install
/// flow — including codex-home resolution, dry-run handling, archive
/// extraction, and [`apply_install`] — without hitting `github.com`.
pub fn run_with_fetcher(
    codex: bool,
    force: bool,
    dry_run: bool,
    codex_home: Option<&Path>,
    git_ref: &str,
    fetcher: &dyn SourceFetcher,
) -> Result<i32> {
    if !codex {
        return Err(miette!(
            "wiki install: --codex is required (only the Codex integration is supported)"
        ));
    }

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
        println!("  codex home: {}", home.display());
        println!("  ref:        {git_ref}");
        println!("  planned files:");
        for rel in planned {
            println!("    - {}/{}", home.display(), rel);
        }
        println!("dry run: no network, no files written.");
        return Ok(0);
    }

    println!("wiki install --codex");
    println!("  codex home: {}", home.display());
    println!("  ref:        {git_ref}");

    let archive = fetcher.fetch_archive(git_ref)?;
    let temp = tempfile::Builder::new()
        .prefix("wiki-install-")
        .tempdir()
        .into_diagnostic()
        .wrap_err("wiki install: failed to create temp directory")?;
    let _extracted = extract_plugin_files(&archive, temp.path())?;

    let summary = apply_install(&home, temp.path(), git_ref, force)?;
    print_summary(&home, &summary);
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
            Some(Path::new("/tmp/codex-test-home-dry-run")),
            "main",
        )
        .expect("run");
        assert_eq!(code, 0);
        // Dry run must not have created the directory.
        assert!(!Path::new("/tmp/codex-test-home-dry-run").exists());
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
        assert_eq!(got_sorted, want_sorted);

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
        assert!(err.to_string().contains("exactly one top-level directory"));
    }

    #[test]
    fn extract_plugin_files_rejects_parent_dir_zip_slip() {
        let archive = build_zip(&[
            ("wiki-main/plugins/wiki/hooks/hooks.json", b"{}"),
            ("wiki-main/../evil.txt", b"pwn"),
        ]);
        let temp = tempfile::tempdir().expect("tempdir");
        let err = extract_plugin_files(&archive, temp.path()).unwrap_err();
        assert!(err.to_string().contains("parent-directory traversal"));
    }

    #[test]
    fn extract_plugin_files_rejects_absolute_path_zip_slip() {
        let archive = build_zip(&[("/etc/passwd", b"pwn")]);
        let temp = tempfile::tempdir().expect("tempdir");
        let err = extract_plugin_files(&archive, temp.path()).unwrap_err();
        assert!(err.to_string().contains("absolute path in archive"));
    }

    // ── Phase 4–6 tests ───────────────────────────────────────────────────

    /// Build an `extracted_root/skills/wiki/...` tree on disk for tests.
    fn stage_extracted(root: &Path) {
        let skill = root.join("skills/wiki");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: wiki\ndescription: test\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(skill.join("references/maintenance.md"), "ref body").unwrap();
    }

    #[test]
    fn rfc3339_matches_known_epoch() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(rfc3339_utc(t), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn fresh_install_writes_skill_hooks_config_and_manifest() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        let summary =
            apply_install(codex.path(), extracted.path(), "main", false).expect("install");
        assert_eq!(summary.skill, ActionStatus::New);
        assert_eq!(summary.hooks, ActionStatus::New);
        assert_eq!(summary.config, ActionStatus::New);
        assert!(summary.backups.is_empty());

        // Skill files + marker.
        let skill_root = codex.path().join("skills/wiki");
        assert!(skill_root.join("SKILL.md").is_file());
        assert!(skill_root.join("references/maintenance.md").is_file());
        let marker = std::fs::read_to_string(skill_root.join(MANAGED_MARKER)).unwrap();
        assert!(marker.contains("source_ref: main"));
        assert!(marker.contains("installed_at:"));

        // hooks.json contains the managed group.
        let hooks: JsonValue = serde_json::from_str(
            &std::fs::read_to_string(codex.path().join("hooks.json")).unwrap(),
        )
        .unwrap();
        let post = &hooks["hooks"]["PostToolUse"];
        assert!(post.is_array());
        let arr = post.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["_wikiInstallId"].as_str(), Some(HOOK_INSTALL_ID));
        assert_eq!(arr[0]["hooks"][0]["command"].as_str(), Some(HOOK_COMMAND));

        // config.toml has features.codex_hooks = true.
        let cfg = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
        assert!(cfg.contains("codex_hooks = true"));

        // Manifest written with skill files.
        let manifest_body =
            std::fs::read_to_string(codex.path().join(".wiki-install/manifest.json")).unwrap();
        let manifest: JsonValue = serde_json::from_str(&manifest_body).unwrap();
        assert_eq!(manifest["hookId"].as_str(), Some(HOOK_INSTALL_ID));
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
    fn rerun_updates_managed_skill_and_is_idempotent_for_hooks() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        apply_install(codex.path(), extracted.path(), "main", false).unwrap();

        // Change source body so the second install is a real update.
        std::fs::write(
            extracted.path().join("skills/wiki/SKILL.md"),
            "---\nname: wiki\n---\nupdated body\n",
        )
        .unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", false).expect("rerun");
        assert_eq!(summary.skill, ActionStatus::Updated);
        assert_eq!(summary.hooks, ActionStatus::Unchanged);
        assert_eq!(summary.config, ActionStatus::Unchanged);

        let body = std::fs::read_to_string(codex.path().join("skills/wiki/SKILL.md")).unwrap();
        assert!(body.contains("updated body"));

        // Backup of previous skill dir exists.
        let backups = codex.path().join(".wiki-install/backups");
        let has_skill_backup = std::fs::read_dir(&backups).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("skills-wiki-")
        });
        assert!(has_skill_backup);
    }

    #[test]
    fn unmanaged_skill_without_force_fails() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        let target = codex.path().join("skills/wiki");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("README.md"), "user-authored").unwrap();

        let err = apply_install(codex.path(), extracted.path(), "main", false).unwrap_err();
        assert!(err.to_string().contains("not managed"), "got: {err}");

        // Original user file intact; no managed marker, no hooks.json.
        assert!(target.join("README.md").is_file());
        assert!(!target.join(MANAGED_MARKER).exists());
        assert!(!codex.path().join("hooks.json").exists());
    }

    #[test]
    fn force_backs_up_and_replaces_unmanaged_skill() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        let target = codex.path().join("skills/wiki");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("README.md"), "user-authored").unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", true).expect("force");
        assert_eq!(summary.skill, ActionStatus::Updated);
        assert!(summary.backups.iter().any(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("skills-wiki-"))
                .unwrap_or(false)
        }));

        // New skill installed; old unmanaged file gone from target.
        assert!(target.join("SKILL.md").is_file());
        assert!(!target.join("README.md").is_file());

        // Backup preserves the user's file.
        let backup_dir = summary
            .backups
            .iter()
            .find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("skills-wiki-"))
                    .unwrap_or(false)
            })
            .unwrap();
        assert!(backup_dir.join("README.md").is_file());
    }

    #[test]
    fn hooks_upsert_replaces_managed_group_and_preserves_unrelated() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        // Pre-populate hooks.json with an unrelated event plus an existing
        // managed group (tagged) and an unrelated PostToolUse group.
        let existing = json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "echo pre"}]}],
                "PostToolUse": [
                    {"matcher": "Edit", "hooks": [{"type": "command", "command": "echo other"}]},
                    {
                        "matcher": "Read|Bash",
                        "hooks": [{"type": "command", "command": "wiki hook --codex"}],
                        "_wikiInstallId": HOOK_INSTALL_ID
                    }
                ]
            },
            "otherTopLevel": {"keep": true}
        });
        std::fs::write(
            codex.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", false).unwrap();
        assert_eq!(summary.hooks, ActionStatus::Updated);

        let body = std::fs::read_to_string(codex.path().join("hooks.json")).unwrap();
        let parsed: JsonValue = serde_json::from_str(&body).unwrap();

        // Unrelated event preserved.
        assert_eq!(
            parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str(),
            Some("echo pre")
        );
        // Unrelated top-level key preserved.
        assert_eq!(parsed["otherTopLevel"]["keep"].as_bool(), Some(true));
        // Still exactly one managed group; unrelated PostToolUse group preserved.
        let post = parsed["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        let managed_count = post
            .iter()
            .filter(|g| g.get("_wikiInstallId").and_then(|v| v.as_str()) == Some(HOOK_INSTALL_ID))
            .count();
        assert_eq!(managed_count, 1);
        // The managed group carries the full payload (timeout/statusMessage).
        let managed = post
            .iter()
            .find(|g| g.get("_wikiInstallId").and_then(|v| v.as_str()) == Some(HOOK_INSTALL_ID))
            .unwrap();
        assert_eq!(managed["hooks"][0]["timeout"].as_i64(), Some(10));
        assert_eq!(
            managed["hooks"][0]["statusMessage"].as_str(),
            Some("Injecting wiki context")
        );
    }

    #[test]
    fn hooks_upsert_converts_legacy_claude_command() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        let existing = json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "Read|Bash",
                    "hooks": [{"type": "command", "command": "wiki hook --claude"}]
                }]
            }
        });
        std::fs::write(
            codex.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        apply_install(codex.path(), extracted.path(), "main", false).unwrap();

        let body = std::fs::read_to_string(codex.path().join("hooks.json")).unwrap();
        let parsed: JsonValue = serde_json::from_str(&body).unwrap();
        let post = parsed["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(
            post[0]["hooks"][0]["command"].as_str(),
            Some("wiki hook --codex")
        );
        assert_eq!(post[0]["_wikiInstallId"].as_str(), Some(HOOK_INSTALL_ID));
    }

    #[test]
    fn hooks_upsert_fails_closed_on_invalid_json() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());
        std::fs::write(codex.path().join("hooks.json"), "not json {").unwrap();

        let err = apply_install(codex.path(), extracted.path(), "main", false).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "got: {err}");
    }

    #[test]
    fn config_toml_created_when_missing() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        apply_install(codex.path(), extracted.path(), "main", false).unwrap();
        let body = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
        assert!(body.contains("[features]"));
        assert!(body.contains("codex_hooks = true"));
    }

    #[test]
    fn config_toml_preserves_unrelated_keys() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        let original = "# user notes\nmodel = \"gpt-5\"\n\n[features]\nother_flag = true\n";
        std::fs::write(codex.path().join("config.toml"), original).unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", false).unwrap();
        assert_eq!(summary.config, ActionStatus::Updated);

        let body = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
        assert!(body.contains("# user notes"), "comment lost: {body}");
        assert!(body.contains("model = \"gpt-5\""));
        assert!(body.contains("other_flag = true"));
        assert!(body.contains("codex_hooks = true"));
    }

    #[test]
    fn config_toml_flips_false_to_true() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        std::fs::write(
            codex.path().join("config.toml"),
            "[features]\ncodex_hooks = false\n",
        )
        .unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", false).unwrap();
        assert_eq!(summary.config, ActionStatus::Updated);

        let body = std::fs::read_to_string(codex.path().join("config.toml")).unwrap();
        assert!(body.contains("codex_hooks = true"));
        assert!(!body.contains("codex_hooks = false"));
    }

    #[test]
    fn config_toml_unchanged_when_already_true() {
        let codex = tempfile::tempdir().unwrap();
        let extracted = tempfile::tempdir().unwrap();
        stage_extracted(extracted.path());

        std::fs::write(
            codex.path().join("config.toml"),
            "[features]\ncodex_hooks = true\n",
        )
        .unwrap();

        let summary = apply_install(codex.path(), extracted.path(), "main", false).unwrap();
        assert_eq!(summary.config, ActionStatus::Unchanged);
    }

    #[test]
    fn validate_staged_skill_rejects_wrong_name() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = tmp.path().join("skills/wiki");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: other\n---\n").unwrap();
        let err = validate_staged_skill(&skill).unwrap_err();
        assert!(err.to_string().contains("name: wiki"));
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
    fn fixture_fetcher_trait_impl_compiles() {
        let f = FixtureFetcher {
            bytes: build_zip(&[("wiki-main/plugins/wiki/hooks/hooks.json", b"{}")]),
        };
        let bytes = f.fetch_archive("main").unwrap();
        assert!(!bytes.is_empty());
    }
}
