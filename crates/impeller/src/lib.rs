//! Public facade: the crate applications depend on.
//!
//! Re-exports the public API and carries the feature flags that select which
//! rendering backends and presentation targets are compiled in. The frame loop
//! is identical across every combination -- acquire a frame, record a canvas,
//! finish to get a sync handle, present it -- which is what keeps the
//! rendering and presentation axes orthogonal at the API level.
