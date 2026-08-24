//! Domain layer for pecan.
//!
//! Everything here is pure filesystem/domain logic with no HTTP or async
//! runtime dependencies: discovering pi sessions, extracting lightweight
//! summaries, parsing threads, and managing pecan's own local state
//! (project allowlist and settled threads).

pub mod error;
pub mod paths;
pub mod scan;
pub mod session;
pub mod store;
pub mod tasks;
pub mod thread;
pub mod workflows;

pub use error::{CoreError, Result};
pub use paths::PiPaths;
pub use store::StateStore;
