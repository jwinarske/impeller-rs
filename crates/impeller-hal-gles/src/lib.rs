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
//! Offscreen rendering of solid-colour geometry, with blending. Multisampling
//! is not implemented: it needs a multisample renderbuffer and a blit resolve,
//! which is a different framebuffer shape rather than a flag, and a
//! multisampled pass is refused rather than silently rendering aliased.

pub mod context;
pub mod hal;
pub mod render;

pub use context::{DisplayTarget, GlesContext};
pub use hal::GlesHal;
pub use render::GlesTexture;
