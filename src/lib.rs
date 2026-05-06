//! Hermetic is a Rust CLI/library for building Railgun transactions while
//! keeping public-chain RPC and SDK reverse requests on Rust-owned Tor egress.

pub mod cli;

pub(crate) mod embedded;
pub(crate) mod eth;
pub(crate) mod railgun;
pub(crate) mod tor;
