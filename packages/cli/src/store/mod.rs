//! Hardened filesystem foundation for the consolidated wiki store (plan D9):
//! every creation path under the git common dir is routed through
//! [`fd`] — descriptor-validated, symlink-refusing, mode-checked.

pub mod fd;
