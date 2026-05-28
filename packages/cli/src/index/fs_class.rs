//! `statfs` magic detection — overlayfs / NFS / CIFS / FUSE classify as
//! [`crate::index::HostileFs::Yes`] and disable the dir-mtime Merkle
//! optimization in Pass 3.

use std::path::Path;

use crate::index::HostileFs;

#[cfg(target_os = "linux")]
pub fn detect(dot_git: &Path) -> HostileFs {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = CString::new(dot_git.as_os_str().as_bytes()) else {
        return HostileFs::Yes;
    };

    // SAFETY: `statfs64` accepts a NUL-terminated C string and a pointer to an
    // uninitialized `libc::statfs64`; both are valid for the call's duration.
    let mut buf: libc::statfs64 = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs64(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        return HostileFs::Yes;
    }

    // Hostile magics (libc::statfs64.f_type is i64/i32 depending on arch — cast
    // to u64 for comparison).
    let f_type = buf.f_type as u64;
    const OVERLAYFS: u64 = 0x794c7630;
    const NFS: u64 = 0x6969;
    const CIFS: u64 = 0xff534d42;
    const FUSE: u64 = 0x65735546;

    match f_type {
        OVERLAYFS | NFS | CIFS | FUSE => HostileFs::Yes,
        _ => HostileFs::No,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn detect(_dot_git: &Path) -> HostileFs {
    // CARD.md requires fail-closed on hostile filesystems. `statfs` magic
    // detection is Linux-only; without it we cannot distinguish overlayfs /
    // NFS / CIFS / FUSE from APFS or NTFS. Conservatively treat all
    // non-Linux filesystems as hostile so Pass 3 always runs a full rescan.
    // This is slow but correct; the dir-mtime Merkle optimisation is Linux-
    // only anyway.
    HostileFs::Yes
}

#[cfg(not(target_os = "linux"))]
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detect_non_linux_is_hostile() {
        // Non-Linux platforms must return HostileFs::Yes (fail-closed contract).
        assert_eq!(detect(Path::new("/any/path")), HostileFs::Yes);
    }
}
