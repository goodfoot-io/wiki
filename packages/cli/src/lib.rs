//! Library surface for `wiki` integration tests.
//!
//! The `wiki` crate is primarily a binary. This library target exposes just
//! enough of the binary's modules for integration tests in `tests/` to drive
//! end-to-end flows without hitting the network or shelling out.
//!
//! Only items genuinely required by integration tests should be re-exported
//! here. Do not leak internal helpers beyond what tests need.

pub mod frontmatter;
pub mod git;
pub mod index;
// The command-lifecycle half of `perf` (init/finish/spans) is only called
// from the binary's main; the lib target needs the module solely for the
// scope events inside `index`.
#[allow(dead_code)]
mod perf;
pub mod wikiignore;
