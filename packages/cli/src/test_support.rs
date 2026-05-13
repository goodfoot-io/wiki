use std::path::{Path, PathBuf};

/// Test helper: ensure `repo_root/<dir>` exists and return its absolute path.
///
/// This is the wiki-root directory; callers pass the returned path as the
/// `wiki_root` argument to command functions. The legacy name is kept so
/// historical call sites continue to compile.
pub fn write_wiki_toml(repo_root: &Path, dir: &str) -> PathBuf {
    let wiki_root = repo_root.join(dir);
    std::fs::create_dir_all(&wiki_root).expect("create wiki dir");
    wiki_root
}
