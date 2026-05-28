//! Advisory `fs4::try_lock_exclusive` on `.git/wiki-refresh.lock`.
//! Contention means the current process serves the existing snapshot —
//! it does not wait. Phase 1 skeleton.
