//! A rendering context, over whichever backend is available.

use crate::backend::{no_backend, Backend, BackendPreference};
use impeller_core::Recording;
use impeller_hal::{Capabilities, Error, Extent2D, HalContext, PixelFormat, Result};

/// A device to render with.
///
/// Which backend is behind it is decided at creation and reported by
/// [`Context::backend`]. Callers branch on [`Context::capabilities`] rather than
/// on which backend it turned out to be: a device without fence export is the
/// same problem whether it is Vulkan or GLES.
pub enum Context {
    #[cfg(feature = "vulkan")]
    Vulkan(impeller_hal_vulkan::VulkanContext),
    #[cfg(feature = "gles")]
    Gles(impeller_hal_gles::GlesContext),
}

impl Context {
    /// Create a context, choosing a backend.
    pub fn new(preference: BackendPreference) -> Result<Self> {
        let mut attempts = Vec::new();

        #[cfg(feature = "vulkan")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Vulkan
        ) {
            match impeller_hal_vulkan::VulkanContext::new(
                impeller_hal_vulkan::DevicePreference::Auto,
            ) {
                Ok(ctx) => return Ok(Self::Vulkan(ctx)),
                Err(e) => attempts.push((Backend::Vulkan, e)),
            }
        }

        #[cfg(feature = "gles")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Gles
        ) {
            match impeller_hal_gles::GlesContext::new(impeller_hal_gles::DisplayTarget::Surfaceless)
            {
                Ok(ctx) => return Ok(Self::Gles(ctx)),
                Err(e) => attempts.push((Backend::Gles, e)),
            }
        }

        let _ = preference;
        Err(no_backend(attempts))
    }

    pub fn backend(&self) -> Backend {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Backend::Vulkan,
            #[cfg(feature = "gles")]
            Self::Gles(_) => Backend::Gles,
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::capabilities(ctx),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::capabilities(ctx),
        }
    }

    /// Create a surface to draw into.
    pub fn create_surface(&mut self, extent: Extent2D, format: PixelFormat) -> Result<Surface> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => Ok(Surface::Vulkan(HalContext::create_texture(
                ctx,
                &impeller_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => Ok(Surface::Gles(HalContext::create_texture(
                ctx,
                &impeller_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
        }
    }

    /// Draw a recording into a surface.
    pub fn draw(&mut self, surface: &mut Surface, recording: &Recording) -> Result<()> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                HalContext::submit_batch(ctx, texture, &recording.batch, recording.pass)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => {
                HalContext::submit_batch(ctx, texture, &recording.batch, recording.pass)
            }
            // A surface belongs to the context that made it; pairing one with
            // another context would use a handle the device never allocated.
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Read a surface back as tightly packed RGBA8.
    pub fn read(&mut self, surface: &mut Surface) -> Result<Vec<u8>> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => HalContext::read_texture(ctx, texture),
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::read_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Release a surface.
    ///
    /// Explicit because the memory belongs to the context, which a surface
    /// cannot reach from its own `Drop`.
    pub fn destroy_surface(&mut self, surface: Surface) {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                HalContext::destroy_texture(ctx, texture)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::destroy_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

/// Something to draw into.
pub enum Surface {
    #[cfg(feature = "vulkan")]
    Vulkan(impeller_hal_vulkan::VulkanTexture),
    #[cfg(feature = "gles")]
    Gles(impeller_hal_gles::GlesTexture),
}

impl Surface {
    pub fn extent(&self) -> Extent2D {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.extent(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.extent(),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.format(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.format(),
        }
    }
}
