//! Resource descriptions: what the renderer asks a backend to allocate.

use crate::format::{Extent2D, Fourcc, Modifier, PixelFormat};

/// How a texture will be used, which backends need up front to choose a
/// layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureUsage {
    /// Bound as a shader input.
    pub sampled: bool,
    /// Used as a color attachment.
    pub render_target: bool,
    /// Source or destination of a copy.
    pub transfer: bool,
    /// Will be exported for scanout.
    ///
    /// Backends that can allocate with explicit modifiers use this to pick a
    /// layout the display controller accepts, rather than one that is optimal
    /// for rendering alone.
    pub scanout: bool,
}

impl TextureUsage {
    /// A plain offscreen render target: the golden and conformance suites run
    /// entirely on these.
    pub const fn offscreen() -> Self {
        Self {
            sampled: true,
            render_target: true,
            transfer: true,
            scanout: false,
        }
    }
}

/// One plane of a dma-buf image.
#[cfg(unix)]
#[derive(Debug)]
pub struct DmaBufPlane {
    /// The dma-buf file descriptor. Ownership transfers to the importer.
    pub fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// An image that already exists, to be imported rather than allocated.
///
/// This is the first of the two requirements that exist in the HAL from day
/// one for the sake of the DRM path. The renderer must be able to draw into
/// images it did not allocate — buffers GBM made, or buffers another device
/// allocated and shared — because on a split render/display SoC the display
/// controller and the GPU are different devices with different ideas about
/// memory layout.
#[cfg(unix)]
#[derive(Debug)]
pub struct ExternalImageDesc {
    /// Planes making up the image. Most formats are single-plane.
    pub planes: Vec<DmaBufPlane>,
    /// The DRM format code the buffer was allocated with.
    pub fourcc: Fourcc,
    /// The layout the buffer was allocated with.
    ///
    /// [`Modifier::INVALID`] is accepted only where no negotiation took place;
    /// a buffer that came out of format negotiation always carries the
    /// explicit modifier that was agreed on.
    pub modifier: Modifier,
}

/// What to allocate, or what to import.
#[derive(Debug)]
pub struct TextureDescriptor {
    pub extent: Extent2D,
    pub format: PixelFormat,
    pub usage: TextureUsage,
    /// Sample count for MSAA targets. 1 means single-sampled.
    pub sample_count: u32,
    /// When present, import this existing image instead of allocating.
    #[cfg(unix)]
    pub external: Option<ExternalImageDesc>,
}

impl TextureDescriptor {
    /// An offscreen render target, the workhorse of the test suites.
    pub fn offscreen(extent: Extent2D, format: PixelFormat) -> Self {
        Self {
            extent,
            format,
            usage: TextureUsage::offscreen(),
            sample_count: 1,
            #[cfg(unix)]
            external: None,
        }
    }

    /// Whether this describes an import rather than an allocation.
    pub fn is_external(&self) -> bool {
        #[cfg(unix)]
        {
            self.external.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

/// How a buffer will be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BufferUsage {
    pub vertex: bool,
    pub index: bool,
    pub uniform: bool,
    pub transfer: bool,
}

#[derive(Debug, Clone)]
pub struct BufferDescriptor {
    pub size: u64,
    pub usage: BufferUsage,
    /// Whether the host needs to write to this buffer directly, as per-frame
    /// ring allocations do.
    pub host_visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_targets_are_not_external() {
        let desc = TextureDescriptor::offscreen(Extent2D::new(1920, 1080), PixelFormat::Rgba8Unorm);
        assert!(!desc.is_external());
        assert_eq!(desc.sample_count, 1);
        assert!(desc.usage.render_target);
        // Offscreen targets are read back and sampled, so both must be set for
        // the golden suites to work.
        assert!(desc.usage.transfer);
        assert!(desc.usage.sampled);
        assert!(!desc.usage.scanout);
    }

    #[test]
    fn scanout_usage_is_distinct_from_render_target_usage() {
        // A backend allocating for scanout must pick a layout the display
        // controller accepts, which is not necessarily the one that is fastest
        // to render into, so the two flags cannot be conflated.
        let usage = TextureUsage {
            scanout: true,
            ..TextureUsage::offscreen()
        };
        assert!(usage.scanout);
        assert!(usage.render_target);
        assert!(!TextureUsage::offscreen().scanout);
    }
}
