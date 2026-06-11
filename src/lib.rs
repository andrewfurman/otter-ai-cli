//! Otter.ai API client used by the `otter` CLI binary.
//!
//! `client.rs` mirrors every known Otter.ai endpoint; `config.rs` handles
//! credential storage in ~/.otterai/config.json.

pub mod client;
pub mod config;

pub use client::{ApiResponse, Client, Error};
