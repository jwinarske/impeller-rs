//! Rendering HAL trait: the backend-agnostic contract every rendering backend
//! implements. Deliberately Vulkan-leaning -- recording commands and replaying
//! them as GL calls works, whereas extracting Vulkan-grade explicitness from an
//! immediate-mode abstraction does not.
//!
//! Two requirements must exist in the trait from day one, before any DRM code
//! is written, because retrofitting them is a breaking redesign:
//!
//! 1. External render targets -- the renderer must draw into images it did not
//!    allocate, so `TextureDescriptor` carries a dma-buf fd, DRM fourcc, and
//!    format modifier.
//! 2. Exportable sync -- `HalFence` must convert to a `sync_file` fd where the
//!    platform allows, so the render-done fence can be attached to an atomic
//!    commit as `IN_FENCE_FD`.
//!
//! Callers branch on reported capabilities, never on backend identity.
