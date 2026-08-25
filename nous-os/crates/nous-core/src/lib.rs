//! # nous-core
//!
//! Shared foundations for NOUS OS: the wire protocol, the capability model, the
//! policy engine and the journal. Everything the daemon, the shell and the
//! control tool agree on lives here.
//!
//! The crate has no third-party dependencies, on purpose. It builds on a fresh
//! machine with nothing but a Rust toolchain, and there is no supply chain
//! between an inference result and your bootloader.

pub mod cap;
pub mod config;
pub mod ipc;
pub mod journal;
pub mod json;
pub mod log;
pub mod glyph;
pub mod policy;
pub mod proto;

pub use cap::{Capability, Risk};
pub use config::Config;
pub use journal::{Journal, Outcome, Record, Undo};
pub use json::{json_obj, Json};
pub use policy::{Decision, Policy, Subject, Verdict};
pub use proto::{Event, Frame, Plan, Request, Response, Step};

/// Version of the OS as a whole, not just this crate.
pub const NOUS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Product name, used in banners and the greeter.
pub const NOUS_NAME: &str = "NOUS";
