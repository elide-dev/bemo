//! Runtime-independent transport implementation extracted from Elide.
//!
//! The Rust handle API lives in [`abi`]; unmangled C exports live only in `bemo-ffi`.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("The transport ABI requires a 64-bit target");

pub mod abi;
pub mod alloc;
pub mod buffer;
pub mod compression;
pub mod driver;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
pub mod http;
pub mod tls;

/// Bemo metadata ABI (distinct from the preserved Elide transport ABI 3).
pub const ABI_VERSION: u32 = 1;
/// The complete Elide transport ABI 3 boundary is available.
pub const CAP_TRANSPORT_V3: u64 = 1;
/// `bemo_alloc`, `bemo_free`, and `bemo_alloc_live` allocate from the transport's allocator.
pub const CAP_NATIVE_ALLOC: u64 = 2;
/// Complete HTTP/1 responses can report physical completion.
pub const CAP_HTTP1_COMPLETE_CREDIT: u64 = 4;
/// Implemented data-plane capabilities.
pub const CAPABILITIES: u64 = CAP_TRANSPORT_V3 | CAP_NATIVE_ALLOC | CAP_HTTP1_COMPLETE_CREDIT;
