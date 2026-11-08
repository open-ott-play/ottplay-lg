//! App-owned local companion lifecycle. No public proxy or media-server endpoint.
pub mod client;
pub mod protocol;
mod supervisor;
pub use supervisor::{Configuration, ErasureStop, Event, Supervisor};
