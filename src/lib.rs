//! LithosDB: A rock-solid on-disk B+Tree database engine from scratch in pure Rust.

pub mod storage;
pub mod btree;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
