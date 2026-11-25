//! Native PTY sessions for interactive terminals and headless delegated agents.

pub mod manager;
pub mod process_group;
pub mod ring;

pub use manager::{PtyManager, PtySession, SpawnSpec};
