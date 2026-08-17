//! DRM/KMS direct scanout: rendering straight to a display with no compositor.
//!
//! This is the configuration embedded, automotive, kiosk, and industrial
//! products actually ship, and the one most renderers leave as an exercise. It
//! is a presentation target, not a rendering backend: the renderer draws through
//! a HAL as always, and this decides how the result reaches a panel.
//!
//! Everything to do with KMS belongs to drm-rs and is consumed through
//! [`output::ScanoutOutput`], which states exactly what this crate requires.
//! Expressing it as a trait lets the two projects be sequenced against each
//! other, and makes the parts most worth testing — ring accounting and fence
//! plumbing — testable without a display attached.

pub mod device;
pub mod output;
pub mod target;

pub use output::{CommitRequest, DmaBufPlanes, FbHandle, Mode, OutputEvent, ScanoutOutput};
pub use target::{DrmScanoutTarget, DEFAULT_RING_DEPTH};
