//! The backend's implementation of the rendering HAL.
//!
//! Thin by design: every method forwards to an inherent method on
//! [`VulkanContext`]. Keeping the concrete API usable directly matters for
//! bring-up and for tests that need backend specifics, while the trait is what
//! the renderer and the conformance harness talk to.

use crate::device::VulkanContext;
use crate::resource::VulkanTexture;
use impeller_hal::{
    Batch, Capabilities, Extent2D, Hal, HalContext, HalTexture, PixelFormat, Result,
    TextureDescriptor,
};

/// Marker naming the Vulkan backend's family of types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanHal;

impl Hal for VulkanHal {
    type Context = VulkanContext;
    type Texture = VulkanTexture;

    const NAME: &'static str = "vulkan";
}

impl HalTexture for VulkanTexture {
    fn extent(&self) -> Extent2D {
        VulkanTexture::extent(self)
    }

    fn format(&self) -> PixelFormat {
        VulkanTexture::format(self)
    }
}

impl HalContext for VulkanContext {
    type Hal = VulkanHal;

    fn capabilities(&self) -> &Capabilities {
        VulkanContext::capabilities(self)
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<VulkanTexture> {
        VulkanContext::create_texture(self, desc)
    }

    fn destroy_texture(&mut self, texture: VulkanTexture) {
        VulkanContext::destroy_texture(self, texture)
    }

    fn submit_batch(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        clear: Option<[f32; 4]>,
    ) -> Result<()> {
        VulkanContext::submit_batch(self, target, batch, clear)
    }

    fn read_texture(&mut self, texture: &mut VulkanTexture) -> Result<Vec<u8>> {
        VulkanContext::read_texture(self, texture)
    }
}
