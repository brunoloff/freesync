//! Provider-independent synchronization state and filesystem safety.
pub mod db;
pub mod diagnostics;
pub mod engine;
pub mod error;
pub mod executor;
pub mod fake;
pub mod local;
pub mod model;
pub mod planner;
pub mod profile;
pub mod provider;
pub mod reconcile;
pub mod remote;
pub mod watcher;

pub use error::{Error, ErrorCode, Result};
pub use model::*;
pub use provider::Provider;
