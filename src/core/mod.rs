//! Protocol-free live search, guarded fetching, configuration and passage selection.
//! Frontends compose this service without introducing protocol dependencies here.

pub mod config;
pub mod engines;
pub mod fetch;
mod service;
pub mod text;

pub use config::Config;
pub use fetch::Page;
pub use service::{Answer, Link, Query, Search, SearchError};
