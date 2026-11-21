//! Byte-anchored, offline session projections.
pub mod model;
pub mod provider;
pub mod parser;
pub mod graph;
mod access;
mod query;
pub mod store;
pub use model::{Capture, Source};
pub use provider::{ByteProvider, ByteResult, LocalProvider, Binding};
pub use store::Core;
pub mod json;
mod output;
pub mod admission;
pub mod limits;

pub mod scope;

mod deletion_binding;
