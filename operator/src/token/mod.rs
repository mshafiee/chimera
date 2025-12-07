//! Token safety validation module
//!
//! Implements the Fast/Slow path pattern for token validation:
//! - Fast Path (Ingress): Check freeze/mint authority from cached metadata
//! - Slow Path (Executor): Honeypot detection via transaction simulation

mod cache;
mod metadata;
mod parser;
mod pools;

pub use cache::*;
pub use metadata::*;
pub use parser::*;
pub use pools::*;
