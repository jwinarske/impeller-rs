//! GLES 3.0 backend via glow, with contexts from EGL.
//!
//! Commands are recorded and replayed as GL calls rather than issued as they
//! arrive, which is why the HAL hands a whole batch over: a record-and-replay
//! backend wants to see the entire scene before touching any GL state, so it
//! can order binds and avoid redundant ones.
//!
//! GLES 3.0 is the floor. GLES 2.0 is permanently out of scope; the feature gap
//! is too large to bridge.
//!
//! # Status
//!
//! Context creation and capability detection. Rendering is not implemented yet,
//! so the `Hal` traits are not claimed: a backend that answered every call with
//! an error would look usable and fail at the first draw.

pub mod context;

pub use context::{DisplayTarget, GlesContext};
