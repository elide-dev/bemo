//! Runtime-independent transport core.
//!
//! This foundation exposes compatibility metadata. Socket, buffer, and TLS
//! capabilities will be advertised only after their implementation is migrated.
#![forbid(unsafe_code)]

/// Version of Dokar's C ABI (independent of Elide's existing transport ABI).
pub const ABI_VERSION: u32 = 1;
/// Implemented transport capabilities. Zero means no data-plane implementation.
pub const CAPABILITIES: u64 = 0;
