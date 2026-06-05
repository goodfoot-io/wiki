//! Advisory `fs4::try_lock_exclusive` on `.wiki/wiki-refresh.lock`.
//! Contention means the current process serves the existing snapshot —
//! it does not wait.

use std::fs::{File, OpenOptions};
use std::path::Path;

use fs4::fs_std::FileExt;

/// Holds the OS-level exclusive lock on `.wiki/wiki-refresh.lock`.
///
/// The lock is released by `Drop` (closing the file releases the fcntl lock).
pub struct RefreshLock {
    _file: File,
}

/// Try-acquire an exclusive advisory lock on `<wiki_dir>/wiki-refresh.lock`.
///
/// * `Ok(Some(_))` — lock acquired; caller may refresh.
/// * `Ok(None)` — another process holds the lock; caller should serve the
///   existing snapshot instead of waiting.
pub fn try_acquire(wiki_dir: &Path) -> anyhow::Result<Option<RefreshLock>> {
    let path = wiki_dir.join("wiki-refresh.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    match file.try_lock_exclusive() {
        Ok(true) => Ok(Some(RefreshLock { _file: file })),
        Ok(false) => Ok(None),
        Err(e) => {
            // Some platforms surface `WouldBlock` as Err instead of Ok(false).
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(e.into())
            }
        }
    }
}
