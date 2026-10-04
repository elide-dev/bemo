//! Runtime-independent transport implementation extracted from Elide.
//!
//! The Rust handle API lives in [`abi`]; unmangled C exports live only in `dokar-ffi`.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("The transport ABI requires a 64-bit target");

pub mod abi;
pub mod buffer;
pub mod driver;
pub mod http;
pub mod tls;

/// Dokar metadata ABI (distinct from the preserved Elide transport ABI 3).
pub const ABI_VERSION: u32 = 1;
/// Data-plane capabilities are advertised after the binding contracts pass.
pub const CAPABILITIES: u64 = 0;
