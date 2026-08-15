//! Error types shared across every backend.

use thiserror::Error;

/// Result alias used throughout the HAL.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    /// The backend or driver does not support the requested operation.
    ///
    /// Callers are expected to have checked [`crate::Capabilities`] first;
    /// receiving this is a bug in the caller, not a condition to branch on at
    /// the call site.
    #[error("unsupported by this backend: {0}")]
    Unsupported(&'static str),

    /// Device memory or host memory exhausted.
    #[error("out of memory allocating {what}")]
    OutOfMemory { what: &'static str },

    /// A requested resource exceeded a reported device limit.
    #[error("{what} of {requested} exceeds device limit of {limit}")]
    LimitExceeded {
        what: &'static str,
        requested: u64,
        limit: u64,
    },

    /// A dma-buf could not be imported, usually a format or modifier the
    /// importing device does not accept.
    #[error("failed to import external image: {0}")]
    ExternalImport(String),

    /// The device was lost. Everything created from it is invalid.
    #[error("device lost")]
    DeviceLost,

    /// A wait exceeded its timeout without the fence signaling.
    #[error("timed out waiting on fence")]
    Timeout,

    /// Backend-specific failure that has no portable representation.
    #[error("{backend} error: {detail}")]
    Backend {
        backend: &'static str,
        detail: String,
    },
}
