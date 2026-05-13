use std::path::{Path, PathBuf};

/// Test helper: ensure `repo_root/<dir>` exists and return its absolute path.
pub fn write_wiki_toml(repo_root: &Path, dir: &str) -> PathBuf {
    let wiki_dir = repo_root.join(dir);
    std::fs::create_dir_all(&wiki_dir).expect("create wiki dir");
    wiki_dir
}
