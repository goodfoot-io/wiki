use std::path::Path;

use miette::Result;

use super::check;

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
pub fn run(input: &str, _wiki_root: &Path, repo_root: &Path) -> Result<i32> {
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
    let wiki_root = cfg.current.root.as_path();

    let rel_path = super::normalize_repo_relative_path(&file_path, repo_root);
    if !super::matches_default_discovery_path(&rel_path, wiki_root, repo_root) {
        return Ok(0);
    }

    let globs = vec![file_path.clone()];
    let diagnostics = match check::collect(&globs, wiki_root, repo_root) {
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
