use std::path::{Path, PathBuf};

/// Test helper: write an empty `wiki.toml` to `repo_root/<dir>/wiki.toml` so a
/// `WikiConfig::load` walk-up rooted in that directory finds it. Returns the
/// absolute path to the wiki root (i.e. `repo_root/<dir>`).
pub fn write_wiki_toml(repo_root: &Path, dir: &str) -> PathBuf {
    let wiki_root = repo_root.join(dir);
    std::fs::create_dir_all(&wiki_root).expect("create wiki dir");
    std::fs::write(wiki_root.join("wiki.toml"), "").expect("write wiki.toml");
    wiki_root
}
