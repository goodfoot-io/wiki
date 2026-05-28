//! `statfs` magic detection — overlayfs / NFS / CIFS classify as
//! [`crate::index::HostileFs::Yes`] and disable the dir-mtime Merkle
//! optimization in Pass 3. Phase 1 skeleton.
