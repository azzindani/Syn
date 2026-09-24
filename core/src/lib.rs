//! Core: single-user session protocol, relay bus, primitive ops, security.
//! Zero dependencies: mirrors the Python POC in `../relay/` with closed
//! schemas enforced by the type system instead of runtime checks.

pub mod acp;
pub mod agent;
pub mod auth;
pub mod batch;
pub mod bus;
pub mod catalog;
pub mod cdp;
pub mod desk;
pub mod chats;
pub mod coach;
pub mod config;
pub mod guard;
pub mod hand;
pub mod json;
pub mod labels;
pub mod live;
pub mod looptools;
pub mod manual;
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
pub mod shell;
pub mod snapshots;
pub mod stream;
pub mod summarise;
pub mod surface;
pub mod tools;
pub mod vfs;
pub mod ws;

pub use bus::Relay;
pub use protocol::{Op, Error, StructVerb};
