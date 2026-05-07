use std::path::Path;

use miette::Result;

use super::check;
use crate::index::DocSource;

/// Parse `tool_input.file_path` from a PostToolUse JSON event.
fn parse_file_path(input: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(input.trim()).ok()?;
    json.get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// PostToolUse hook: run `wiki check --fix` on the written or edited file.
///
/// Only processes `.md` files. Outputs a JSON `systemMessage` envelope when
/// validation errors remain after auto-fix so Claude can address them.
pub fn run(input: &str, _wiki_root: &Path, repo_root: &Path, source: DocSource) -> Result<i32> {
    let Some(file_path) = parse_file_path(input) else {
        return Ok(0);
    };

    if !file_path.ends_with(".md") {
        return Ok(0);
    }

    // Walk up from the edited file's directory to find a wiki.toml. If none
    // exists, the file is outside any wiki and the hook silently no-ops.
    let abs_file = if Path::new(&file_path).is_absolute() {
        std::path::PathBuf::from(&file_path)
    } else {
        repo_root.join(&file_path)
    };
    let walk_start = abs_file.parent().unwrap_or(repo_root);
    let cfg = match crate::wiki_config::WikiConfig::load(walk_start, repo_root) {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };
    // Pick the wiki that owns the edited file. For `*.wiki.md` floats this is
    // the namespace declared in frontmatter (or the default wiki when the
    // float is untagged); for regular pages it is the deepest ancestor wiki
    // root. Silently no-op when no wiki can own the file.
    let canon_walk = std::fs::canonicalize(walk_start).unwrap_or_else(|_| walk_start.to_path_buf());
    let target_wiki: Option<&crate::wiki_config::WikiInfo> = {
        // Deepest enclosing wiki root, if any.
        let enclosing = cfg
            .all()
            .filter(|w| canon_walk.starts_with(&w.root))
            .max_by_key(|w| w.root.components().count());
        if file_path.ends_with(".wiki.md") {
            let content = std::fs::read_to_string(&abs_file).unwrap_or_default();
            match crate::frontmatter::parse_namespace(&content) {
                // Tagged float: route to the declared namespace, falling back
                // to the enclosing wiki and then to default.
                Some(ns) => cfg
                    .wikis
                    .get(&ns)
                    .or(enclosing)
                    .or_else(|| cfg.default()),
                // Untagged float: prefer the enclosing wiki so a float nested
                // under a peer root is owned by that peer; otherwise default.
                None => enclosing.or_else(|| cfg.default()),
            }
        } else {
            enclosing
        }
    };
    let Some(target_wiki) = target_wiki else {
        return Ok(0);
    };
    let wiki_root = target_wiki.root.as_path();

    let rel_path = super::normalize_repo_relative_path(&file_path, repo_root);
    if !super::matches_default_discovery_path(&rel_path, wiki_root, repo_root) {
        return Ok(0);
    }

    let globs = vec![file_path.clone()];
    let diagnostics = match check::collect_with_config(&globs, wiki_root, repo_root, None, source) {
        Ok(d) => d,
        Err(_) => return Ok(0), // not a wiki page or discovery failed — skip silently
    };

    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.kind != "alias_resolve")
        .collect();

    if errors.is_empty() {
        return Ok(0);
    }

    let lines: Vec<String> = errors
        .iter()
        .map(|d| format!("  {}:{} [{}] {}", d.file, d.line, d.kind, d.message))
        .collect();

    let system_message = format!(
        "wiki check found validation errors in `{file_path}` that require manual fixes:\n{}",
        lines.join("\n")
    );

    println!("{}", serde_json::json!({ "systemMessage": system_message }));

    Ok(0)
}
