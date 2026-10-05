//! Crate-wide concurrency primitives: the one poison-tolerant mutex lock and
//! the lock-free result collector for `ignore`'s parallel directory walker.
//!
//! Panic policy: a panicking worker is never folded into a partial result.
//! Scoped-thread joins re-raise the worker's panic on the joining thread
//! (`std::panic::resume_unwind`), [`collect_parallel_walk`] does the same for
//! walker visitors, and the binary's panic hook turns any panic, on any
//! thread, into the CLI's internal-error exit before unwinding begins.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard};

use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, WalkParallel, WalkState};

/// Lock `mutex`, recovering the data if a thread panicked while holding it.
///
/// Every mutex in this crate guards state whose invariants hold between
/// individual statements: memoized caches of read-only repository facts,
/// perf counters, an append-only log file. A guard dropped mid-panic cannot
/// leave such state half-updated in a way a later reader could misread, so
/// recovering is sound. Recovery never hides the panic itself (see the
/// module docs); it only stops one panic from cascading into a second,
/// misleading `PoisonError` panic on every later lock.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Run `walker` to completion and return every item its visitors produced.
///
/// `visit` is called once per walked entry with the calling thread's own
/// output buffer, so collection takes no shared lock: each per-thread
/// visitor owns its buffer and hands it back over a channel when the walker
/// drops it. Item order is unspecified; callers sort.
///
/// A panic inside `visit` stops the whole walk (`WalkState::Quit`) and is
/// re-raised on the calling thread once the walker has wound down, so the
/// caller can never observe a truncated result. (Left alone, `ignore` would
/// deadlock instead: the panicked worker never deactivates, so its peers
/// wait for work forever.)
pub(crate) fn collect_parallel_walk<T, F>(walker: WalkParallel, visit: F) -> Vec<T>
where
    T: Send,
    F: Fn(Result<DirEntry, ignore::Error>, &mut Vec<T>) -> WalkState + Sync,
{
    let (tx, rx) = mpsc::channel();
    let mut builder = CollectBuilder { visit: &visit, tx };
    walker.visit(&mut builder);
    // `visit` returns only after every visitor it built has been dropped
    // (each sent its batch); dropping the builder's own sender closes the
    // channel, so the drain below terminates after the last batch.
    drop(builder);
    let mut collected = Vec::new();
    for batch in rx {
        match batch {
            Ok(items) => collected.extend(items),
            Err(payload) => resume_unwind(payload),
        }
    }
    collected
}

/// A visitor's hand-back: its items, or the payload of the panic that
/// stopped it.
type Batch<T> = Result<Vec<T>, Box<dyn Any + Send>>;

struct CollectBuilder<'s, T, F> {
    visit: &'s F,
    tx: Sender<Batch<T>>,
}

impl<'s, T, F> ParallelVisitorBuilder<'s> for CollectBuilder<'s, T, F>
where
    T: Send + 's,
    F: Fn(Result<DirEntry, ignore::Error>, &mut Vec<T>) -> WalkState + Sync,
{
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        Box::new(CollectVisitor {
            visit: self.visit,
            items: Vec::new(),
            panic: None,
            tx: self.tx.clone(),
        })
    }
}

struct CollectVisitor<'s, T, F> {
    visit: &'s F,
    items: Vec<T>,
    panic: Option<Box<dyn Any + Send>>,
    tx: Sender<Batch<T>>,
}

impl<T, F> ParallelVisitor for CollectVisitor<'_, T, F>
where
    T: Send,
    F: Fn(Result<DirEntry, ignore::Error>, &mut Vec<T>) -> WalkState + Sync,
{
    fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
        if self.panic.is_some() {
            return WalkState::Quit;
        }
        let visit = self.visit;
        let items = &mut self.items;
        // Unwind safety: after a panic this visitor's buffer is discarded
        // (only the payload is handed back), so no state the panic may have
        // left half-written is ever observed.
        match catch_unwind(AssertUnwindSafe(|| visit(entry, items))) {
            Ok(state) => state,
            Err(payload) => {
                self.panic = Some(payload);
                WalkState::Quit
            }
        }
    }
}

impl<T, F> Drop for CollectVisitor<'_, T, F> {
    fn drop(&mut self) {
        let batch = match self.panic.take() {
            Some(payload) => Err(payload),
            None => Ok(std::mem::take(&mut self.items)),
        };
        // The receiver lives in `collect_parallel_walk`'s frame across the
        // whole blocking `WalkParallel::visit` call that owns this visitor,
        // so it is still connected here and the send cannot fail.
        let _ = self.tx.send(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn tree_with_files(count: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..count {
            let sub = dir.path().join(format!("d{}", i % 7));
            std::fs::create_dir_all(&sub).expect("mkdir");
            std::fs::write(sub.join(format!("f{i}.md")), "x").expect("write");
        }
        dir
    }

    fn files_only(
        entry: Result<DirEntry, ignore::Error>,
        out: &mut Vec<PathBuf>,
    ) -> WalkState {
        if let Ok(entry) = entry
            && entry.file_type().is_some_and(|t| t.is_file())
        {
            out.push(entry.into_path());
        }
        WalkState::Continue
    }

    #[test]
    fn collects_every_entry_across_threads() {
        let dir = tree_with_files(200);
        let walker = ignore::WalkBuilder::new(dir.path()).threads(4).build_parallel();
        let got: BTreeSet<PathBuf> = collect_parallel_walk(walker, files_only).into_iter().collect();
        let want: BTreeSet<PathBuf> = ignore::WalkBuilder::new(dir.path())
            .build()
            .map(|e| e.expect("serial walk entry"))
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(ignore::DirEntry::into_path)
            .collect();
        assert_eq!(got.len(), 200);
        assert_eq!(got, want);
    }

    #[test]
    fn visitor_panic_propagates_instead_of_truncating_or_hanging() {
        let dir = tree_with_files(200);
        let walker = ignore::WalkBuilder::new(dir.path()).threads(4).build_parallel();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            collect_parallel_walk(walker, |entry, out: &mut Vec<PathBuf>| {
                if let Ok(entry) = &entry
                    && entry.file_name() == "f17.md"
                {
                    panic!("injected visitor panic");
                }
                files_only(entry, out)
            })
        }));
        let payload = outcome.expect_err("a visitor panic must propagate");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"injected visitor panic"));
    }

    #[test]
    fn lock_recovers_a_poisoned_mutex() {
        let mutex = Mutex::new(vec![1]);
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = mutex.lock().expect("first lock");
            guard.push(2);
            panic!("poison it");
        }));
        assert!(poisoned.is_err());
        assert!(mutex.is_poisoned());
        assert_eq!(*lock(&mutex), vec![1, 2]);
    }
}
