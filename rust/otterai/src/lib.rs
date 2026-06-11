//! Unofficial Otter.ai API client, ported from the `otterai` Python package.

pub mod client;
pub mod config;

pub use client::{Client, Error, LoginData};
