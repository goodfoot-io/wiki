//! Library surface for `wiki` integration tests.
//!
//! The `wiki` crate is primarily a binary. This library target exposes just
//! enough of the binary's modules for integration tests in `tests/` to drive
//! end-to-end flows without hitting the network or shelling out.
//!
//! Only items genuinely required by integration tests should be re-exported
//! here. Do not leak internal helpers beyond what tests need.

pub mod cache;
mod concurrency;
pub mod frontmatter;
pub mod git;
pub mod index;
// `index` needs `perf` for its scope events; the command-lifecycle half
// (init/finish/spans) is called only from the binary's main. Public so the
// lib build does not flag that half as dead — the binary build, where every
// module is private, remains the dead-code check for it.
pub mod perf;
pub mod store;
pub mod wikiignore;
