//! Rig core: single-user harness protocol, relay bus, primitive ops, security.
//! Zero dependencies: mirrors the Python POC in `../relay/` with closed
//! schemas enforced by the type system instead of runtime checks.

pub mod acp;
pub mod bus;
pub mod guard;
pub mod mcpgate;
pub mod memory;
pub mod ooxml;
pub mod ops;
pub mod paths;
pub mod protocol;
pub mod provider;
pub mod queue;
pub mod router;
pub mod runner;
pub mod security;
pub mod sessions;
pub mod snapshots;
pub mod stream;
pub mod vfs;

pub use bus::Relay;
pub use protocol::{Op, HarnessError, StructVerb};
