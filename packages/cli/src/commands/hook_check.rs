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
pub fn run(input: &str, wiki_root: &Path, repo_root: &Path, source: DocSource) -> Result<i32> {
    let Some(file_path) = parse_file_path(input) else {
        return Ok(0);
    };

    if !file_path.ends_with(".md") {
        return Ok(0);
    }

    let rel_path = super::normalize_repo_relative_path(&file_path, repo_root);
    if !super::matches_default_discovery_path(&rel_path, wiki_root, repo_root) {
        return Ok(0);
    }

    let globs = vec![file_path.clone()];
    let diagnostics = match check::collect_with_source(&globs, wiki_root, repo_root, source) {
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
