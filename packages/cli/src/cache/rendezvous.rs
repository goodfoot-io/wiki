//! The rendezvous lock (plan D7): coarse-grained shared/exclusive process
//! coordination over one advisory `flock(2)` (via fs4) on
//! `<common>/wiki/rendezvous.lock`. Shared holders compose (search/list/
//! summary/plain-check); exclusive holders exclude everything (index
//! refresh publication, `check --fix`-class destructive work, tier-scoped
//! clear-cache). Ownership is kernel-held, so a crashed process releases
//! instantly.
//!
//! Acquisition waits a bounded ~10 s of 10 ms retries (the refresh
//! contention contract), then errs — the caller serves stale/uncached with
//! its single diagnostic line. Readers never upgrade shared→exclusive in
//! place (fcntl upgrade deadlock hazard); release and re-acquire instead.
//! The lock file is created through [`crate::store::fd`] like every other
//! creation path under the common dir.

use std::fs::File;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;

use crate::cache::schema::{RENDEZVOUS_LOCK_FILE_NAME, STORE_DIR_NAME};
use crate::store::fd::DirFd;

/// Bounded wait before acquisition gives up (plan D7: ~10 s).
const WAIT_BUDGET: Duration = Duration::from_secs(10);
/// Interval between acquisition retries (plan D7: 10 ms).
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// A held rendezvous lock; releases on drop (explicit unlock first —
/// closing a descriptor does not always promptly release the advisory lock
/// for same-process reopeners).
#[derive(Debug)]
pub struct RendezvousGuard {
    _file: File,
}

impl Drop for RendezvousGuard {
    fn drop(&mut self) {
        // Best-effort: drop cannot propagate; the kernel releases the flock
        // when the descriptor closes regardless.
        let _ = FileExt::unlock(&self._file);
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Shared,
    Exclusive,
}

impl Mode {
    fn try_lock(self, file: &File) -> io::Result<bool> {
        // Qualified calls on purpose: newer std toolchains stabilize
        // inherent `File::try_lock_shared`/`unlock`, which would otherwise
        // shadow the fs4 trait methods the plan pins (D7).
        let acquired = match self {
            Mode::Shared => FileExt::try_lock_shared(file),
            Mode::Exclusive => FileExt::try_lock_exclusive(file),
        };
        match acquired {
            Ok(held) => Ok(held),
            // Some platforms surface `WouldBlock` as Err instead of
            // Ok(false) (index/lock.rs precedent).
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Try once for the shared mode without waiting: `Ok(Some(_))` — acquired;
/// `Ok(None)` — someone holds it exclusively right now.
pub fn try_acquire_shared(common_dir: &Path) -> io::Result<Option<RendezvousGuard>> {
    try_acquire(common_dir, Mode::Shared)
}

/// Try once for the exclusive mode without waiting: `Ok(Some(_))` —
/// acquired; `Ok(None)` — anyone else holds it right now.
pub fn try_acquire_exclusive(common_dir: &Path) -> io::Result<Option<RendezvousGuard>> {
    try_acquire(common_dir, Mode::Exclusive)
}

/// Acquire the shared rendezvous lock, waiting up to ~10 s of 10 ms retries.
pub fn acquire_shared(common_dir: &Path) -> io::Result<RendezvousGuard> {
    acquire_for(common_dir, Mode::Shared, WAIT_BUDGET)
}

/// Acquire the exclusive rendezvous lock, waiting up to ~10 s of 10 ms
/// retries.
pub fn acquire_exclusive(common_dir: &Path) -> io::Result<RendezvousGuard> {
    acquire_for(common_dir, Mode::Exclusive, WAIT_BUDGET)
}

fn try_acquire(common_dir: &Path, mode: Mode) -> io::Result<Option<RendezvousGuard>> {
    let common = DirFd::open(common_dir)?;
    let wiki = common.ensure_private_subtree(Path::new(STORE_DIR_NAME))?;
    wiki.validate_private()?;
    let file = wiki.create_file(RENDEZVOUS_LOCK_FILE_NAME)?;
    if mode.try_lock(&file)? {
        Ok(Some(RendezvousGuard { _file: file }))
    } else {
        Ok(None)
    }
}

fn acquire_for(
    common_dir: &Path,
    mode: Mode,
    budget: Duration,
) -> io::Result<RendezvousGuard> {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(guard) = try_acquire(common_dir, mode)? {
            return Ok(guard);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "rendezvous lock stayed contended for the whole wait budget",
            ));
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two processes (here: two descriptors in one process — flock is per
    /// open file description) hold the shared mode simultaneously.
    #[test]
    fn shared_holders_compose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = acquire_shared(dir.path()).expect("first shared holder");
        let second = acquire_shared(dir.path()).expect("second shared holder");
        drop((first, second));
    }

    /// Exclusive excludes both another exclusive and any shared holder;
    /// shared likewise blocks an exclusive. Release lets the other side in.
    #[test]
    fn exclusive_excludes_and_is_excluded_by_everything() {
        let dir = tempfile::tempdir().expect("tempdir");

        let held = try_acquire_exclusive(dir.path())
            .expect("try exclusive")
            .expect("free store grants exclusive");
        assert!(
            try_acquire_exclusive(dir.path()).expect("try again").is_none(),
            "a second exclusive must be refused"
        );
        assert!(
            try_acquire_shared(dir.path()).expect("try shared").is_none(),
            "shared must be refused while exclusive is held"
        );
        drop(held);

        let shared = try_acquire_shared(dir.path())
            .expect("try shared after release")
            .expect("release lets a shared holder in");
        assert!(
            try_acquire_exclusive(dir.path())
                .expect("try exclusive")
                .is_none(),
            "exclusive must be refused while shared is held"
        );
        drop(shared);
    }

    /// A contended lock errors out after the bounded budget — never hangs,
    /// never panics (plan D7's floor behavior).
    #[test]
    fn bounded_wait_errors_after_the_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let held = try_acquire_exclusive(dir.path())
            .expect("try exclusive")
            .expect("hold exclusive");

        let started = Instant::now();
        let result = acquire_for(dir.path(), Mode::Shared, Duration::from_millis(50));
        let err = result.expect_err("a contended lock must time out");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "the waiter must actually wait"
        );
        drop(held);
    }

    /// Dropping the guard releases the lock immediately.
    #[test]
    fn dropping_the_guard_releases_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        drop(acquire_exclusive(dir.path()).expect("acquire"));
        assert!(
            try_acquire_exclusive(dir.path())
                .expect("try again")
                .is_some(),
            "the released lock must be acquirable"
        );
    }
}
