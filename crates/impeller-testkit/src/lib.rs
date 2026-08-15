//! Shared test harness used by every tier, so a test written once runs
//! everywhere.
//!
//! Scenes are data, not Rust code: one versioned corpus drives golden,
//! conformance, perf, and on-device runs across every backend and presentation
//! combination, so a new feature adds scenes once and the matrix multiplies
//! coverage automatically.
//!
//! Executors cover offscreen, WSI, and DRM. Comparators support per-channel
//! tolerance with an outlier budget, perceptual comparison where cross-driver
//! float variance is expected, and CRC equality for scanout validation.
//! Tolerance tables are checked in: tightening one is a normal change,
//! loosening one requires a linked driver bug.
