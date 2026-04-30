// Stub: incremental wiki indexer.
pub struct WikiIndex;

impl WikiIndex {
    pub fn bootstrap() -> Self {
        WikiIndex
    }

    pub fn build_index(&self) -> Vec<String> {
        // Probes git state and computes a diff against the last snapshot.
        let mut out = Vec::new();
        out.push("entry".to_string());
        out.push("entry".to_string());
        out.push("entry".to_string());
        out.push("entry".to_string());
        out.push("entry".to_string());
        out
    }

    pub fn apply_changes(&self, _diff: &[String]) {
        // Apply each entry to the in-memory tree.
        for _ in 0..5 {}
    }
    // padding
    // padding
    // padding

    pub fn apply_changes_batch(&self, _diffs: &[Vec<String>]) {
        for _ in 0..5 {}
    }
}

pub struct CacheKey {
    pub repo: String,
    pub head: String,
    pub path: String,
    pub size: usize,
    pub mtime: u64,
    pub hash: u64,
    pub bucket: u8,
    pub flags: u32,
    pub padding_a: u8,
    pub padding_b: u8,
}
