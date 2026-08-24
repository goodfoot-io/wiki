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
//! own open is outside std's reach. Non-Linux unix targets compile a reduced
//! form (`lstat` checks only; the uid check is skipped) — the supported
//! environment is Linux, where the kernel enforces `O_NOFOLLOW`.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

/// The one mode a wiki-derived private directory may have: owner-only.
pub const PRIVATE_DIR_MODE: u32 = 0o700;

/// Creation mode for lock and database files: owner read/write only.
const PRIVATE_FILE_MODE: u32 = 0o600;

#[cfg(target_os = "linux")]
const DIR_OPEN_FLAGS: i32 = libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC;
#[cfg(target_os = "linux")]
const FILE_OPEN_FLAGS: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;
#[cfg(not(target_os = "linux"))]
const DIR_OPEN_FLAGS: i32 = 0;
#[cfg(not(target_os = "linux"))]
const FILE_OPEN_FLAGS: i32 = 0;

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
        Ok(Self {
            file,
            path: path.to_path_buf(),
            dev: meta.dev(),
            ino: meta.ino(),
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
        if !held.is_dir() || held.dev() != self.dev || held.ino() != self.ino {
            return Err(rebound_error(
                &self.path,
                &format!("{}:{}", held.dev(), held.ino()),
                &format!("{}:{}", self.dev, self.ino),
            ));
        }
        match fs::symlink_metadata(&self.path) {
            Ok(now)
                if !now.is_symlink()
                    && now.is_dir()
                    && now.dev() == self.dev
                    && now.ino() == self.ino =>
            {
                Ok(())
            }
            Ok(now) => Err(rebound_error(
                &self.path,
                &format!("{}:{}", now.dev(), now.ino()),
                &format!("{}:{}", self.dev, self.ino),
            )),
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
                    fs::set_permissions(
                        &current,
                        fs::Permissions::from_mode(PRIVATE_DIR_MODE),
                    )?;
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
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(FILE_OPEN_FLAGS)
            .open(&path)
    }
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => Err(symlink_refused(path)),
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
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
