//! Library surface for `wiki` integration tests.
//!
//! The `wiki` crate is primarily a binary. This library target exposes just
//! enough of the binary's modules for integration tests in `tests/` to drive
//! end-to-end flows without hitting the network or shelling out.
//!
//! Only items genuinely required by integration tests should be re-exported
//! here. Do not leak internal helpers beyond what tests need.

#[path = "commands/install.rs"]
pub mod install;
