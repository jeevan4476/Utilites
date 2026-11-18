pub mod cli;
pub mod network;
pub mod qr;
pub mod server;
pub mod web_assets;
pub mod zip;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
