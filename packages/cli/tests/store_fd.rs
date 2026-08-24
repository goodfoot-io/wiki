//! Unit checks for the descriptor-hardened filesystem primitives
//! ([`wiki::store::fd`], plan D9): symlink refusal on descent, owner/mode
//! validation, private-subtree creation, and retained-fd rebinding
//! validation. All fixtures live in temp dirs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::TempDir;

use wiki::store::fd::{DirFd, PRIVATE_DIR_MODE};

/// `chmod` helper for fixture directories.
fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set_permissions");
}

/// The mode bits of `path`.
fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777
}

// ── Symlink refusal ──────────────────────────────────────────────────────────

/// Opening a symlinked directory is refused — never followed.
#[test]
fn open_refuses_a_symlinked_directory() {
    let dir = TempDir::new().expect("tempdir");
    fs::create_dir(dir.path().join("real")).expect("real dir");
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link"))
        .expect("symlink");

    assert!(
        DirFd::open(&dir.path().join("link")).is_err(),
        "a symlink final component must be refused"
    );
    // The real directory opens fine.
    assert!(DirFd::open(&dir.path().join("real")).is_ok());
}

/// A symlink introduced at any level of a subtree descent is refused.
#[test]
fn ensure_private_subtree_refuses_symlink_components() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");
    fs::create_dir(dir.path().join("wiki")).expect("wiki dir");
    fs::create_dir(dir.path().join("elsewhere")).expect("elsewhere dir");
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), dir.path().join("wiki/journal"))
        .expect("symlink inside the tree");

    let result = root.ensure_private_subtree(Path::new("wiki/journal"));
    assert!(result.is_err(), "a symlink descent component must be refused");
}

/// Creating a file over a symlink name is refused.
#[test]
fn create_file_refuses_a_symlink_name() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");
    fs::write(dir.path().join("victim"), b"innocent").expect("write victim");
    std::os::unix::fs::symlink(dir.path().join("victim"), dir.path().join("store.init.lock"))
        .expect("symlink over the lock name");

    assert!(
        root.create_file("store.init.lock").is_err(),
        "a symlink at the file position must be refused"
    );
    assert_eq!(
        fs::read(dir.path().join("victim")).expect("read victim"),
        b"innocent",
        "the symlink target must be untouched"
    );
}

// ── Owner + mode validation ─────────────────────────────────────────────────

/// A directory with a too-permissive mode fails private validation; the
/// exact private mode passes.
#[test]
fn validate_private_rejects_wrong_modes_and_accepts_the_private_mode() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");

    for lax in [0o755, 0o777, 0o711, 0o750] {
        set_mode(dir.path(), lax);
        assert!(
            root.validate_private().is_err(),
            "mode {lax:o} must fail private validation"
        );
    }

    set_mode(dir.path(), PRIVATE_DIR_MODE);
    root.validate_private().expect("the private mode validates");
}

/// A directory created by [`DirFd::ensure_private_subtree`] lands with
/// exactly mode 0700 regardless of umask.
#[test]
fn created_subtrees_get_exactly_the_private_mode() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");

    root.ensure_private_subtree(Path::new("wiki"))
        .expect("ensure wiki");
    assert_eq!(mode_of(&dir.path().join("wiki")), 0o700);

    // Nested descent creates every missing level privately.
    root.ensure_private_subtree(Path::new("journal/abc123"))
        .expect("ensure journal/abc123");
    assert_eq!(mode_of(&dir.path().join("journal")), 0o700);
    assert_eq!(mode_of(&dir.path().join("journal/abc123")), 0o700);
}

/// Subtree creation is idempotent: an existing private directory is adopted,
/// an existing too-permissive one is refused (never silently chmod'ed back).
#[test]
fn ensure_private_subtree_adopts_private_and_refuses_lax_existing_dirs() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");

    root.ensure_private_subtree(Path::new("wiki"))
        .expect("first create");
    root.ensure_private_subtree(Path::new("wiki"))
        .expect("second call adopts the existing private dir");

    set_mode(&dir.path().join("wiki"), 0o777);
    assert!(
        root.ensure_private_subtree(Path::new("wiki")).is_err(),
        "an existing lax directory must be refused, not repaired"
    );
}

/// Files are created owner-only (0600) and existing files are opened without
/// truncation.
#[test]
fn create_file_is_owner_only_and_never_truncates() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");

    drop(root.create_file("lock").expect("create lock"));
    assert_eq!(mode_of(&dir.path().join("lock")), 0o600);

    fs::write(dir.path().join("db"), b"precious bytes").expect("seed db");
    drop(root.create_file("db").expect("reopen db"));
    assert_eq!(
        fs::read(dir.path().join("db")).expect("read db"),
        b"precious bytes",
        "create_file must not truncate an existing file"
    );
}

// ── Retained-fd rebinding validation ────────────────────────────────────────

/// Swapping the directory out from under a retained descriptor (rename away,
/// recreate at the same path) invalidates the handle: revalidation fails and
/// mutating methods refuse.
#[test]
fn rebinding_the_directory_invalidates_a_retained_descriptor() {
    let dir = TempDir::new().expect("tempdir");
    let wiki_path = dir.path().join("wiki");
    let root = DirFd::open(dir.path()).expect("open root");
    let wiki = root.ensure_private_subtree(Path::new("wiki")).expect("wiki");
    wiki.validate_private().expect("pristine handle validates");

    // Rebind: move the real directory aside, put a fresh impostor at the
    // old path.
    fs::rename(&wiki_path, dir.path().join("wiki.moved")).expect("rename away");
    fs::create_dir(&wiki_path).expect("impostor at the old path");
    set_mode(&wiki_path, 0o700);

    assert!(
        wiki.revalidate().is_err(),
        "the retained descriptor must detect the swap"
    );
    assert!(
        wiki.validate_private().is_err(),
        "validation must fail through the stale handle"
    );
    assert!(
        wiki.create_file("store.sqlite").is_err(),
        "mutation through a rebound handle must be refused"
    );

    // The impostor directory received nothing.
    assert!(
        fs::read_dir(&wiki_path).expect("read impostor").next().is_none(),
        "the swapped-in directory must stay empty"
    );
}

/// An untouched handle keeps validating, and deleting the directory out from
/// under it is detected too.
#[test]
fn revalidate_passes_when_untouched_and_fails_when_deleted() {
    let dir = TempDir::new().expect("tempdir");
    let root = DirFd::open(dir.path()).expect("open root");
    let wiki = root.ensure_private_subtree(Path::new("wiki")).expect("wiki");

    wiki.revalidate().expect("untouched handle stays valid");

    fs::remove_dir(dir.path().join("wiki")).expect("delete wiki");
    assert!(
        wiki.revalidate().is_err(),
        "a deleted directory must invalidate the handle"
    );
}
