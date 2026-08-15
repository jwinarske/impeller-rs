//! Rendering HAL trait: the backend-agnostic contract every rendering backend
//! implements.
//!
//! The HAL answers how draw commands become pixels in a GPU image.
//! Presentation — how a finished image reaches a display, and how the frame
//! loop is paced — is a separate, orthogonal axis and lives in
//! `impeller-present`.
//!
//! # Shape of the trait
//!
//! Deliberately Vulkan-leaning. Recording commands and replaying them as GL
//! calls works; extracting Vulkan-grade explicitness from an immediate-mode
//! abstraction does not. GLES pays a small CPU cost for command recording, and
//! that is the accepted trade. The trait is never reduced to a lowest common
//! denominator to accommodate a weaker backend: backends implement, emulate,
//! or capability-gate.
//!
//! # Two requirements present from day one
//!
//! Both exist for the sake of the DRM presentation path, and both are here
//! before any DRM code, because retrofitting either is a breaking redesign:
//!
//! 1. **External render targets** — [`ExternalImageDesc`] lets the renderer
//!    draw into images it did not allocate, carrying dma-buf fd, DRM fourcc,
//!    and format modifier.
//! 2. **Exportable sync** — [`HalFence::export_sync_file`] converts GPU
//!    completion into a `sync_file` fd that can ride an atomic commit as
//!    `IN_FENCE_FD`.
//!
//! # Branch on capabilities, never on backend identity
//!
//! [`Capabilities`] is the only thing layers above the HAL are allowed to
//! condition on. A Mali GPU lacking fence export and a desktop Vulkan driver
//! lacking `VK_KHR_external_fence_fd` are the same problem, and code asking
//! "is this GLES?" instead of "can this export a fence?" gets both wrong.

pub mod capabilities;
pub mod error;
pub mod format;
pub mod resource;
pub mod sync;

pub use capabilities::{Capabilities, DmaBufSupport, SampleCounts, SyncSupport};
pub use error::{Error, Result};
pub use format::{Extent2D, FormatModifierSet, Fourcc, Modifier, PixelFormat};
pub use resource::{BufferDescriptor, BufferUsage, TextureDescriptor, TextureUsage};
pub use sync::{HalFence, FRAME_WAIT_TIMEOUT};

#[cfg(unix)]
pub use resource::{DmaBufPlane, ExternalImageDesc};

use std::sync::Arc;

/// A rendering backend, as a family of associated types.
///
/// Implementors are zero-sized markers; the types they name carry the work.
pub trait Hal: 'static {
    type Context: HalContext<Hal = Self>;
    type CommandBuffer: HalCommandBuffer<Hal = Self>;
    type Pipeline: Send + Sync;
    type Buffer: Send + Sync;
    type Texture: Send + Sync;
    type Sampler: Send + Sync;
    type Fence: HalFence;

    /// Backend name, for logs and report fingerprints.
    const NAME: &'static str;
}

/// A device, and the resources created from it.
///
/// `Send + Sync` because resource creation is thread-safe. Canvas recording is
/// single-threaded per frame, but nothing forces allocation onto that thread.
pub trait HalContext: Send + Sync {
    type Hal: Hal;

    /// What this device can do. The only thing callers branch on.
    fn capabilities(&self) -> &Capabilities;

    fn create_command_buffer(&self) -> Result<<Self::Hal as Hal>::CommandBuffer>;

    /// Allocate a texture, or import an external image when the descriptor
    /// carries one.
    fn create_texture(&self, desc: &TextureDescriptor) -> Result<<Self::Hal as Hal>::Texture>;

    fn create_buffer(&self, desc: &BufferDescriptor) -> Result<<Self::Hal as Hal>::Buffer>;

    /// Submit recorded work, returning a fence that signals on completion.
    fn submit(&self, cmd: <Self::Hal as Hal>::CommandBuffer) -> Result<<Self::Hal as Hal>::Fence>;

    /// Export a texture as a dma-buf for scanout or cross-device sharing.
    ///
    /// Returns [`Error::Unsupported`] where
    /// [`DmaBufSupport::export`] is false; the DRM path then allocates through
    /// GBM and imports instead.
    #[cfg(unix)]
    fn export_texture(&self, _texture: &<Self::Hal as Hal>::Texture) -> Result<ExternalImageDesc> {
        Err(Error::Unsupported("dma-buf export"))
    }
}

/// Recorded work, encoded once and submitted.
///
/// Draw commands are buffered per pass, sorted by pipeline, and encoded at
/// pass end rather than issued as they arrive.
pub trait HalCommandBuffer: Send {
    type Hal: Hal;

    /// Begin a render pass targeting `target`.
    fn begin_render_pass(
        &mut self,
        target: &<Self::Hal as Hal>::Texture,
        clear: Option<[f32; 4]>,
    ) -> Result<()>;

    fn end_render_pass(&mut self) -> Result<()>;

    fn bind_pipeline(&mut self, pipeline: &Arc<<Self::Hal as Hal>::Pipeline>) -> Result<()>;

    /// Finish recording. The buffer is submitted through
    /// [`HalContext::submit`].
    fn finish(&mut self) -> Result<()>;
}
