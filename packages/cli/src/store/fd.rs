//! Descriptor-hardened filesystem primitives for everything the wiki store
//! writes under the git common dir (plan D9). Hardening precedes any new
//! write path going live: the merged store's directory, lock files, and
//! database file are all created through here.
//!
//! Guarantees, in the smallest correct form:
//!
//! * **Canonical dir opened once** — [`DirFd::open`] opens the target with
//!   `O_DIRECTORY` semantics (the open fails unless the final component is a
//!   real directory) and captures its `(device, inode)` pair.
//! * **Symlink refusal on descent** — every component is `lstat`-checked on
//!   every platform, and on Linux the final component is additionally opened
//!   `O_NOFOLLOW`, so a symlink swapped into the descent path is refused,
//!   never followed.
//! * **Owner + mode validated before mutating** — [`DirFd::validate_private`]
//!   fstats the *retained descriptor* (no path race) and requires ownership
//!   by the effective uid and exactly mode `0700`: group and other hold no
//!   rights on wiki-derived state.
//! * **Private subtrees** — [`DirFd::ensure_private_subtree`] creates missing
//!   components mode `0700` (explicitly re-applied, umask-independent), then
//!   re-opens and validates each through the strict path above.
//! * **Retained-fd rebinding validation** — [`DirFd::revalidate`] requires
//!   agreement between three views: the fstat of the held descriptor, its
//!   captured `(device, inode)` pair, and the `lstat` of the live path. A
//!   directory swapped by rename/recreate invalidates the handle, and every
//!   mutating method revalidates first.
//!
//! Known boundary: SQLite opens the database by path, so the final handoff
//! to SQLite cannot carry the descriptor — this module pins, validates, and
//! pre-creates the file; the residual window between validation and SQLite's
//! own open is outside std's reach. Non-Linux targets compile a reduced
//! form: other unix targets skip the uid check, and Windows additionally
//! swaps `(device, inode)` for `(volume serial, file index)` (zeros when the
//! filesystem cannot report them, degrading revalidation to the lstat
//! checks) and skips POSIX mode validation entirely — NTFS has no mode
//! bits, so privacy rests on the ACLs inherited from the store directory.
//! The supported environment is Linux, where the kernel enforces
//! `O_NOFOLLOW`.

use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as WindowsOpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// The one mode a wiki-derived private directory may have: owner-only.
/// Enforced on unix (see `apply_private_dir_mode`); retained as the
/// documented contract on targets without POSIX mode bits.
#[cfg_attr(not(unix), allow(dead_code))]
pub const PRIVATE_DIR_MODE: u32 = 0o700;

/// Creation mode for lock and database files: owner read/write only.
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

#[cfg(target_os = "linux")]
const DIR_OPEN_FLAGS: i32 = libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC;
#[cfg(target_os = "linux")]
const FILE_OPEN_FLAGS: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;
#[cfg(all(unix, not(target_os = "linux")))]
const DIR_OPEN_FLAGS: i32 = 0;
#[cfg(all(unix, not(target_os = "linux")))]
const FILE_OPEN_FLAGS: i32 = 0;
// Win32 `CreateFileW` flags. `FILE_FLAG_BACKUP_SEMANTICS` is what makes
// opening a directory handle legal at all; `FILE_FLAG_OPEN_REPARSE_POINT`
// refuses to traverse a symlink final component — the `O_NOFOLLOW`
// analogue. Typed `u32` to match `OpenOptionsExt::custom_flags` here.
#[cfg(windows)]
const DIR_OPEN_FLAGS: u32 = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
#[cfg(windows)]
const FILE_OPEN_FLAGS: u32 = FILE_FLAG_OPEN_REPARSE_POINT;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// A retained, validated descriptor for one directory under the store's
/// subtree. Cloning the `PathBuf` would reopen the race this type exists to
/// close; hold the handle.
pub struct DirFd {
    file: File,
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl DirFd {
    /// Open `path` as a directory, refusing a symlink final component and
    /// capturing its identity for later rebinding validation.
    pub fn open(path: &Path) -> io::Result<Self> {
        reject_symlink(path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(DIR_OPEN_FLAGS)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: not a directory", path.display()),
            ));
        }
        let (dev, ino) = file_identity(&file)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            dev,
            ino,
        })
    }

    /// Validate the retained directory before mutating it (plan D9): it must
    /// still be the directory that was opened, owned by the effective uid,
    /// with exactly the private mode.
    pub fn validate_private(&self) -> io::Result<()> {
        self.revalidate()?;
        validate_private_meta(&self.file.metadata()?, &self.path)
    }

    /// Retained-fd rebinding validation: the held descriptor must still be
    /// the captured directory *and* the path it was opened from must still
    /// name that same `(device, inode)` — a swap (rename over,
    /// remove-and-recreate, symlink substitution, deletion) fails here
    /// instead of silently redirecting writes into an impostor directory.
    pub fn revalidate(&self) -> io::Result<()> {
        let held = self.file.metadata()?;
        let held_id = file_identity(&self.file)?;
        if !held.is_dir() || held_id != (self.dev, self.ino) {
            return Err(rebound_error(
                &self.path,
                &format!("{}:{}", held_id.0, held_id.1),
                &format!("{}:{}", self.dev, self.ino),
            ));
        }
        match fs::symlink_metadata(&self.path) {
            Ok(now) => {
                let now_id = identity_at(&self.path, &now)?;
                if !now.is_symlink() && now.is_dir() && now_id == (self.dev, self.ino) {
                    Ok(())
                } else {
                    Err(rebound_error(
                        &self.path,
                        &format!("{}:{}", now_id.0, now_id.1),
                        &format!("{}:{}", self.dev, self.ino),
                    ))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Ensure `rel` exists beneath the retained directory, creating missing
    /// components mode `0700`, refusing symlink or non-directory components,
    /// validating each level, and returning the leaf as a retained
    /// descriptor. Only normal relative components are accepted.
    pub fn ensure_private_subtree(&self, rel: &Path) -> io::Result<DirFd> {
        self.revalidate()?;
        let mut current = self.path.clone();
        for component in rel.components() {
            let segment = match component {
                Component::Normal(segment) => segment,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{rel:?}: only normal relative components are allowed"),
                    ));
                }
            };
            current.push(segment);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.is_symlink() => return Err(symlink_refused(&current)),
                Ok(meta) if !meta.is_dir() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{}: not a directory", current.display()),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                    apply_private_dir_mode(&current)?;
                }
                Err(e) => return Err(e),
            }
        }
        // Strict reopen pins the final component and captures its identity;
        // validation proves the whole descent landed on private ground.
        let leaf = DirFd::open(&current)?;
        leaf.validate_private()?;
        Ok(leaf)
    }

    /// Create-or-open `name` directly inside the retained directory: symlink
    /// refusal (`lstat` everywhere, `O_NOFOLLOW` on Linux), creation mode
    /// `0600`, no truncation — lock files and the database file arrive here
    /// before SQLite ever sees the path.
    pub fn create_file(&self, name: &str) -> io::Result<File> {
        self.revalidate()?;
        if name.contains('/') || name.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name:?}: must be a plain file name"),
            ));
        }
        let path = self.path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_symlink() => return Err(symlink_refused(&path)),
            Ok(meta) if !meta.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{}: not a regular file", path.display()),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false);
        apply_private_file_options(&mut options);
        options.open(&path)
    }
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => Err(symlink_refused(path)),
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The captured identity pair behind rebinding validation, read from a
/// retained handle: `(device, inode)` via `fstat` on unix; `(volume serial,
/// file index)` via `GetFileInformationByHandle` on Windows — std's
/// by-handle accessors are still nightly-gated (`windows_by_handle`). A
/// filesystem that reports no index yields zeros, which degrades
/// revalidation to the path-lstat checks — the documented reduced form for
/// non-Linux targets — instead of failing every open.
#[cfg(unix)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    // SAFETY: `info` is a valid, fully initialized `BY_HANDLE_FILE_INFORMATION`
    // for the call's duration and the retained handle is owned by `file`.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the raw handle is live (borrowed from `file`) and `info` is a
    // correctly sized out-parameter.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

/// Identity pair for the rebinding check's path-side view. Unix reads it
/// straight off the `lstat` result. Windows has no stable by-handle access
/// from `symlink_metadata`, so it re-opens the path with the module's
/// no-follow directory flags and queries the resulting handle — that open
/// itself refuses a symlinked final component, preserving the check's
/// swap-detection semantics.
#[cfg(unix)]
fn identity_at(_path: &Path, meta: &fs::Metadata) -> io::Result<(u64, u64)> {
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn identity_at(path: &Path, _meta: &fs::Metadata) -> io::Result<(u64, u64)> {
    let probe = OpenOptions::new()
        .read(true)
        .custom_flags(DIR_OPEN_FLAGS)
        .open(path)?;
    file_identity(&probe)
}

/// Create-time hardening for directories: explicit `chmod 0700`,
/// umask-independent. Windows has no POSIX mode bits to apply; a fresh
/// directory inherits the parent's ACLs (the documented reduced form).
#[cfg(unix)]
fn apply_private_dir_mode(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE))
}

#[cfg(windows)]
fn apply_private_dir_mode(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Creation-mode + no-follow flags for files under the store. Unix applies
/// an explicit `0600` (umask-independent); Windows has no POSIX mode bits —
/// new files inherit the parent directory's ACLs (the documented reduced
/// form).
#[cfg(unix)]
fn apply_private_file_options(options: &mut OpenOptions) {
    options.mode(PRIVATE_FILE_MODE).custom_flags(FILE_OPEN_FLAGS);
}

#[cfg(windows)]
fn apply_private_file_options(options: &mut OpenOptions) {
    options.custom_flags(FILE_OPEN_FLAGS);
}

fn symlink_refused(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{}: symlink refused", path.display()),
    )
}

fn rebound_error(path: &Path, found: &str, expected: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{}: directory was rebound (identity {found}, expected {expected})",
            path.display()
        ),
    )
}

fn validate_private_meta(meta: &fs::Metadata, path: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{}: owner uid {} is not the effective uid {}",
                    path.display(),
                    meta.uid(),
                    unsafe { libc::geteuid() }
                ),
            ));
        }
    }
    #[cfg(unix)]
    {
        let mode = meta.permissions().mode() & 0o777;
        if mode != PRIVATE_DIR_MODE {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{}: mode {:o} is not private (expected {:o})",
                    path.display(),
                    mode,
                    PRIVATE_DIR_MODE
                ),
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        // No POSIX mode bits exist to validate on Windows; privacy rests on
        // the ACLs inherited from the store directory (the documented
        // reduced form). Identity was already proven by `revalidate`.
        let _ = (meta, path);
        Ok(())
    }
}
